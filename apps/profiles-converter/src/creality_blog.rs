/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Creality's published resin settings, as material profiles.
//!
//! The vendor publishes one table per resin in a post on its forum's
//! blog, with a column per machine and a row per setting. The input is
//! the post saved as HTML:
//! <https://forum.creality.com/t/creality-resin-3d-printers-settings-except-mage-series/10077>.
//!
//! The post states no bottom layer count, so no block carries one.

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

use crate::anycubic_wiki::{Cell, quantity};
use crate::elegoo_settings::machines::Machines;
use crate::elegoo_settings::{microns, slug};
use crate::html_tables::{Table, tables_of};
use crate::profile::ConvertedProfile;
use crate::reader::{ProfileReader, SourceReading};
use crate::source_note::SourceNote;

/// The vendor that publishes the post, spelled as the bundle spells it.
const BRAND: &str = "Creality";

/// The first cell of every settings table, above the setting names.
const CORNER: &str = "Recommended Settings";

/// Every machine the post publishes a column for, against the printer
/// we ship for it.
const MODELS: [(&str, &[&str]); 7] = [
    ("HALOT-ONE", &["creality-halot-one"]),
    ("HALOT-ONE PLUS", &["creality-halot-one-plus"]),
    ("HALOT-ONE PRO", &["creality-halot-one-pro"]),
    ("HALOT-MAX", &[]),
    ("HALOT-LITE", &[]),
    ("HALOT-SKY", &[]),
    ("LD-006", &[]),
];

/// Which printers of ours a column's machine covers.
fn machines_named_by(model: &str) -> Machines {
    let model = model.trim();
    match MODELS.iter().find(|(name, _)| *name == model) {
        Some((_, [])) => Machines::NotOurs,
        Some((_, ids)) => Machines::Ours(ids),
        None => Machines::Unknown,
    }
}

/// Reads Creality's resin settings post.
#[derive(Clone, Copy, Debug, Default)]
pub struct CrealityBlogReader;

/// Everything that can stop this reader.
#[derive(Debug)]
pub enum CrealityBlogError {
    /// The post could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The post holds no settings table, so it is not the one this
    /// reader reads or its shape has changed.
    NoTables { path: PathBuf },
    /// A table states a setting this reader does not know.
    UnknownSetting { resin: String, setting: String },
    /// A resin's name cannot be an id.
    InvalidId {
        resin: String,
        source: sc_print_primitives::InvalidProfileId,
    },
}

impl fmt::Display for CrealityBlogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::NoTables { path } => write!(f, "{} holds no settings table", path.display()),
            Self::UnknownSetting { resin, setting } => {
                write!(
                    f,
                    "{resin:?} states {setting:?}, which this reader does not know"
                )
            }
            Self::InvalidId { resin, source } => write!(f, "{resin:?} cannot be named: {source}"),
        }
    }
}

impl std::error::Error for CrealityBlogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidId { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl ProfileReader for CrealityBlogReader {
    type Error = CrealityBlogError;

    fn read(&self, inputs: &[&Path]) -> Result<SourceReading, Self::Error> {
        let mut run = Run::default();
        for path in inputs {
            let html = std::fs::read_to_string(path).map_err(|source| CrealityBlogError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            // The forum lists other topics in tables of its own under the
            // post, so only tables opening with the settings corner count.
            let tables: Vec<Table> = tables_of(&html)
                .into_iter()
                .filter(|table| {
                    table
                        .rows
                        .first()
                        .and_then(|row| row.first())
                        .is_some_and(|corner| corner == CORNER)
                })
                .collect();
            if tables.is_empty() {
                return Err(CrealityBlogError::NoTables {
                    path: path.to_path_buf(),
                });
            }
            for table in &tables {
                run.absorb(table)?;
            }
        }
        Ok(run.finish())
    }
}

/// A setting a table states, one per row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    LayerThickness,
    InitialExposure,
    Exposure,
    TurnOffDelay,
    RisingHeight,
    MotorSpeed,
    RetractSpeed,
}

impl Setting {
    fn of(label: &str) -> Option<Self> {
        Some(match label {
            "Layer thickness" => Self::LayerThickness,
            "Initial exposure" => Self::InitialExposure,
            "Exposure time" => Self::Exposure,
            "Turn off delay" => Self::TurnOffDelay,
            "Rising height" => Self::RisingHeight,
            "Motor speed" => Self::MotorSpeed,
            "Retract speed" => Self::RetractSpeed,
            _ => return None,
        })
    }
}

/// One resin at one layer height, and the machines it is calibrated on.
struct Resin {
    identity: MaterialIdentity,
    height: PositiveFiniteMm,
    blocks: BTreeMap<CalibrationTarget, MaterialPerPrinter>,
}

/// A reason columns were passed over, counted per machine.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Skipped {
    MachineNotShipped(String),
    ModelNotRecognised(String),
    Unreadable(String),
}

#[derive(Default)]
struct Run {
    resins: BTreeMap<MaterialId, Resin>,
    skipped: BTreeMap<Skipped, usize>,
}

impl Run {
    /// Read every machine column of `table`, the settings of one resin.
    fn absorb(&mut self, table: &Table) -> Result<(), CrealityBlogError> {
        let resin = table.heading.trim();
        let Some((machines, rows)) = table.rows.split_first() else {
            return Ok(());
        };
        let mut settings = Vec::with_capacity(rows.len());
        for row in rows {
            let label = row.first().map(String::as_str).unwrap_or_default();
            let setting = Setting::of(label).ok_or_else(|| CrealityBlogError::UnknownSetting {
                resin: resin.to_owned(),
                setting: label.to_owned(),
            })?;
            settings.push((setting, row));
        }
        let cell = |setting: Setting, column: usize| {
            settings
                .iter()
                .find(|(s, _)| *s == setting)
                .and_then(|(_, row)| row.get(column))
                .map(String::as_str)
                .unwrap_or_default()
        };

        for (column, model) in machines.iter().enumerate().skip(1) {
            let printers = match machines_named_by(model) {
                Machines::Ours(ids) => ids,
                Machines::NotOurs => {
                    self.skip(Skipped::MachineNotShipped(model.clone()));
                    continue;
                }
                Machines::Unknown => {
                    self.skip(Skipped::ModelNotRecognised(model.clone()));
                    continue;
                }
            };
            let height = match quantity(cell(Setting::LayerThickness, column), "mm", true) {
                Cell::Value(height) => PositiveFiniteMm::new(height).ok(),
                Cell::Absent | Cell::Unreadable => None,
            };
            let block = block_of(|setting| cell(setting, column));
            let (Some(height), Some(block)) = (height, block) else {
                self.skip(Skipped::Unreadable(model.clone()));
                continue;
            };

            let id = material_id_of(resin, height)?;
            let entry = self.resins.entry(id.clone()).or_insert_with(|| Resin {
                identity: identity_of(&id, resin, height),
                height,
                blocks: BTreeMap::new(),
            });
            for printer in printers {
                let printer = PrinterId::new(printer).expect("the map holds only ids we ship");
                entry
                    .blocks
                    .insert(CalibrationTarget::from_printer(&printer), block);
            }
        }
        Ok(())
    }

    fn skip(&mut self, reason: Skipped) {
        *self.skipped.entry(reason).or_default() += 1;
    }

    /// Turn what was read into profiles, and what was not into notes.
    fn finish(self) -> SourceReading {
        let notes = self
            .skipped
            .into_iter()
            .map(|(reason, rows)| match reason {
                Skipped::MachineNotShipped(model) => {
                    SourceNote::RowsForAMachineWeDoNotShip { model, rows }
                }
                Skipped::ModelNotRecognised(model) => {
                    SourceNote::RowsForAnUnknownModel { model, rows }
                }
                Skipped::Unreadable(model) => SourceNote::RowsWeCannotRead { model, rows },
            })
            .collect();
        let profiles = self
            .resins
            .into_values()
            .map(|resin| {
                ConvertedProfile::Material(MaterialProfile {
                    extension: false,
                    identity: resin.identity,
                    intent: MaterialIntent {
                        target_energy_density_mj_per_cm2: None,
                        target_layer_height_mm: Some(resin.height),
                    },
                    defaults: MaterialDefaults::default(),
                    physical: MaterialPhysical::default(),
                    per_printer: resin.blocks,
                    notes: BTreeMap::new(),
                })
            })
            .collect();
        SourceReading { profiles, notes }
    }
}

/// The calibration one column states, or nothing when a setting every
/// block needs is missing or in a unit this reader does not know.
fn block_of<'a>(at: impl Fn(Setting) -> &'a str) -> Option<MaterialPerPrinter> {
    let value = |setting, unit| match quantity(at(setting), unit, true) {
        Cell::Value(value) => Some(value),
        Cell::Absent | Cell::Unreadable => None,
    };
    let wait = match quantity(at(Setting::TurnOffDelay), "s", true) {
        Cell::Value(value) => Some(NonNegativeFiniteSec::new(value).ok()?),
        Cell::Absent => None,
        Cell::Unreadable => return None,
    };
    Some(MaterialPerPrinter {
        normal_exposure_sec: Some(PositiveFiniteSec::new(value(Setting::Exposure, "s")?).ok()?),
        bottom_exposure_sec: Some(
            PositiveFiniteSec::new(value(Setting::InitialExposure, "s")?).ok()?,
        ),
        bottom_layer_count: None,
        wait_before_cure_sec: wait,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: None,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: Some(PositiveFiniteMm::new(value(Setting::RisingHeight, "mm")?).ok()?),
        lift_speed_mm_min: Some(speed(at(Setting::MotorSpeed))?),
        retract_speed_mm_min: Some(speed(at(Setting::RetractSpeed))?),
        bottom_lift_distance_mm: None,
        bottom_lift_speed_mm_min: None,
        bottom_retract_speed_mm_min: None,
        light_pwm: None,
        bottom_light_pwm: None,
        second_travel: None,
        bottom_second_travel: None,
    })
}

/// A speed in mm/min, which the post states in mm/s for most machines
/// and in mm/min for the LD-006.
fn speed(cell: &str) -> Option<PositiveFiniteMmPerMin> {
    let per_min = match quantity(cell, "mm/min", false) {
        Cell::Value(value) => value,
        Cell::Absent => return None,
        Cell::Unreadable => match quantity(cell, "mm/s", false) {
            Cell::Value(value) => value * 60.0,
            Cell::Absent | Cell::Unreadable => return None,
        },
    };
    PositiveFiniteMmPerMin::new(per_min).ok()
}

/// Name a resin after the product and the layer height its exposures
/// were measured at.
fn material_id_of(resin: &str, height: PositiveFiniteMm) -> Result<MaterialId, CrealityBlogError> {
    let id = format!("{}-{}-{}um", slug(BRAND), slug(resin), microns(height));
    MaterialId::new(&id).map_err(|source| CrealityBlogError::InvalidId {
        resin: resin.to_owned(),
        source,
    })
}

fn identity_of(id: &MaterialId, resin: &str, height: PositiveFiniteMm) -> MaterialIdentity {
    let name = format!("{resin} ({} um)", microns(height));
    MaterialIdentity {
        id: id.clone(),
        brand: NonEmptyString::new(BRAND).expect("a constant that is not empty"),
        family: NonEmptyString::new(family_of(resin)).expect("a constant that is not empty"),
        name: NonEmptyString::new(&name).expect("a resin name is never empty"),
        profile_version: Version::new(1, 0, 0),
    }
}

/// Which of the library's families a resin belongs to, in the
/// vocabulary the bundle already uses.
fn family_of(resin: &str) -> &'static str {
    let resin = resin.to_ascii_lowercase();
    if resin.contains("abs") {
        "abs-like"
    } else if resin.contains("tough") {
        "tough"
    } else if resin.contains("castable") {
        "other"
    } else if resin.contains("dental") || resin.contains("high temperature") {
        "engineering"
    } else {
        "standard"
    }
}

#[cfg(test)]
mod creality_blog_tests {
    use super::*;

    /// The post's first table, as published.
    fn pla_based() -> Table {
        let rows: [[&str; 8]; 8] = [
            [
                "Recommended Settings",
                "HALOT-ONE PRO",
                "HALOT-ONE PLUS",
                "HALOT-MAX",
                "HALOT-LITE",
                "HALOT-SKY",
                "HALOT-ONE",
                "LD-006",
            ],
            [
                "Layer thickness",
                "0.05mm",
                "0.05mm",
                "0.05mm",
                "0.05mm",
                "0.05mm",
                "0.05mm",
                "0.05mm",
            ],
            [
                "Initial exposure",
                "20s",
                "20s",
                "50s",
                "60s",
                "60s",
                "70s",
                "30s",
            ],
            [
                "Exposure time",
                "2.5s",
                "2.8s",
                "2.5s",
                "2.8s",
                "2.8s",
                "3.2s",
                "2.4s",
            ],
            ["Turn off delay", "1s", "1s", "1s", "2s", "2s", "2s", "0s"],
            [
                "Rising height",
                "7mm",
                "7mm",
                "7mm",
                "7mm",
                "7mm",
                "7mm",
                "5mm",
            ],
            [
                "Motor speed",
                "3mm/s",
                "3mm/s",
                "1mm/s",
                "5mm/s",
                "5mm/s",
                "5mm/s",
                "60mm/min",
            ],
            [
                "Retract speed",
                "3mm/s",
                "3mm/s",
                "1mm/s",
                "5mm/s",
                "5mm/s",
                "5mm/s",
                "150mm/min",
            ],
        ];
        Table {
            heading: "PLA-Based UV Curable Resin".to_owned(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
                .collect(),
        }
    }

    fn read(tables: &[Table]) -> SourceReading {
        let mut run = Run::default();
        for table in tables {
            run.absorb(table).unwrap();
        }
        run.finish()
    }

    fn only_material(reading: &SourceReading) -> &MaterialProfile {
        match reading.profiles.as_slice() {
            [ConvertedProfile::Material(material)] => material,
            other => panic!("expected exactly 1 resin, got {}", other.len()),
        }
    }

    fn block(profile: &MaterialProfile, printer: &str) -> MaterialPerPrinter {
        let id = PrinterId::new(printer).unwrap();
        profile.per_printer[&CalibrationTarget::from_printer(&id)]
    }

    #[test]
    fn a_table_becomes_a_resin_named_for_its_heading_and_layer() {
        let reading = read(&[pla_based()]);

        let profile = only_material(&reading);
        assert_eq!(
            profile.identity.id.as_str(),
            "creality-pla-based-uv-curable-resin-50um"
        );
        assert_eq!(profile.identity.brand.as_str(), "Creality");
        assert_eq!(
            profile.identity.name.as_str(),
            "PLA-Based UV Curable Resin (50 um)"
        );
    }

    #[test]
    fn each_halot_one_column_calibrates_its_own_machine() {
        let reading = read(&[pla_based()]);

        let profile = only_material(&reading);
        assert_eq!(profile.per_printer.len(), 3);
        let one = block(profile, "creality-halot-one");
        assert_eq!(one.normal_exposure_sec, PositiveFiniteSec::new(3.2).ok());
        assert_eq!(one.bottom_exposure_sec, PositiveFiniteSec::new(70.0).ok());
        assert_eq!(
            one.wait_before_cure_sec,
            NonNegativeFiniteSec::new(2.0).ok()
        );
        assert_eq!(one.lift_distance_mm, PositiveFiniteMm::new(7.0).ok());
        assert_eq!(
            one.lift_speed_mm_min,
            PositiveFiniteMmPerMin::new(300.0).ok()
        );
        assert_eq!(
            one.retract_speed_mm_min,
            PositiveFiniteMmPerMin::new(300.0).ok()
        );
        assert_eq!(one.bottom_layer_count, None);
        assert_eq!(
            block(profile, "creality-halot-one-pro").normal_exposure_sec,
            PositiveFiniteSec::new(2.5).ok(),
        );
    }

    #[test]
    fn machines_we_do_not_ship_are_counted_per_machine() {
        let reading = read(&[pla_based(), pla_based()]);

        assert!(
            reading
                .notes
                .contains(&SourceNote::RowsForAMachineWeDoNotShip {
                    model: "LD-006".to_owned(),
                    rows: 2,
                })
        );
        assert_eq!(reading.notes.len(), 4);
    }

    #[test]
    fn a_speed_in_mm_per_min_is_taken_as_it_is() {
        assert_eq!(speed("60mm/min"), PositiveFiniteMmPerMin::new(60.0).ok());
        assert_eq!(speed("3mm/s"), PositiveFiniteMmPerMin::new(180.0).ok());
        assert_eq!(speed("3mm"), None);
    }

    #[test]
    fn a_setting_this_reader_does_not_know_stops_the_read() {
        let mut unknown = pla_based();
        unknown.rows[2][0] = "Bottom layers".to_owned();
        let mut run = Run::default();

        let err = run.absorb(&unknown).unwrap_err();
        assert!(
            matches!(err, CrealityBlogError::UnknownSetting { .. }),
            "{err}"
        );
    }

    #[test]
    fn a_resin_sorts_into_the_family_its_kind_belongs_to() {
        assert_eq!(family_of("ABS Like Resin"), "abs-like");
        assert_eq!(family_of("Toughness Resin"), "tough");
        assert_eq!(family_of("Dental Castable Resin"), "other");
        assert_eq!(family_of("High temperature Resin"), "engineering");
        assert_eq!(family_of("Water washable resin Plus"), "standard");
    }
}
