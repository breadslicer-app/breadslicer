/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Printer and resin settings a person exports from another slicer and
//! brings in (#565): Lychee's `.lyp`, Photon Workshop's `.pwsp` and
//! CHITUBOX's `.cfgx`. Only
//! the files the person chooses are read, and their layout was worked out
//! from such exported files alone, for interoperability. Nothing of either
//! program ships here.
//!
//! A file gives its printers, each with the resins set for it. A resin
//! becomes a material profile calibrated for the printer of our catalog
//! the file's printer matches; the printer's own flags (mirroring and the
//! like) are never copied, since each program states them its own way.

use std::collections::BTreeMap;

use sc_print_primitives::{
    LightPwm, MaterialId, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm,
    PositiveFiniteMmPerMin, PositiveFiniteSec, PrinterId, ScaleCompensation,
    ScaleCompensationPercent,
};
use semver::Version;

use crate::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialPhysical, MaterialProfile, PrinterProfile, SecondTravelStage,
};

pub mod msgpack;

mod chitubox;
mod lychee;
mod workshop;

#[cfg(test)]
mod tests;

pub use chitubox::read_chitubox;
pub use lychee::{read_lychee, read_lychee_resins};
pub use workshop::read_photon_workshop;

/// A printer of an imported file, with the resins set for it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedPrinter {
    pub name: String,
    /// The panel's pixels, across and deep.
    pub resolution: (u32, u32),
    /// The plate, across and deep, in millimetres.
    pub plate_mm: (f64, f64),
    pub resins: Vec<ImportedResin>,
}

/// A resin of an imported file, as settings for its printer.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedResin {
    pub brand: String,
    pub name: String,
    pub layer_height_mm: Option<f64>,
    pub settings: MaterialPerPrinter,
    pub scale: Option<ScaleCompensation>,
    /// Settings the file holds that have no place in our profiles, by
    /// their name in the file: the import says so instead of dropping them
    /// unseen.
    pub not_imported: Vec<String>,
}

/// Why a file was not imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// Not a file of the kind, or one a newer version wrote.
    Unreadable(String),
    /// A file of the kind, with no printer and resin in it.
    Empty,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Unreadable(why) => write!(f, "the file could not be read: {why}"),
            ImportError::Empty => write!(f, "the file holds no printer with resins"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Reads `bytes`, a file named with `extension` (`lyp`, `pwsp` or `cfgx`).
pub fn read_printer_settings(
    extension: &str,
    bytes: &[u8],
) -> Result<Vec<ImportedPrinter>, ImportError> {
    match extension.to_ascii_lowercase().as_str() {
        "lyp" => read_lychee(bytes),
        "pwsp" => read_photon_workshop(bytes),
        "cfgx" => read_chitubox(bytes),
        other => Err(ImportError::Unreadable(format!(
            "files ending in .{other} are not printer settings"
        ))),
    }
}

/// The printer of `catalog` that `text` (a file name, say) names: the
/// longest printer name, with or without its maker, found in it, letters and
/// digits alone, so "Elegoo Mars 5 Ultra Resin.lyr" is the Mars 5 Ultra and
/// not the Mars 5 (#618).
pub fn printer_named_in<'a>(
    text: &str,
    printers: impl IntoIterator<Item = (&'a PrinterId, &'a str, &'a str)>,
) -> Option<PrinterId> {
    let letters = |text: &str| -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let named = letters(text);
    printers
        .into_iter()
        .filter_map(|(id, maker, name)| {
            let name = letters(name);
            let full = format!("{}{name}", letters(maker));
            let found = if named.contains(&full) {
                full.len()
            } else if !name.is_empty() && named.contains(&name) {
                name.len()
            } else {
                return None;
            };
            Some((found, id))
        })
        .max_by_key(|(found, _)| *found)
        .map(|(_, id)| id.clone())
}

/// How far a plate may measure from ours, as a share of it, and still be
/// the same printer: an export's printable area is the person's to edit.
const PLATE_SLACK: f64 = 0.01;

/// The printer of `catalog` an imported one is: the same panel pixels, the
/// plate within 1 % each way; of those, the one the file names, else the
/// closest plate.
pub fn matching_printer<'a>(
    imported: &ImportedPrinter,
    catalog: impl IntoIterator<Item = &'a PrinterProfile>,
) -> Option<PrinterId> {
    let off = |printer: &PrinterProfile| {
        let hw = &printer.hardware;
        let res = hw.pixel_resolution();
        let volume = hw.build_volume();
        let dx = (volume.x_mm() - imported.plate_mm.0).abs() / volume.x_mm();
        let dy = (volume.y_mm() - imported.plate_mm.1).abs() / volume.y_mm();
        ((res.x(), res.y()) == imported.resolution && dx <= PLATE_SLACK && dy <= PLATE_SLACK)
            .then_some(dx + dy)
    };
    // Printers that share a panel and a plate (the M7 and the M7 Pro) are
    // told apart by the file's name for its printer.
    let letters = |text: &str| -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let named = letters(&imported.name);
    let other_name = |printer: &PrinterProfile| {
        let ours = letters(&printer.identity.name.to_string());
        ours.is_empty() || !named.ends_with(&ours)
    };
    catalog
        .into_iter()
        .filter_map(|printer| off(printer).map(|d| ((other_name(printer), d), printer)))
        .min_by(|a, b| a.0.0.cmp(&b.0.0).then(a.0.1.total_cmp(&b.0.1)))
        .map(|(_, printer)| printer.identity.id.clone())
}

/// A lowercase id piece: letters and digits, the rest as single dashes.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// `resin` as a material profile of its own, calibrated for `printer`.
/// `source` names the program it came from in its id (`lychee`,
/// `workshop`). `None` when its names leave nothing to make an id of.
pub fn material_profile(
    resin: &ImportedResin,
    printer: &PrinterId,
    source: &str,
) -> Option<MaterialProfile> {
    let id = MaterialId::new(&format!(
        "{source}-{}-{}",
        slug(&resin.brand),
        slug(&resin.name)
    ))
    .ok()?;
    let brand = NonEmptyString::new(if resin.brand.trim().is_empty() {
        "Imported"
    } else {
        resin.brand.trim()
    })
    .ok()?;
    let name = NonEmptyString::new(resin.name.trim()).ok()?;
    let mut per_printer = BTreeMap::new();
    per_printer.insert(CalibrationTarget::from_printer(printer), resin.settings);
    Some(MaterialProfile {
        extension: false,
        identity: MaterialIdentity {
            id,
            brand,
            family: NonEmptyString::new("imported").expect("not empty"),
            name,
            profile_version: Version::new(1, 0, 0),
            origin: None,
        },
        intent: MaterialIntent {
            target_energy_density_mj_per_cm2: None,
            target_layer_height_mm: resin
                .layer_height_mm
                .and_then(|h| PositiveFiniteMm::new(h).ok()),
        },
        defaults: MaterialDefaults {
            scale_compensation_pct: resin.scale.filter(|s| *s != ScaleCompensation::NONE),
            ..MaterialDefaults::default()
        },
        physical: MaterialPhysical::default(),
        per_printer,
        notes: BTreeMap::new(),
    })
}

/// A material block with nothing set.
pub(crate) fn empty_settings() -> MaterialPerPrinter {
    MaterialPerPrinter {
        normal_exposure_sec: None,
        bottom_exposure_sec: None,
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

pub(crate) fn sec(v: Option<f64>) -> Option<PositiveFiniteSec> {
    v.and_then(|v| PositiveFiniteSec::new(v).ok())
}

pub(crate) fn wait(v: Option<f64>) -> Option<NonNegativeFiniteSec> {
    v.and_then(|v| NonNegativeFiniteSec::new(v).ok())
}

pub(crate) fn mm(v: Option<f64>) -> Option<PositiveFiniteMm> {
    v.and_then(|v| PositiveFiniteMm::new(v).ok())
}

pub(crate) fn per_min(v: Option<f64>) -> Option<PositiveFiniteMmPerMin> {
    v.and_then(|v| PositiveFiniteMmPerMin::new(v).ok())
}

pub(crate) fn pwm(v: Option<f64>) -> Option<LightPwm> {
    v.and_then(|v| LightPwm::new(v).ok())
}

/// A second stage when all three of its values are there.
pub(crate) fn stage(
    distance_mm: Option<f64>,
    up_mm_min: Option<f64>,
    down_mm_min: Option<f64>,
) -> Option<SecondTravelStage> {
    Some(SecondTravelStage {
        lift_distance_mm: mm(distance_mm)?,
        lift_speed_mm_min: per_min(up_mm_min)?,
        retract_speed_mm_min: per_min(down_mm_min)?,
    })
}

/// A scale of 100 % each way as a compensation: 101 % is +1 %. `None`
/// when an axis is missing or out of what our profiles hold.
pub(crate) fn scale_from_percent(x: f64, y: f64, z: f64) -> Option<ScaleCompensation> {
    Some(ScaleCompensation {
        x: ScaleCompensationPercent::new(x - 100.0).ok()?,
        y: ScaleCompensationPercent::new(y - 100.0).ok()?,
        z: ScaleCompensationPercent::new(z - 100.0).ok()?,
    })
}
