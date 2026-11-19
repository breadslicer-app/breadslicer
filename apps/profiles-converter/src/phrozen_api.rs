/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Phrozen's resin settings, as material profiles (#172).
//!
//! Phrozen's resin profile pages (`phrozen3d.com/pages/resin-<printer>`)
//! fill their tables from a public JSON service:
//! `resin.phrozen3d.info/api/resin-variants?machine=<name>` answers one
//! entry per resin and colour for that machine, and
//! `/api/resins?machine=<name>` the brand of each resin. The inputs are
//! those answers saved as `<machine>.variants.json`; a
//! `<machine>.resins.json` next to one names the brands. Exposures come as
//! ranges (`35 ± 5`) and the profile takes the middle.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use sc_print_primitives::{
    DensityGPerCm3, MaterialId, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm,
    PositiveFiniteMmPerMin, PositiveFiniteSec, PrinterId,
};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialPhysical, MaterialProfile, SecondTravelStage,
};
use semver::Version;
use serde::Deserialize;

use crate::elegoo_settings::{microns, slug};
use crate::profile::ConvertedProfile;
use crate::reader::{ProfileReader, SourceReading};
use crate::source_note::SourceNote;

/// Each machine the service answers for, against the printer we ship for
/// it.
const MACHINES: [(&str, Option<&str>); 11] = [
    ("Phrozen Sonic Mini 4K", Some("phrozen-sonic-mini-4k")),
    ("Phrozen Sonic Mini 8K", Some("phrozen-sonic-mini-8k")),
    ("Phrozen Sonic Mini 8K S", Some("phrozen-sonic-mini-8k-s")),
    ("Phrozen Sonic Mighty 4K", Some("phrozen-sonic-mighty-4k")),
    ("Phrozen Sonic Mighty 8K", Some("phrozen-sonic-mighty-8k")),
    ("Phrozen Sonic Mighty 12K", None),
    (
        "Phrozen Sonic Mighty Revo",
        Some("phrozen-sonic-mighty-revo"),
    ),
    ("Phrozen Sonic Mighty Revo 16K", None),
    ("Phrozen Sonic Mega 8K", Some("phrozen-sonic-mega-8k")),
    ("Phrozen Sonic Mega 8K S", None),
    ("Phrozen Sonic Mega 8K V2", None),
];

/// The brand each of the service's brand keys stands for.
fn brand_name(key: &str) -> String {
    match key.trim().to_ascii_lowercase().as_str() {
        "" | "phrozen" => "Phrozen".to_owned(),
        "basf" => "BASF Forward AM".to_owned(),
        "henkel" => "Loctite".to_owned(),
        _ => {
            let mut chars = key.trim().chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// Reads Phrozen's resin settings service, saved per machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhrozenApiReader;

/// Everything that can stop this reader.
#[derive(Debug)]
pub enum PhrozenApiError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// An answer names a machine the reader does not know.
    UnknownMachine { path: PathBuf, machine: String },
    /// A resin's name cannot be an id.
    InvalidId {
        resin: String,
        source: sc_print_primitives::InvalidProfileId,
    },
}

impl fmt::Display for PhrozenApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Json { path, source } => write!(f, "{}: {source}", path.display()),
            Self::UnknownMachine { path, machine } => write!(
                f,
                "{} answers for {machine:?}, a machine this reader does not know",
                path.display()
            ),
            Self::InvalidId { resin, source } => write!(f, "{resin:?} cannot be named: {source}"),
        }
    }
}

impl std::error::Error for PhrozenApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::InvalidId { source, .. } => Some(source),
            Self::UnknownMachine { .. } => None,
        }
    }
}

#[derive(Deserialize)]
struct Variant {
    name: String,
    variant: Option<String>,
    datasheet: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct Resin {
    name: String,
    #[serde(default)]
    brand: Option<String>,
}

/// A figure as the service writes it: a number, or a range `a ± b`, read
/// as its middle `a`. Empty is no figure.
fn figure(value: Option<&serde_json::Value>) -> Option<f64> {
    let text = match value? {
        serde_json::Value::Number(n) => return n.as_f64(),
        serde_json::Value::String(s) => s,
        _ => return None,
    };
    let middle = text.split('±').next()?.trim();
    middle.parse().ok().filter(|v: &f64| v.is_finite())
}

/// The calibration an entry states, or `None` when an exposure or the
/// travel is missing.
fn block_of(sheet: &BTreeMap<String, serde_json::Value>) -> Option<MaterialPerPrinter> {
    let get = |key: &str| figure(sheet.get(key));
    let mm = |key: &str| get(key).and_then(|v| PositiveFiniteMm::new(v).ok());
    let speed = |key: &str| get(key).and_then(|v| PositiveFiniteMmPerMin::new(v).ok());
    let second = |prefix: &str| {
        Some(SecondTravelStage {
            lift_distance_mm: mm(&format!("{prefix}_second_lifting_distance_mm"))?,
            lift_speed_mm_min: speed(&format!("{prefix}_second_lifting_speed_mm_min"))?,
            retract_speed_mm_min: speed(&format!("{prefix}_second_retract_speed_mm_min"))?,
        })
    };
    let resting = sheet
        .get("waiting_mode_during_printing_mode")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|mode| mode.trim().eq_ignore_ascii_case("resting time"));
    let rest = resting
        .then(|| get("waiting_mode_during_printing_rest_time_after_retract_s"))
        .flatten()
        .and_then(|v| NonNegativeFiniteSec::new(v).ok());
    Some(MaterialPerPrinter {
        normal_exposure_sec: Some(
            PositiveFiniteSec::new(get("normal_layers_exposure_time_s")?).ok()?,
        ),
        bottom_exposure_sec: Some(
            PositiveFiniteSec::new(get("bottom_layers_exposure_time_s")?).ok()?,
        ),
        bottom_layer_count: get("bottom_layers_layer_count")
            .filter(|n| *n >= 0.0 && n.fract() == 0.0)
            .map(|n| n as u32),
        // The rest after the platform comes back is the wait before the
        // next exposure.
        transition_layer_count: None,
        wait_before_cure_sec: rest,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: rest,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: Some(mm("normal_layers_lifting_distance_mm")?),
        lift_speed_mm_min: Some(speed("normal_layers_lifting_speed_mm_min")?),
        retract_speed_mm_min: Some(speed("normal_layers_retract_speed_mm_min")?),
        bottom_lift_distance_mm: mm("bottom_layers_lifting_distance_mm"),
        bottom_lift_speed_mm_min: speed("bottom_layers_lifting_speed_mm_min"),
        bottom_retract_speed_mm_min: speed("bottom_layers_retract_speed_mm_min"),
        light_pwm: None,
        bottom_light_pwm: None,
        second_travel: second("normal_layers"),
        bottom_second_travel: second("bottom_layers"),
    })
}

/// A resin's name as the profile carries it: without the brand in front
/// (the profile states it apart) and with its colour, unless the colour
/// only repeats the name.
fn resin_name(brand: &str, resin: &str, colour: &str) -> String {
    let resin = resin
        .get(..brand.len())
        .filter(|head| head.eq_ignore_ascii_case(brand))
        .and_then(|_| resin[brand.len()..].strip_prefix(' '))
        .map(str::trim)
        .filter(|rest| !rest.is_empty())
        .unwrap_or(resin);
    if colour.is_empty() || colour.eq_ignore_ascii_case(resin) {
        resin.to_owned()
    } else {
        format!("{resin} {colour}")
    }
}

/// The library's family of a resin, in the bundle's words.
fn family_of(resin: &str) -> &'static str {
    let resin = resin.to_ascii_lowercase();
    if resin.contains("abs") {
        "abs-like"
    } else if resin.contains("water washable") || resin.contains("water-washable") {
        "water-washable"
    } else if resin.contains("castable") || resin.contains("cast") {
        "castable"
    } else {
        "standard"
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, PhrozenApiError> {
    let text = std::fs::read_to_string(path).map_err(|source| PhrozenApiError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| PhrozenApiError::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// The brand of each resin, from the `.resins.json` saved next to a
/// `.variants.json`, when there is one.
fn brands_next_to(path: &Path) -> Result<BTreeMap<String, String>, PhrozenApiError> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let Some(stem) = name.strip_suffix(".variants.json") else {
        return Ok(BTreeMap::new());
    };
    let resins = path.with_file_name(format!("{stem}.resins.json"));
    if !resins.exists() {
        return Ok(BTreeMap::new());
    }
    let list: Vec<Resin> = read_json(&resins)?;
    Ok(list
        .into_iter()
        .map(|r| (r.name.trim().to_owned(), r.brand.unwrap_or_default()))
        .collect())
}

impl ProfileReader for PhrozenApiReader {
    type Error = PhrozenApiError;

    fn read(&self, inputs: &[&Path]) -> Result<SourceReading, Self::Error> {
        let mut resins: BTreeMap<MaterialId, MaterialProfile> = BTreeMap::new();
        let mut disagree: BTreeSet<(MaterialId, PrinterId)> = BTreeSet::new();
        let mut notes = Vec::new();
        // A resin's brand is the same on every machine, and some answers
        // leave a resin out of their brand list.
        let mut brands = BTreeMap::new();
        for path in inputs {
            for (resin, brand) in brands_next_to(path)? {
                if !brand.is_empty() {
                    brands.entry(resin).or_insert(brand);
                }
            }
        }
        for path in inputs {
            let variants: Vec<Variant> = read_json(path)?;
            let Some(machine) = variants.iter().find_map(|v| {
                v.datasheet
                    .get("machine")
                    .and_then(serde_json::Value::as_str)
                    .map(|m| m.trim().to_owned())
            }) else {
                continue;
            };
            let Some((_, ours)) = MACHINES.iter().find(|(m, _)| *m == machine) else {
                return Err(PhrozenApiError::UnknownMachine {
                    path: path.to_path_buf(),
                    machine,
                });
            };
            let Some(printer) = ours else {
                notes.push(SourceNote::RowsForAMachineWeDoNotShip {
                    model: machine,
                    rows: variants.len(),
                });
                continue;
            };
            let printer = PrinterId::new(printer).expect("the map holds only ids we ship");
            let mut unread = 0;
            for entry in &variants {
                let resin = entry.name.trim();
                let colour = entry.variant.as_deref().map(str::trim).unwrap_or("");
                let height = figure(entry.datasheet.get("basic_info_layer_height_mm"))
                    .and_then(|h| PositiveFiniteMm::new(h).ok());
                let (Some(block), Some(height), false) =
                    (block_of(&entry.datasheet), height, resin.is_empty())
                else {
                    unread += 1;
                    continue;
                };
                let brand = brand_name(brands.get(resin).map(String::as_str).unwrap_or(""));
                let named = resin_name(&brand, resin, colour);
                let id_text = format!("{}-{}-{}um", slug(&brand), slug(&named), microns(height));
                let id =
                    MaterialId::new(&id_text).map_err(|source| PhrozenApiError::InvalidId {
                        resin: named.clone(),
                        source,
                    })?;
                let density = figure(entry.datasheet.get("basic_info_density_g_ml"))
                    .and_then(|d| DensityGPerCm3::new(d).ok());
                let profile = resins.entry(id.clone()).or_insert_with(|| MaterialProfile {
                    extension: false,
                    identity: MaterialIdentity {
                        id: id.clone(),
                        brand: NonEmptyString::new(&brand).expect("not empty"),
                        family: NonEmptyString::new(family_of(resin)).expect("a constant"),
                        name: NonEmptyString::new(&format!("{named} ({} um)", microns(height)))
                            .expect("not empty"),
                        profile_version: Version::new(1, 0, 0),
                        origin: None,
                    },
                    intent: MaterialIntent {
                        target_energy_density_mj_per_cm2: None,
                        target_layer_height_mm: Some(height),
                    },
                    defaults: MaterialDefaults::default(),
                    physical: MaterialPhysical::default(),
                    per_printer: BTreeMap::new(),
                    notes: BTreeMap::new(),
                });
                if profile.physical.liquid_density_g_per_cm3.is_none() {
                    profile.physical.liquid_density_g_per_cm3 = density;
                }
                let target = CalibrationTarget::from_printer(&printer);
                match profile.per_printer.get(&target) {
                    Some(held) if *held != block => {
                        disagree.insert((id, printer.clone()));
                    }
                    Some(_) => {}
                    None => {
                        profile.per_printer.insert(target, block);
                    }
                }
            }
            if unread > 0 {
                notes.push(SourceNote::RowsWeCannotRead {
                    model: machine,
                    rows: unread,
                });
            }
        }
        for (material, printer) in disagree {
            if let Some(profile) = resins.get_mut(&material) {
                profile
                    .per_printer
                    .remove(&CalibrationTarget::from_printer(&printer));
                if profile.per_printer.is_empty() {
                    resins.remove(&material);
                }
            }
            notes.push(SourceNote::RowsDisagree { material, printer });
        }
        Ok(SourceReading {
            profiles: resins
                .into_values()
                .map(ConvertedProfile::Material)
                .collect(),
            notes,
        })
    }
}

#[cfg(test)]
mod phrozen_api_tests {
    use super::*;

    const MINI_8K: &str = r#"[{
        "name": "Aqua 8K Resin\t", "variant": "Gray", "color": null,
        "datasheet": {
            "machine": "Phrozen Sonic Mini 8K",
            "bottom_layers_layer_count": "6",
            "basic_info_layer_height_mm": "0.05",
            "basic_info_density_g_ml": "1.1",
            "bottom_layers_exposure_time_s": "35 ± 5",
            "normal_layers_exposure_time_s": "2.3 ± 0.3",
            "transition_layers_layer_count": "6",
            "bottom_layers_lifting_distance_mm": "6",
            "bottom_layers_retract_distance_mm": "6",
            "normal_layers_lifting_distance_mm": "5",
            "normal_layers_retract_distance_mm": "5",
            "waiting_mode_during_printing_mode": "Resting time",
            "bottom_layers_lifting_speed_mm_min": "60",
            "bottom_layers_retract_speed_mm_min": "150",
            "normal_layers_lifting_speed_mm_min": "70",
            "normal_layers_retract_speed_mm_min": "160",
            "waiting_mode_during_printing_rest_time_after_retract_s": "4 ± 1",
            "normal_layers_second_lifting_distance_mm": "",
            "normal_layers_second_lifting_speed_mm_min": ""
        }
    }, {
        "name": "IND249", "variant": null,
        "datasheet": {
            "machine": "Phrozen Sonic Mini 8K",
            "bottom_layers_layer_count": "8",
            "basic_info_layer_height_mm": "0.1",
            "bottom_layers_exposure_time_s": "20",
            "normal_layers_exposure_time_s": "",
            "bottom_layers_lifting_distance_mm": "6",
            "normal_layers_lifting_distance_mm": "6",
            "bottom_layers_lifting_speed_mm_min": "60",
            "bottom_layers_retract_speed_mm_min": "150",
            "normal_layers_lifting_speed_mm_min": "60",
            "normal_layers_retract_speed_mm_min": "150",
            "waiting_mode_during_printing_mode": ""
        }
    }]"#;

    const MINI_8K_RESINS: &str = r#"[
        {"id": 1, "name": "Aqua 8K Resin", "brand": "phrozen"},
        {"id": 2, "name": "IND249", "brand": "henkel"}
    ]"#;

    fn read(files: &[(&str, &str)]) -> SourceReading {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for (name, text) in files {
            let path = dir.path().join(name);
            std::fs::write(&path, text).unwrap();
            if name.ends_with(".variants.json") {
                paths.push(path);
            }
        }
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        PhrozenApiReader.read(&refs).unwrap()
    }

    #[test]
    fn an_entry_becomes_a_material_for_the_machine_it_was_asked_for() {
        let reading = read(&[
            ("mini-8k.variants.json", MINI_8K),
            ("mini-8k.resins.json", MINI_8K_RESINS),
        ]);
        let [ConvertedProfile::Material(profile)] = reading.profiles.as_slice() else {
            panic!("one material, the other entry states no normal exposure");
        };
        assert_eq!(
            profile.identity.id.as_str(),
            "phrozen-aqua-8k-resin-gray-50um"
        );
        assert_eq!(profile.identity.brand.as_str(), "Phrozen");
        assert_eq!(profile.identity.name.as_str(), "Aqua 8K Resin Gray (50 um)");
        assert_eq!(
            profile.physical.liquid_density_g_per_cm3.unwrap().value(),
            1.1
        );
        let printer = PrinterId::new("phrozen-sonic-mini-8k").unwrap();
        let b = profile.per_printer[&CalibrationTarget::from_printer(&printer)];
        // A range is read as its middle.
        assert_eq!(b.normal_exposure_sec.unwrap().value(), 2.3);
        assert_eq!(b.bottom_exposure_sec.unwrap().value(), 35.0);
        assert_eq!(b.bottom_layer_count, Some(6));
        assert_eq!(b.lift_distance_mm.unwrap().value(), 5.0);
        assert_eq!(b.lift_speed_mm_min.unwrap().value(), 70.0);
        assert_eq!(b.retract_speed_mm_min.unwrap().value(), 160.0);
        assert_eq!(b.bottom_lift_distance_mm.unwrap().value(), 6.0);
        assert_eq!(b.bottom_retract_speed_mm_min.unwrap().value(), 150.0);
        assert_eq!(b.wait_before_cure_sec.unwrap().value(), 4.0);
        assert_eq!(b.bottom_wait_before_cure_sec.unwrap().value(), 4.0);
        // An empty second stage is no second stage.
        assert!(b.second_travel.is_none());
        assert_eq!(reading.notes.len(), 1, "the unreadable entry is noted");
    }

    #[test]
    fn a_machine_we_do_not_ship_is_noted() {
        let mighty_12k = MINI_8K.replace("Phrozen Sonic Mini 8K", "Phrozen Sonic Mighty 12K");
        let reading = read(&[("m12.variants.json", &mighty_12k)]);
        assert!(reading.profiles.is_empty());
        assert!(matches!(
            reading.notes.as_slice(),
            [SourceNote::RowsForAMachineWeDoNotShip { rows: 2, .. }]
        ));
    }

    #[test]
    fn another_brand_s_resin_carries_its_brand() {
        let filled = MINI_8K.replace(
            "\"normal_layers_exposure_time_s\": \"\"",
            "\"normal_layers_exposure_time_s\": \"3\"",
        );
        let reading = read(&[
            ("mini-8k.variants.json", &filled),
            ("mini-8k.resins.json", MINI_8K_RESINS),
        ]);
        let ids: Vec<&str> = reading
            .profiles
            .iter()
            .map(|p| match p {
                ConvertedProfile::Material(m) => m.identity.id.as_str(),
                _ => "",
            })
            .collect();
        assert!(ids.contains(&"loctite-ind249-100um"), "{ids:?}");
    }

    #[test]
    fn an_unknown_machine_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.variants.json");
        std::fs::write(&path, MINI_8K.replace("Sonic Mini 8K", "Sonic Giga 99K")).unwrap();
        assert!(matches!(
            PhrozenApiReader.read(&[path.as_path()]),
            Err(PhrozenApiError::UnknownMachine { .. })
        ));
    }

    #[test]
    fn a_name_drops_its_brand_and_a_colour_that_repeats_it() {
        assert_eq!(
            resin_name("Phrozen", "Phrozen Xmas Special Resin", "Elf Green"),
            "Xmas Special Resin Elf Green"
        );
        assert_eq!(resin_name("Phrozen", "FR940", "FR940"), "FR940");
        assert_eq!(
            resin_name("Phrozen", "Aqua Clear Plus", ""),
            "Aqua Clear Plus"
        );
    }

    #[test]
    fn a_range_reads_as_its_middle() {
        let v = |s: &str| serde_json::Value::String(s.to_owned());
        assert_eq!(figure(Some(&v("27.5 ± 2.5 "))), Some(27.5));
        assert_eq!(figure(Some(&v("10± 5"))), Some(10.0));
        assert_eq!(figure(Some(&v("6"))), Some(6.0));
        assert_eq!(figure(Some(&v(""))), None);
    }
}
