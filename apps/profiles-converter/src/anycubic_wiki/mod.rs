/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Anycubic's published resin settings, as material profiles.
//!
//! The vendor publishes one table per machine on its wiki, with a
//! column per resin and a row per setting. A column is a calibration,
//! not a resin, so the columns of one resin converge on one profile with
//! a block per machine. The input is the wiki page saved as HTML:
//! <https://wiki.anycubic.com/en/filament-and-resin/resin-settings>.
//!
//! The page states every quantity with its unit, and this reader holds
//! it to that unit rather than guessing: a speed written as `8mm` is
//! reported, not read as 8 mm/s.

pub mod machines;

use std::collections::{BTreeMap, BTreeSet};
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

use crate::anycubic_wiki::machines::{Machines, machines_named_by};
use crate::elegoo_settings::{microns, slug};
use crate::html_tables::{Table, tables_of};
use crate::profile::ConvertedProfile;
use crate::reader::{ProfileReader, SourceReading};
use crate::source_note::SourceNote;

/// The vendor that publishes the page, spelled as the bundle spells it.
const BRAND: &str = "Anycubic";

/// What every table's heading opens with, before the machine it names.
const HEADING: &str = "Printing Settings for ";

/// The heading's suffix for a machine's standard table, when the wiki
/// publishes a faster mode beside it.
const STANDARD_MODE: &str = "Standard Speed Setting";

/// The layer the wiki publishes its standard tables at. A standard
/// table without a layer row is read at it: every table that names a
/// layer names this one for every resin but the high speed ones.
const STANDARD_LAYER_MM: f64 = 0.05;

/// Spellings the wiki gives one resin, against the one a profile takes.
const SPELLINGS: [(&str, &str); 4] = [
    ("Water Wash Resin+", "Water-Wash Resin+"),
    ("DLP Craftman Resin", "DLP Craftsman Resin"),
    ("ABS-Like Resin +", "ABS-Like Resin+"),
    ("Plant-based Resin", "Plant-Based Resin"),
];

/// Reads Anycubic's resin settings page.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnycubicWikiReader;

/// Everything that can stop this reader.
#[derive(Debug)]
pub enum AnycubicWikiError {
    /// The page could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The page holds no settings table, so it is not the one this
    /// reader reads or its shape has changed.
    NoTables { path: PathBuf },
    /// A table sits under a heading this reader does not recognise.
    UnexpectedHeading { heading: String },
    /// A table states a setting this reader does not know.
    UnknownSetting { heading: String, setting: String },
    /// A resin's name cannot be an id.
    InvalidId {
        resin: String,
        source: sc_print_primitives::InvalidProfileId,
    },
}

impl fmt::Display for AnycubicWikiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::NoTables { path } => write!(f, "{} holds no settings table", path.display()),
            Self::UnexpectedHeading { heading } => {
                write!(f, "a table sits under {heading:?}, which names no machine")
            }
            Self::UnknownSetting { heading, setting } => {
                write!(
                    f,
                    "{heading:?} states {setting:?}, which this reader does not know"
                )
            }
            Self::InvalidId { resin, source } => write!(f, "{resin:?} cannot be named: {source}"),
        }
    }
}

impl std::error::Error for AnycubicWikiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidId { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl ProfileReader for AnycubicWikiReader {
    type Error = AnycubicWikiError;

    fn read(&self, inputs: &[&Path]) -> Result<SourceReading, Self::Error> {
        let mut run = Run::default();
        for path in inputs {
            let html = std::fs::read_to_string(path).map_err(|source| AnycubicWikiError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let tables = tables_of(&html);
            if tables.is_empty() {
                return Err(AnycubicWikiError::NoTables {
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
    Exposure,
    LightOff,
    BottomExposure,
    BottomLayers,
    LiftDistance,
    LiftSpeed,
    RetractSpeed,
    /// A slicing setting rather than a resin one, so no profile
    /// carries it.
    AntiAliasing,
}

impl Setting {
    fn of(label: &str) -> Option<Self> {
        Some(match label {
            "Layer Thickness" => Self::LayerThickness,
            "Exposure Time" => Self::Exposure,
            "Light-Off Time" => Self::LightOff,
            "Bottom Exposure Time" => Self::BottomExposure,
            "Bottom Layer" => Self::BottomLayers,
            "Z Lift Distance" => Self::LiftDistance,
            "Z Lift Speed" => Self::LiftSpeed,
            "Z Retract Speed" => Self::RetractSpeed,
            "Anti-Aliasing Level" => Self::AntiAliasing,
            _ => return None,
        })
    }
}

/// What a cell says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Cell {
    /// Empty, or the `/` the wiki writes for a resin it does not
    /// publish on that machine.
    Absent,
    Value(f64),
    /// A value in a unit other than the row's.
    Unreadable,
}

/// Read `cell` as a number in `unit`, which may be left off only where
/// `bare` allows it.
pub(crate) fn quantity(cell: &str, unit: &str, bare: bool) -> Cell {
    let text = cell.trim();
    if text.is_empty() || text == "/" {
        return Cell::Absent;
    }
    let number = match text.strip_suffix(unit) {
        Some(number) => number.trim(),
        None if bare => text,
        None => return Cell::Unreadable,
    };
    match number.parse::<f64>() {
        Ok(value) if value.is_finite() => Cell::Value(value),
        _ => Cell::Unreadable,
    }
}

/// One resin at one layer height and speed mode, and the machines it is
/// calibrated on.
struct Resin {
    identity: MaterialIdentity,
    height: PositiveFiniteMm,
    blocks: BTreeMap<CalibrationTarget, Slot>,
}

/// What is known about one machine's calibration for one resin.
enum Slot {
    Agreed(Box<MaterialPerPrinter>),
    /// Tables disagreed, so the resin carries no block for the machine.
    Disagreed,
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
    conflicts: Vec<(MaterialId, PrinterId)>,
    spellings: BTreeSet<(MaterialId, String, String)>,
}

impl Run {
    /// Read every resin column of `table`.
    fn absorb(&mut self, table: &Table) -> Result<(), AnycubicWikiError> {
        let Some(named) = table.heading.strip_prefix(HEADING) else {
            return Err(AnycubicWikiError::UnexpectedHeading {
                heading: table.heading.clone(),
            });
        };
        let (model, fast) = model_and_mode(named);
        let Some((resins, rows)) = table.rows.split_first() else {
            return Ok(());
        };
        let mut settings = Vec::with_capacity(rows.len());
        for row in rows {
            let label = row.first().map(String::as_str).unwrap_or_default();
            let setting = Setting::of(label).ok_or_else(|| AnycubicWikiError::UnknownSetting {
                heading: table.heading.clone(),
                setting: label.to_owned(),
            })?;
            settings.push((setting, row));
        }
        let cell = |setting: Setting, column: usize| {
            settings
                .iter()
                .find(|(s, _)| *s == setting)
                .map(|(_, row)| row.get(column).map(String::as_str).unwrap_or_default())
        };

        for (column, resin) in resins.iter().enumerate().skip(1) {
            if quantity(
                cell(Setting::Exposure, column).unwrap_or_default(),
                "s",
                true,
            ) == Cell::Absent
            {
                continue; // a resin the wiki does not publish on this machine
            }
            let printers = match machines_named_by(model) {
                Machines::Ours(ids) => ids,
                Machines::NotOurs => {
                    self.skip(Skipped::MachineNotShipped(model.to_owned()));
                    continue;
                }
                Machines::Unknown => {
                    self.skip(Skipped::ModelNotRecognised(model.to_owned()));
                    continue;
                }
            };
            let height =
                match cell(Setting::LayerThickness, column).map(|c| quantity(c, "mm", true)) {
                    Some(Cell::Value(height)) => mm(height),
                    None | Some(Cell::Absent) if !fast => mm(STANDARD_LAYER_MM),
                    _ => None,
                };
            let block = block_of(|setting, unit, bare| {
                cell(setting, column).map_or(Cell::Absent, |c| quantity(c, unit, bare))
            });
            let (Some(height), Some(block)) = (height, block) else {
                self.skip(Skipped::Unreadable(model.to_owned()));
                continue;
            };

            let resin = resin.trim();
            let name = SPELLINGS
                .iter()
                .find(|(spelled, _)| *spelled == resin)
                .map_or(resin, |(_, kept)| *kept);
            let id = material_id_of(name, height, fast)?;
            if name != resin {
                self.spellings
                    .insert((id.clone(), name.to_owned(), resin.to_owned()));
            }
            let entry = self.resins.entry(id.clone()).or_insert_with(|| Resin {
                identity: identity_of(&id, name, height, fast),
                height,
                blocks: BTreeMap::new(),
            });
            for printer in printers {
                let printer = PrinterId::new(printer).expect("the map holds only ids we ship");
                let target = CalibrationTarget::from_printer(&printer);
                match entry.blocks.get(&target) {
                    Some(Slot::Agreed(seen)) if **seen != block => {
                        entry.blocks.insert(target, Slot::Disagreed);
                        self.conflicts.push((id.clone(), printer));
                    }
                    Some(_) => {}
                    None => {
                        entry.blocks.insert(target, Slot::Agreed(Box::new(block)));
                    }
                }
            }
        }
        Ok(())
    }

    fn skip(&mut self, reason: Skipped) {
        *self.skipped.entry(reason).or_default() += 1;
    }

    /// Turn what was read into profiles, and what was not into notes.
    fn finish(self) -> SourceReading {
        let mut notes: Vec<SourceNote> = self
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
        notes.extend(
            self.spellings
                .into_iter()
                .map(
                    |(material, kept, passed_over)| SourceNote::NameSpelledDifferently {
                        material,
                        kept,
                        passed_over,
                    },
                ),
        );
        notes.extend(
            self.conflicts
                .into_iter()
                .map(|(material, printer)| SourceNote::RowsDisagree { material, printer }),
        );
        let profiles = self
            .resins
            .into_values()
            .filter_map(|resin| {
                let per_printer: BTreeMap<CalibrationTarget, MaterialPerPrinter> = resin
                    .blocks
                    .into_iter()
                    .filter_map(|(target, slot)| match slot {
                        Slot::Agreed(block) => Some((target, *block)),
                        Slot::Disagreed => None,
                    })
                    .collect();
                (!per_printer.is_empty()).then(|| {
                    ConvertedProfile::Material(MaterialProfile {
                        extension: false,
                        identity: resin.identity,
                        intent: MaterialIntent {
                            target_energy_density_mj_per_cm2: None,
                            target_layer_height_mm: Some(resin.height),
                        },
                        defaults: MaterialDefaults::default(),
                        physical: MaterialPhysical::default(),
                        per_printer,
                        notes: BTreeMap::new(),
                    })
                })
            })
            .collect();
        SourceReading { profiles, notes }
    }
}

/// The machine a heading names, and whether its table is a speed mode
/// published beside the standard one.
fn model_and_mode(named: &str) -> (&str, bool) {
    match named.split_once(" (") {
        Some((model, mode)) => (model.trim(), mode.trim_end_matches(')') != STANDARD_MODE),
        None => (named.trim(), false),
    }
}

/// The calibration one column states, or nothing when a setting every
/// block needs is missing or in the wrong unit.
///
/// The wiki publishes one travel for the whole lift, the same for the
/// bottom layers, so no block carries a second stage or a bottom travel
/// of its own.
fn block_of(at: impl Fn(Setting, &str, bool) -> Cell) -> Option<MaterialPerPrinter> {
    let value = |setting, unit, bare| match at(setting, unit, bare) {
        Cell::Value(value) => Some(value),
        Cell::Absent | Cell::Unreadable => None,
    };
    let light_off = match at(Setting::LightOff, "s", true) {
        Cell::Value(value) => Some(NonNegativeFiniteSec::new(value).ok()?),
        Cell::Absent => None,
        Cell::Unreadable => return None,
    };
    let bottom_layers = value(Setting::BottomLayers, "", true)?;
    Some(MaterialPerPrinter {
        normal_exposure_sec: Some(sec(value(Setting::Exposure, "s", true)?)?),
        bottom_exposure_sec: Some(sec(value(Setting::BottomExposure, "s", true)?)?),
        bottom_layer_count: Some(count(bottom_layers)?),
        wait_before_cure_sec: light_off,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: None,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: Some(mm(value(Setting::LiftDistance, "mm", true)?)?),
        lift_speed_mm_min: Some(per_min(value(Setting::LiftSpeed, "mm/s", false)?)?),
        retract_speed_mm_min: Some(per_min(value(Setting::RetractSpeed, "mm/s", false)?)?),
        bottom_lift_distance_mm: None,
        bottom_lift_speed_mm_min: None,
        bottom_retract_speed_mm_min: None,
        light_pwm: None,
        bottom_light_pwm: None,
        second_travel: None,
        bottom_second_travel: None,
    })
}

/// Name a resin after the product, the layer height its exposures were
/// measured at and, for a speed mode, that mode.
///
/// The wiki names no colour, so unlike the bundle's other Anycubic
/// profile the id carries none.
fn material_id_of(
    resin: &str,
    height: PositiveFiniteMm,
    fast: bool,
) -> Result<MaterialId, AnycubicWikiError> {
    let mut id = format!("{}-{}-{}um", slug(BRAND), slug(resin), microns(height));
    if fast {
        id.push_str("-high-speed");
    }
    MaterialId::new(&id).map_err(|source| AnycubicWikiError::InvalidId {
        resin: resin.to_owned(),
        source,
    })
}

fn identity_of(
    id: &MaterialId,
    resin: &str,
    height: PositiveFiniteMm,
    fast: bool,
) -> MaterialIdentity {
    let mode = if fast { ", high speed" } else { "" };
    let name = format!("{resin} ({} um{mode})", microns(height));
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
    if resin.contains("abs-like") {
        "abs-like"
    } else if resin.contains("tough") {
        "tough"
    } else {
        "standard"
    }
}

fn mm(value: f64) -> Option<PositiveFiniteMm> {
    PositiveFiniteMm::new(value).ok()
}

fn sec(value: f64) -> Option<PositiveFiniteSec> {
    PositiveFiniteSec::new(value).ok()
}

/// A speed the page gives in mm/s, as the mm/min a profile carries.
fn per_min(mm_per_sec: f64) -> Option<PositiveFiniteMmPerMin> {
    PositiveFiniteMmPerMin::new(mm_per_sec * 60.0).ok()
}

/// A layer count, which the page must state as a whole number.
fn count(value: f64) -> Option<u32> {
    (value.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&value)).then_some(value as u32)
}

#[cfg(test)]
mod anycubic_wiki_tests {
    use super::*;

    /// The M7 Pro table as the wiki publishes it, cut to 3 resins.
    fn m7_pro() -> Table {
        table(
            "Printing Settings for Photon Mono M7 Pro",
            &[
                &[
                    "Resin Type",
                    "Standard Resin",
                    "High Speed Resin",
                    "Water Wash Resin+",
                ],
                &["Layer Thickness", "0.05mm", "0.05mm", "0.05mm"],
                &["Exposure Time", "2s", "1.5s", "2s"],
                &["Light-Off Time", "0.5s", "0.5s", "0.5s"],
                &["Bottom Exposure Time", "25s", "15s", "25s"],
                &["Bottom Layer", "4", "5", "4"],
                &["Z Lift Distance", "8mm", "4mm", "8mm"],
                &["Z Lift Speed", "6mm/s", "15mm/s", "6mm/s"],
                &["Z Retract Speed", "6mm/s", "15mm/s", "6mm/s"],
                &["Anti-Aliasing Level", "1", "1", "1"],
            ],
        )
    }

    fn table(heading: &str, rows: &[&[&str]]) -> Table {
        Table {
            heading: heading.to_owned(),
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

    fn material<'a>(reading: &'a SourceReading, id: &str) -> &'a MaterialProfile {
        reading
            .profiles
            .iter()
            .find_map(|profile| match profile {
                ConvertedProfile::Material(m) if m.identity.id.as_str() == id => Some(m),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no profile {id}"))
    }

    fn block(profile: &MaterialProfile, printer: &str) -> MaterialPerPrinter {
        let id = PrinterId::new(printer).unwrap();
        profile.per_printer[&CalibrationTarget::from_printer(&id)]
    }

    #[test]
    fn a_column_becomes_a_resin_named_for_its_product_and_layer() {
        let reading = read(&[m7_pro()]);

        let profile = material(&reading, "anycubic-standard-resin-50um");
        assert_eq!(profile.identity.brand.as_str(), "Anycubic");
        assert_eq!(profile.identity.name.as_str(), "Standard Resin (50 um)");
        assert_eq!(profile.identity.family.as_str(), "standard");
        assert_eq!(
            profile.intent.target_layer_height_mm,
            Some(PositiveFiniteMm::new(0.05).unwrap()),
        );
    }

    #[test]
    fn every_setting_lands_in_the_machines_block_with_speeds_in_mm_per_min() {
        let reading = read(&[m7_pro()]);

        let block = block(
            material(&reading, "anycubic-standard-resin-50um"),
            "anycubic-photon-mono-m7-pro",
        );
        assert_eq!(block.normal_exposure_sec, PositiveFiniteSec::new(2.0).ok());
        assert_eq!(block.bottom_exposure_sec, PositiveFiniteSec::new(25.0).ok());
        assert_eq!(block.bottom_layer_count, Some(4));
        assert_eq!(
            block.wait_before_cure_sec,
            NonNegativeFiniteSec::new(0.5).ok()
        );
        assert_eq!(block.lift_distance_mm, PositiveFiniteMm::new(8.0).ok());
        assert_eq!(
            block.lift_speed_mm_min,
            PositiveFiniteMmPerMin::new(360.0).ok()
        );
        assert_eq!(
            block.retract_speed_mm_min,
            PositiveFiniteMmPerMin::new(360.0).ok()
        );
        assert_eq!(block.second_travel, None);
    }

    #[test]
    fn a_resin_spelled_2_ways_is_one_resin_and_the_spelling_is_reported() {
        let reading = read(&[m7_pro()]);

        material(&reading, "anycubic-water-wash-resin-plus-50um");
        assert!(reading.notes.contains(&SourceNote::NameSpelledDifferently {
            material: MaterialId::new("anycubic-water-wash-resin-plus-50um").unwrap(),
            kept: "Water-Wash Resin+".to_owned(),
            passed_over: "Water Wash Resin+".to_owned(),
        }));
    }

    #[test]
    fn a_speed_mode_becomes_its_own_resin() {
        let fast = table(
            "Printing Settings for Photon Mono M7 (Fast Printing Mode)",
            &[
                &["Resin Type", "Standard Resin"],
                &["Layer Thickness", "0.1mm"],
                &["Exposure Time", "2.2s"],
                &["Light-Off Time", "0.5s"],
                &["Bottom Exposure Time", "25s"],
                &["Bottom Layer", "3"],
                &["Z Lift Distance", "4mm"],
                &["Z Lift Speed", "15mm/s"],
                &["Z Retract Speed", "15mm/s"],
            ],
        );
        let reading = read(&[fast]);

        let profile = material(&reading, "anycubic-standard-resin-100um-high-speed");
        assert_eq!(
            profile.identity.name.as_str(),
            "Standard Resin (100 um, high speed)"
        );
    }

    #[test]
    fn a_standard_table_without_a_layer_row_is_read_at_the_standard_layer() {
        let mut no_layer = m7_pro();
        no_layer.heading =
            "Printing Settings for Photon Mono M7 Max (Standard Speed Setting)".into();
        no_layer.rows.remove(1);
        let reading = read(&[no_layer]);

        material(&reading, "anycubic-standard-resin-50um");
    }

    #[test]
    fn a_speed_mode_without_a_layer_row_is_reported_not_guessed() {
        let mut no_layer = m7_pro();
        no_layer.heading = "Printing Settings for Photon Mono M7 Max (High Speed Setting)".into();
        no_layer.rows.remove(1);
        let reading = read(&[no_layer]);

        assert!(reading.profiles.is_empty());
        assert_eq!(
            reading.notes,
            vec![SourceNote::RowsWeCannotRead {
                model: "Photon Mono M7 Max".to_owned(),
                rows: 3,
            }],
        );
    }

    #[test]
    fn a_speed_in_the_wrong_unit_is_reported_not_guessed() {
        let mut typo = m7_pro();
        typo.rows[7][1] = "8mm".to_owned();
        let reading = read(&[typo]);

        assert_eq!(reading.profiles.len(), 2);
        assert!(reading.notes.contains(&SourceNote::RowsWeCannotRead {
            model: "Photon Mono M7 Pro".to_owned(),
            rows: 1,
        }));
    }

    #[test]
    fn a_time_without_its_unit_is_still_a_time() {
        let mut bare = m7_pro();
        bare.rows[2][1] = "4".to_owned();
        let reading = read(&[bare]);

        let block = block(
            material(&reading, "anycubic-standard-resin-50um"),
            "anycubic-photon-mono-m7-pro",
        );
        assert_eq!(block.normal_exposure_sec, PositiveFiniteSec::new(4.0).ok());
    }

    #[test]
    fn a_resin_the_wiki_does_not_publish_on_a_machine_is_left_out_quietly() {
        let mut absent = m7_pro();
        for row in &mut absent.rows[1..] {
            row[3] = "/".to_owned();
        }
        let reading = read(&[absent]);

        assert_eq!(reading.profiles.len(), 2);
        assert_eq!(reading.notes, Vec::new());
    }

    #[test]
    fn a_machine_we_ship_nothing_for_is_counted() {
        let mut other = m7_pro();
        other.heading = "Printing Settings for Photon M3 Plus".into();
        let reading = read(&[other]);

        assert!(reading.profiles.is_empty());
        assert_eq!(
            reading.notes,
            vec![SourceNote::RowsForAMachineWeDoNotShip {
                model: "Photon M3 Plus".to_owned(),
                rows: 3,
            }],
        );
    }

    #[test]
    fn a_setting_this_reader_does_not_know_stops_the_read() {
        let mut unknown = m7_pro();
        unknown.rows[2][0] = "Exposure Energy".to_owned();
        let mut run = Run::default();

        let err = run.absorb(&unknown).unwrap_err();
        assert!(
            matches!(err, AnycubicWikiError::UnknownSetting { .. }),
            "{err}"
        );
    }

    #[test]
    fn a_table_under_a_heading_naming_no_machine_stops_the_read() {
        let mut stray = m7_pro();
        stray.heading = "Resin Overview".into();
        let mut run = Run::default();

        let err = run.absorb(&stray).unwrap_err();
        assert!(
            matches!(err, AnycubicWikiError::UnexpectedHeading { .. }),
            "{err}"
        );
    }
}
