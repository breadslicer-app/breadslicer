/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Creality's wiki pages of resin settings for its newer printers, as
//! material profiles (#175).
//!
//! Each page is one printer's (`wiki.creality.com/en/halot-series/<model>/
//! resin-parameters`, the model in the page's `og:url`), with one small
//! Parameter / Value table per resin under the resin's heading, named
//! `<brand>-<resin>` (Creality's own resins and its PioCreat line). The
//! inputs are the pages saved as HTML. The X1 Max page is an image and the
//! Mage series has none; pages for printers we do not ship are noted.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sc_print_primitives::{
    MaterialId, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin,
    PositiveFiniteSec, PrinterId,
};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialPhysical, MaterialProfile,
};
use semver::Version;

use crate::elegoo_settings::{microns, slug};
use crate::html_tables::{Table, tables_of};
use crate::profile::ConvertedProfile;
use crate::reader::{ProfileReader, SourceReading};
use crate::source_note::SourceNote;

/// A calibration block with nothing set.
fn empty_block() -> MaterialPerPrinter {
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

/// Each wiki model the pages are for, against the printer we ship for it.
const MODELS: [(&str, Option<&str>); 3] = [
    ("halot-x1", Some("creality-halot-x1")),
    ("x1-neo", None),
    ("halot-sky-2025", None),
];

/// Reads Creality's wiki pages of resin settings.
#[derive(Clone, Copy, Debug, Default)]
pub struct CrealityWikiReader;

/// Everything that can stop this reader.
#[derive(Debug)]
pub enum CrealityWikiError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The page names no model the reader knows, in its `og:url`.
    UnknownPage { path: PathBuf },
    /// A resin's name cannot be an id.
    InvalidId {
        resin: String,
        source: sc_print_primitives::InvalidProfileId,
    },
}

impl fmt::Display for CrealityWikiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::UnknownPage { path } => write!(
                f,
                "{} is not a Creality wiki page of resin settings this reader knows",
                path.display()
            ),
            Self::InvalidId { resin, source } => write!(f, "{resin:?} cannot be named: {source}"),
        }
    }
}

impl std::error::Error for CrealityWikiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidId { source, .. } => Some(source),
            Self::UnknownPage { .. } => None,
        }
    }
}

/// The wiki model of a page, from its `og:url`.
fn model_of(html: &str) -> Option<&'static str> {
    let at = html.find("wiki.creality.com/en/halot-series/")?;
    let rest = &html[at + "wiki.creality.com/en/halot-series/".len()..];
    let model = rest.split('/').next()?;
    MODELS.iter().map(|(m, _)| *m).find(|m| *m == model)
}

/// A table's value for the setting whose name starts with any of `names`,
/// in `unit` when the name states it.
fn setting<'a>(table: &'a Table, names: &[&str]) -> Option<(&'a str, &'a str)> {
    table.rows.iter().find_map(|row| {
        let [name, value] = row.as_slice() else {
            return None;
        };
        let lower = name.to_ascii_lowercase();
        names
            .iter()
            .any(|n| lower.starts_with(n))
            .then_some((name.as_str(), value.as_str()))
    })
}

fn number(table: &Table, names: &[&str]) -> Option<f64> {
    setting(table, names).and_then(|(_, v)| v.trim().parse().ok())
}

/// A speed in mm/min, its row stating mm/min or mm/s.
fn speed(table: &Table, names: &[&str]) -> Option<PositiveFiniteMmPerMin> {
    let (name, value) = setting(table, names)?;
    let value: f64 = value.trim().parse().ok()?;
    let per_min = if name.contains("mm/s") {
        value * 60.0
    } else {
        value
    };
    PositiveFiniteMmPerMin::new(per_min).ok()
}

/// The calibration a resin's table states, or `None` when an exposure or
/// the lift is missing.
fn block_of(table: &Table) -> Option<MaterialPerPrinter> {
    let mut block = empty_block();
    block.normal_exposure_sec =
        Some(PositiveFiniteSec::new(number(table, &["exposure time", "print exposure"])?).ok()?);
    block.bottom_exposure_sec =
        Some(PositiveFiniteSec::new(number(table, &["initial exposure"])?).ok()?);
    block.bottom_layer_count = number(table, &["bottom exposure layers"])
        .filter(|n| *n >= 0.0 && n.fract() == 0.0)
        .map(|n| n as u32);
    block.lift_distance_mm =
        Some(PositiveFiniteMm::new(number(table, &["rising height", "lift height"])?).ok()?);
    let motor = speed(table, &["motor speed"])?;
    block.lift_speed_mm_min = Some(motor);
    block.retract_speed_mm_min = Some(motor);
    block.wait_before_cure_sec = number(table, &["light off delay", "light-off delay"])
        .and_then(|v| NonNegativeFiniteSec::new(v).ok());
    // The cushion (buffer) layers are the bottom layers' travel.
    block.bottom_lift_distance_mm = number(table, &["cushion layer height", "buffer layer height"])
        .and_then(|v| PositiveFiniteMm::new(v).ok());
    let cushion = speed(
        table,
        &["cushion layer motor speed", "buffer layer motor speed"],
    );
    block.bottom_lift_speed_mm_min = cushion;
    block.bottom_retract_speed_mm_min = cushion;
    block.bottom_wait_before_cure_sec = number(
        table,
        &["cushion layer delay", "buffer layer light-off delay"],
    )
    .and_then(|v| NonNegativeFiniteSec::new(v).ok());
    Some(block)
}

/// `Creality-HALOT Rigid Resin 2.0-Gray` as its brand and its resin.
fn brand_and_resin(heading: &str) -> Option<(String, String)> {
    let heading = heading.trim_start_matches(|c: char| !c.is_alphanumeric());
    let (brand, resin) = heading.split_once('-')?;
    let (brand, resin) = (brand.trim(), resin.trim());
    (!brand.is_empty() && !resin.is_empty()).then(|| (brand.to_owned(), resin.to_owned()))
}

impl ProfileReader for CrealityWikiReader {
    type Error = CrealityWikiError;

    fn read(&self, inputs: &[&Path]) -> Result<SourceReading, Self::Error> {
        let mut resins: BTreeMap<MaterialId, MaterialProfile> = BTreeMap::new();
        let mut notes = Vec::new();
        for path in inputs {
            let html = std::fs::read_to_string(path).map_err(|source| CrealityWikiError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let model = model_of(&html).ok_or_else(|| CrealityWikiError::UnknownPage {
                path: path.to_path_buf(),
            })?;
            let tables = tables_of(&html);
            let ours = MODELS
                .iter()
                .find(|(m, _)| *m == model)
                .and_then(|(_, p)| *p);
            let Some(printer) = ours else {
                notes.push(SourceNote::RowsForAMachineWeDoNotShip {
                    model: model.to_owned(),
                    rows: tables.len(),
                });
                continue;
            };
            let printer = PrinterId::new(printer).expect("the map holds only ids we ship");
            for table in &tables {
                let (Some((brand, resin)), Some(block), Some(height)) = (
                    brand_and_resin(&table.heading),
                    block_of(table),
                    number(table, &["layer thickness", "layer height"])
                        .and_then(|h| PositiveFiniteMm::new(h).ok()),
                ) else {
                    notes.push(SourceNote::RowsWeCannotRead {
                        model: model.to_owned(),
                        rows: 1,
                    });
                    continue;
                };
                let id_text = format!("{}-{}-{}um", slug(&brand), slug(&resin), microns(height));
                let id =
                    MaterialId::new(&id_text).map_err(|source| CrealityWikiError::InvalidId {
                        resin: resin.clone(),
                        source,
                    })?;
                let profile = resins.entry(id.clone()).or_insert_with(|| MaterialProfile {
                    extension: false,
                    identity: MaterialIdentity {
                        id,
                        brand: NonEmptyString::new(&brand).expect("not empty"),
                        family: NonEmptyString::new(family_of(&resin)).expect("a constant"),
                        name: NonEmptyString::new(&format!("{resin} ({} um)", microns(height)))
                            .expect("not empty"),
                        profile_version: Version::new(1, 0, 0),
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
                profile
                    .per_printer
                    .insert(CalibrationTarget::from_printer(&printer), block);
            }
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

/// The library's family of a resin, in the bundle's words.
fn family_of(resin: &str) -> &'static str {
    let resin = resin.to_ascii_lowercase();
    if resin.contains("abs") {
        "abs-like"
    } else if resin.contains("water washable") {
        "water-washable"
    } else {
        "standard"
    }
}

#[cfg(test)]
mod creality_wiki_tests {
    use super::*;

    fn page(model: &str, body: &str) -> String {
        format!(
            "<html><head><meta property=\"og:url\" \
             content=\"https://wiki.creality.com/en/halot-series/{model}/resin-parameters\"></head>\
             <body>{body}</body></html>"
        )
    }

    const X1_TABLE: &str = "<h2>\u{b6} Creality-HALOT Rigid Resin 2.0-Gray</h2><table>\
        <tr><th>Parameter</th><th>Value</th></tr>\
        <tr><td>Layer Thickness\u{ff08}mm\u{ff09}</td><td>0.05</td></tr>\
        <tr><td>Initial Exposure\u{ff08}s\u{ff09}</td><td>30</td></tr>\
        <tr><td>Exposure Time\u{ff08}s\u{ff09}</td><td>1.8</td></tr>\
        <tr><td>Rising Height\u{ff08}mm\u{ff09}</td><td>8</td></tr>\
        <tr><td>Motor Speed\u{ff08}mm/min\u{ff09}</td><td>180</td></tr>\
        <tr><td>Light off delay\u{ff08}s\u{ff09}</td><td>4</td></tr>\
        <tr><td>Bottom Exposure Layers</td><td>4</td></tr>\
        <tr><td>Cushion layer height\u{ff08}mm\u{ff09}</td><td>5</td></tr>\
        <tr><td>Cushion layer Motor Speed \u{ff08}mm/s\u{ff09}</td><td>2</td></tr>\
        <tr><td>Cushion layer Delay\u{ff08}s\u{ff09}</td><td>6</td></tr></table>";

    fn read(pages: &[(&str, String)]) -> SourceReading {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = pages
            .iter()
            .map(|(name, html)| {
                let path = dir.path().join(name);
                std::fs::write(&path, html).unwrap();
                path
            })
            .collect();
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        CrealityWikiReader.read(&refs).unwrap()
    }

    #[test]
    fn a_resin_table_becomes_a_material_for_the_page_s_printer() {
        let reading = read(&[("x1.html", page("halot-x1", X1_TABLE))]);
        assert!(reading.notes.is_empty());
        let [ConvertedProfile::Material(profile)] = reading.profiles.as_slice() else {
            panic!("one material");
        };
        assert_eq!(
            profile.identity.id.as_str(),
            "creality-halot-rigid-resin-2-0-gray-50um"
        );
        assert_eq!(
            profile.identity.name.as_str(),
            "HALOT Rigid Resin 2.0-Gray (50 um)"
        );
        let printer = PrinterId::new("creality-halot-x1").unwrap();
        let b = profile.per_printer[&CalibrationTarget::from_printer(&printer)];
        assert_eq!(b.normal_exposure_sec.unwrap().value(), 1.8);
        assert_eq!(b.bottom_exposure_sec.unwrap().value(), 30.0);
        assert_eq!(b.bottom_layer_count, Some(4));
        assert_eq!(b.lift_distance_mm.unwrap().value(), 8.0);
        assert_eq!(b.lift_speed_mm_min.unwrap().value(), 180.0);
        assert_eq!(b.wait_before_cure_sec.unwrap().value(), 4.0);
        assert_eq!(b.bottom_lift_distance_mm.unwrap().value(), 5.0);
        // The cushion's speed is in mm/s on the page.
        assert_eq!(b.bottom_lift_speed_mm_min.unwrap().value(), 120.0);
        assert_eq!(b.bottom_wait_before_cure_sec.unwrap().value(), 6.0);
    }

    #[test]
    fn a_page_for_a_printer_we_do_not_ship_is_noted() {
        let reading = read(&[("neo.html", page("x1-neo", X1_TABLE))]);
        assert!(reading.profiles.is_empty());
        assert_eq!(reading.notes.len(), 1);
    }

    #[test]
    fn a_page_of_another_kind_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.html");
        std::fs::write(&path, "<html></html>").unwrap();
        assert!(matches!(
            CrealityWikiReader.read(&[path.as_path()]),
            Err(CrealityWikiError::UnknownPage { .. })
        ));
    }
}
