/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Prusa's published resin settings for the SL1 and SL1S, as material
//! profiles (#174).
//!
//! Prusa publishes them as a PrusaSlicer vendor bundle
//! (`PrusaResearchSLA/<version>.ini`, in `prusa3d/PrusaSlicer-settings-
//! prusa-sla`, the same data PrusaSlicer ships). Each `[sla_material:<name>
//! @<height> ...]` section is one resin at one layer height for one
//! machine, its values completed through its `inherits` parents; sections
//! named `*...*` are only parents. The SL1 and SL1S tilt the vat instead of
//! lifting the platform, so their tilt and tower settings have no place in
//! our profiles: they are reported and left out.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sc_print_primitives::{
    DensityGPerCm3, MaterialId, NonEmptyString, PositiveFiniteMm, PositiveFiniteSec, PrinterId,
    ScaleCompensation, ScaleCompensationPercent,
};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialPhysical, MaterialProfile,
};
use semver::Version;

use crate::elegoo_settings::{microns, slug};
use crate::profile::ConvertedProfile;
use crate::reader::{ProfileReader, SourceReading};
use crate::source_note::SourceNote;

/// Each printer model the bundle names, against the printer we ship.
const MODELS: [(&str, &str); 2] = [("SL1", "prusa-sl1"), ("SL1S", "prusa-sl1s-speed")];

/// Settings of the vat's tilt and the tower that no profile of ours has a
/// place for.
fn is_tilt_setting(key: &str) -> bool {
    key.starts_with("tilt_")
        || key.starts_with("tower_")
        || key.starts_with("delay_")
        || key == "use_tilt"
        || key == "area_fill"
}

/// Reads Prusa's SLA vendor bundle.
#[derive(Clone, Copy, Debug, Default)]
pub struct PrusaSlaReader;

/// Everything that can stop this reader.
#[derive(Debug)]
pub enum PrusaSlaError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A section inherits from itself, through its parents.
    BrokenInheritance { section: String, parent: String },
    /// A resin's name cannot be an id.
    InvalidId {
        resin: String,
        source: sc_print_primitives::InvalidProfileId,
    },
}

impl fmt::Display for PrusaSlaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::BrokenInheritance { section, parent } => write!(
                f,
                "[{section}] inherits from {parent:?}, which loops back to it"
            ),
            Self::InvalidId { resin, source } => write!(f, "{resin:?} cannot be named: {source}"),
        }
    }
}

impl std::error::Error for PrusaSlaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidId { source, .. } => Some(source),
            Self::BrokenInheritance { .. } => None,
        }
    }
}

type Section = BTreeMap<String, String>;

/// The `[sla_material:...]` sections of an INI bundle, in file order.
fn material_sections(text: &str) -> Vec<(String, Section)> {
    let mut sections: Vec<(String, Section)> = Vec::new();
    let mut current: Option<usize> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = header.strip_prefix("sla_material:").map(|name| {
                sections.push((name.trim().to_owned(), Section::new()));
                sections.len() - 1
            });
            continue;
        }
        if let (Some(at), Some((key, value))) = (current, line.split_once('=')) {
            sections[at]
                .1
                .insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    sections
}

/// A section's values completed through its parents: each parent in the
/// order `inherits` lists them, later ones over earlier ones, and the
/// section's own values over all of them.
/// A parent the bundle does not hold is skipped and named in `missing`,
/// as PrusaSlicer loads such a section with what its other parents give.
fn resolved(
    name: &str,
    all: &BTreeMap<String, Section>,
    seen: &mut Vec<String>,
    missing: &mut Vec<String>,
) -> Result<Section, PrusaSlaError> {
    let own = all
        .get(name)
        .ok_or_else(|| PrusaSlaError::BrokenInheritance {
            section: seen.last().cloned().unwrap_or_default(),
            parent: name.to_owned(),
        })?;
    if seen.iter().any(|s| s == name) {
        return Err(PrusaSlaError::BrokenInheritance {
            section: name.to_owned(),
            parent: name.to_owned(),
        });
    }
    seen.push(name.to_owned());
    let mut values = Section::new();
    if let Some(parents) = own.get("inherits") {
        for parent in parents.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            if !all.contains_key(parent) {
                missing.push(parent.to_owned());
                continue;
            }
            values.extend(resolved(parent, all, seen, missing)?);
        }
    }
    seen.pop();
    values.extend(own.iter().map(|(k, v)| (k.clone(), v.clone())));
    values.remove("inherits");
    Ok(values)
}

fn number(values: &Section, key: &str) -> Option<f64> {
    values
        .get(key)?
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite())
}

/// The printer a section's `compatible_printers_condition` names.
fn model_of(values: &Section) -> Option<&'static str> {
    let condition = values.get("compatible_printers_condition")?;
    MODELS
        .iter()
        .find(|(model, _)| condition.contains(&format!("\"{model}\"")))
        .map(|(_, printer)| *printer)
}

/// The brand a vendor stands for, in the bundle's words.
fn brand_of(vendor: &str) -> String {
    match vendor.trim() {
        "Forward AM" => "BASF Forward AM".to_owned(),
        "Henkel" => "Loctite".to_owned(),
        "Esun" => "eSun".to_owned(),
        "Monocure" => "Monocure 3D".to_owned(),
        other => other.to_owned(),
    }
}

/// `Liqcreate Clear Impact @0.05 SL1S` as `Clear Impact`: no height and
/// machine after the `@`, and no vendor or brand in front (`3DM-ABS` and
/// `LOCTITE 3D IND475` too).
fn resin_of(section: &str, vendor: &str, brand: &str) -> String {
    let name = section.split('@').next().unwrap_or(section).trim();
    let mut heads: Vec<String> = [brand, vendor]
        .iter()
        .flat_map(|w| [format!("{w} 3D"), (*w).to_owned()])
        .filter(|w| !w.trim().is_empty())
        .collect();
    heads.sort_by_key(|h| std::cmp::Reverse(h.len()));
    for head in heads {
        let Some(front) = name.get(..head.len()) else {
            continue;
        };
        if !front.eq_ignore_ascii_case(&head) {
            continue;
        }
        let rest = &name[head.len()..];
        if let Some(rest) = rest.strip_prefix([' ', '-'])
            && !rest.trim().is_empty()
        {
            return rest.trim().to_owned();
        }
    }
    name.to_owned()
}

/// The scale correction a section states, as percentages, when it is not
/// 1 on every axis.
fn correction_of(values: &Section) -> Option<ScaleCompensation> {
    let joined: Vec<f64> = values
        .get("material_correction")
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_default();
    let axis = |key: &str, index: usize| {
        number(values, key)
            .or_else(|| joined.get(index).copied())
            .unwrap_or(1.0)
    };
    let factors = [
        axis("material_correction_x", 0),
        axis("material_correction_y", 1),
        axis("material_correction_z", 2),
    ];
    if factors.iter().all(|f| (*f - 1.0).abs() < 1e-9) {
        return None;
    }
    let pct = |f: f64| ScaleCompensationPercent::new(((f - 1.0) * 100.0 * 1e6).round() / 1e6).ok();
    Some(ScaleCompensation {
        x: pct(factors[0])?,
        y: pct(factors[1])?,
        z: pct(factors[2])?,
    })
}

/// The library's family of a resin, in the bundle's words.
fn family_of(resin: &str) -> &'static str {
    let resin = resin.to_ascii_lowercase();
    if resin.contains("abs") {
        "abs-like"
    } else if resin.contains("water washable") || resin.contains("water-washable") {
        "water-washable"
    } else if resin.contains("cast") {
        "castable"
    } else {
        "standard"
    }
}

/// A calibration block with only the exposures set: the vat tilts, so no
/// lift applies.
fn block(normal: PositiveFiniteSec, bottom: PositiveFiniteSec) -> MaterialPerPrinter {
    MaterialPerPrinter {
        normal_exposure_sec: Some(normal),
        bottom_exposure_sec: Some(bottom),
        bottom_layer_count: None,
        wait_before_cure_sec: None,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: None,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: None,
        lift_speed_mm_min: None,
        retract_speed_mm_min: None,
        bottom_lift_distance_mm: None,
        bottom_lift_speed_mm_min: None,
        bottom_retract_speed_mm_min: None,
        light_pwm: None,
        bottom_light_pwm: None,
        second_travel: None,
        bottom_second_travel: None,
    }
}

impl ProfileReader for PrusaSlaReader {
    type Error = PrusaSlaError;

    fn read(&self, inputs: &[&Path]) -> Result<SourceReading, Self::Error> {
        let mut resins: BTreeMap<MaterialId, MaterialProfile> = BTreeMap::new();
        let mut notes = Vec::new();
        let mut disagree = Vec::new();
        let mut tilted = 0;
        let mut unread = 0;
        let mut corrections: BTreeMap<MaterialId, Option<ScaleCompensation>> = BTreeMap::new();
        let mut missing = Vec::new();
        for path in inputs {
            let text = std::fs::read_to_string(path).map_err(|source| PrusaSlaError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let sections = material_sections(&text);
            let all: BTreeMap<String, Section> = sections.iter().cloned().collect();
            for (name, _) in &sections {
                if name.starts_with('*') {
                    continue;
                }
                let values = resolved(name, &all, &mut Vec::new(), &mut missing)?;
                if values.keys().any(|k| is_tilt_setting(k)) {
                    tilted += 1;
                }
                let vendor = values.get("material_vendor").map(String::as_str);
                let (Some(printer), Some(vendor), Some(normal), Some(bottom), Some(height)) = (
                    model_of(&values),
                    vendor.filter(|v| !v.trim().is_empty()),
                    number(&values, "exposure_time").and_then(|v| PositiveFiniteSec::new(v).ok()),
                    number(&values, "initial_exposure_time")
                        .and_then(|v| PositiveFiniteSec::new(v).ok()),
                    number(&values, "initial_layer_height")
                        .and_then(|v| PositiveFiniteMm::new(v).ok()),
                ) else {
                    unread += 1;
                    continue;
                };
                let brand = brand_of(vendor);
                let resin = resin_of(name, vendor, &brand);
                let id_text = format!("{}-{}-{}um", slug(&brand), slug(&resin), microns(height));
                let id = MaterialId::new(&id_text).map_err(|source| PrusaSlaError::InvalidId {
                    resin: resin.clone(),
                    source,
                })?;
                let correction = correction_of(&values);
                match corrections.get(&id) {
                    Some(held) if *held != correction => disagree.push(id.clone()),
                    Some(_) => {}
                    None => {
                        corrections.insert(id.clone(), correction);
                    }
                }
                let density =
                    number(&values, "material_density").and_then(|d| DensityGPerCm3::new(d).ok());
                let profile = resins.entry(id.clone()).or_insert_with(|| MaterialProfile {
                    extension: false,
                    identity: MaterialIdentity {
                        id: id.clone(),
                        brand: NonEmptyString::new(&brand).expect("not empty"),
                        family: NonEmptyString::new(family_of(&resin)).expect("a constant"),
                        name: NonEmptyString::new(&format!("{resin} ({} um)", microns(height)))
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
                let printer = PrinterId::new(printer).expect("the map holds only ids we ship");
                let target = CalibrationTarget::from_printer(&printer);
                let stated = block(normal, bottom);
                match profile.per_printer.get(&target) {
                    Some(held) if *held != stated => {
                        notes.push(SourceNote::RowsDisagree {
                            material: id.clone(),
                            printer,
                        });
                    }
                    Some(_) => {}
                    None => {
                        profile.per_printer.insert(target, stated);
                    }
                }
            }
        }
        for (id, correction) in &corrections {
            if disagree.contains(id) {
                continue;
            }
            if let Some(profile) = resins.get_mut(id) {
                profile.defaults.scale_compensation_pct = *correction;
            }
        }
        if !disagree.is_empty() {
            notes.push(SourceNote::SettingsWithNoPlace {
                settings: "a scale correction that differs between machines".to_owned(),
                rows: disagree.len(),
            });
        }
        missing.sort();
        missing.dedup();
        for parent in missing {
            notes.push(SourceNote::SettingsWithNoPlace {
                settings: format!("a parent {parent:?} the bundle does not hold"),
                rows: 1,
            });
        }
        if tilted > 0 {
            notes.push(SourceNote::SettingsWithNoPlace {
                settings: "the vat's tilt and tower settings".to_owned(),
                rows: tilted,
            });
        }
        if unread > 0 {
            notes.push(SourceNote::RowsWeCannotRead {
                model: "SL1/SL1S".to_owned(),
                rows: unread,
            });
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
mod prusa_sla_tests {
    use super::*;

    const BUNDLE: &str = "\
[vendor]
name = Prusa Research

[sla_material:*common*]
compatible_printers_condition = printer_model==\"SL1\"
exposure_time = 12
initial_exposure_time = 45
initial_layer_height = 0.05
material_correction = 1,1,1

[sla_material:*0.05_sl1s*]
inherits = *common*
compatible_printers_condition = printer_model==\"SL1S\"
exposure_time = 1.7
initial_exposure_time = 25

[sla_material:*sl1s_fast*]
use_tilt = 1,1
tilt_down_initial_speed = layer400,layer400

[sla_material:Liqcreate Dental Model Pro Beige @0.05 SL1S]
inherits = *0.05_sl1s*; *sl1s_fast*
exposure_time = 3
material_vendor = Liqcreate
material_density = 1.15
material_correction_x = 1.0073
material_correction_y = 1.0073

[sla_material:Liqcreate Dental Model Pro Beige @0.05]
inherits = *common*
exposure_time = 9
initial_exposure_time = 40
material_vendor = Liqcreate
material_correction_x = 1.0073
material_correction_y = 1.0073

[sla_material:Prusament Resin Tough Prusa Orange @0.05 SL1S]
inherits = *0.05_sl1s*
material_vendor = Prusa Polymers
";

    fn read(text: &str) -> SourceReading {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("PrusaResearchSLA.ini");
        std::fs::write(&path, text).unwrap();
        PrusaSlaReader.read(&[path.as_path()]).unwrap()
    }

    fn material<'a>(reading: &'a SourceReading, id: &str) -> &'a MaterialProfile {
        reading
            .profiles
            .iter()
            .find_map(|p| match p {
                ConvertedProfile::Material(m) if m.identity.id.as_str() == id => Some(m),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {id}"))
    }

    #[test]
    fn a_resin_on_both_machines_is_one_profile_with_their_values_through_inheritance() {
        let reading = read(BUNDLE);
        let resin = material(&reading, "liqcreate-dental-model-pro-beige-50um");
        assert_eq!(resin.identity.brand.as_str(), "Liqcreate");
        assert_eq!(
            resin.identity.name.as_str(),
            "Dental Model Pro Beige (50 um)"
        );
        let sl1s = PrinterId::new("prusa-sl1s-speed").unwrap();
        let sl1 = PrinterId::new("prusa-sl1").unwrap();
        let s = resin.per_printer[&CalibrationTarget::from_printer(&sl1s)];
        // Its own exposure over the parent's, the parent's bottom one.
        assert_eq!(s.normal_exposure_sec.unwrap().value(), 3.0);
        assert_eq!(s.bottom_exposure_sec.unwrap().value(), 25.0);
        let o = resin.per_printer[&CalibrationTarget::from_printer(&sl1)];
        assert_eq!(o.normal_exposure_sec.unwrap().value(), 9.0);
        assert_eq!(o.bottom_exposure_sec.unwrap().value(), 40.0);
        let c = resin.defaults.scale_compensation_pct.unwrap();
        assert!((c.x.value() - 0.73).abs() < 1e-9);
        assert_eq!(c.z.value(), 0.0);
        assert_eq!(
            resin.physical.liquid_density_g_per_cm3.unwrap().value(),
            1.15
        );
    }

    #[test]
    fn a_resin_with_no_correction_carries_none_and_the_tilt_is_reported() {
        let reading = read(BUNDLE);
        let orange = material(
            &reading,
            "prusa-polymers-prusament-resin-tough-prusa-orange-50um",
        );
        assert!(orange.defaults.scale_compensation_pct.is_none());
        assert!(
            reading
                .notes
                .iter()
                .any(|n| matches!(n, SourceNote::SettingsWithNoPlace { rows: 1, .. }))
        );
    }

    #[test]
    fn a_missing_parent_is_skipped_and_noted() {
        let reading = read(&format!(
            "{BUNDLE}\n[sla_material:Z Resin @0.05 SL1S]\ninherits = *0.05_sl1s*; *nowhere*\nmaterial_vendor = Y\n"
        ));
        let z = material(&reading, "y-z-resin-50um");
        assert_eq!(z.per_printer.len(), 1);
        assert!(reading.notes.iter().any(|n| matches!(
            n,
            SourceNote::SettingsWithNoPlace { settings, .. } if settings.contains("*nowhere*")
        )));
    }

    #[test]
    fn a_section_that_inherits_from_itself_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.ini");
        std::fs::write(
            &path,
            "[sla_material:*a*]\ninherits = *b*\n[sla_material:*b*]\ninherits = *a*\n[sla_material:X @0.05]\ninherits = *a*\nmaterial_vendor = Y\n",
        )
        .unwrap();
        assert!(matches!(
            PrusaSlaReader.read(&[path.as_path()]),
            Err(PrusaSlaError::BrokenInheritance { .. })
        ));
    }

    #[test]
    fn a_name_loses_its_height_and_vendor() {
        assert_eq!(
            resin_of("Liqcreate Clear Impact @0.1 SL1S", "Liqcreate", "Liqcreate"),
            "Clear Impact"
        );
        assert_eq!(
            resin_of("ALTANA Cubic Ink 4-2100 VP @0.05 SL1S", "Altana", "Altana"),
            "Cubic Ink 4-2100 VP"
        );
        assert_eq!(
            resin_of(
                "Prusament Resin Model Grass Green @0.025 SL1S",
                "Prusa Polymers",
                "Prusa Polymers"
            ),
            "Prusament Resin Model Grass Green"
        );
        assert_eq!(resin_of("3DM-ABS @0.025", "3DM", "3DM"), "ABS");
        assert_eq!(
            resin_of("LOCTITE 3D IND475 @0.05 SL1S", "Henkel", "Loctite"),
            "IND475"
        );
    }
}
