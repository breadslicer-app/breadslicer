/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! ELEGOO's published resin settings, as material profiles.
//!
//! The vendor publishes one spreadsheet covering every resin they sell
//! against every machine they sell it for, one row per combination. A
//! row is a calibration, not a resin, so the rows of one resin converge
//! on one profile with a block per machine.
//!
//! Unlike the other adapter here, this one consults nothing of ours:
//! the sheet states in the cell itself whether a machine splits its
//! travel, so nothing has to be looked up to read a row correctly.

pub mod cell;
pub mod machines;
pub mod sheet;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sc_print_primitives::{
    MaterialId, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin,
    PositiveFiniteSec, PrinterId,
};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialPhysical, MaterialProfile, SecondTravelStage,
};
use semver::Version;

use crate::elegoo_settings::cell::{Span, span, stages};
use crate::elegoo_settings::machines::{Machines, machines_named_by};
use crate::elegoo_settings::sheet::{Sheet, SheetReadError};
use crate::profile::ConvertedProfile;
use crate::reader::{ProfileReader, SourceReading};
use crate::source_note::SourceNote;

/// The vendor that publishes this sheet.
const BRAND: &str = "ELEGOO";

/// The header the sheet must carry, in the order its columns appear.
///
/// The reader demands it rather than trusting a column position, so a
/// re-ordered or re-titled sheet fails loudly instead of writing the
/// lifting speed into the exposure.
const HEADER: [&str; 11] = [
    "3DP Model",
    "Resin Type",
    "Notes",
    "Color",
    "Bottom Exposure(s)",
    "Normal Exposure(s)",
    "Layer Height(mm)",
    "Z Lifting Distance(mm)",
    "Z Lifting Speed(mm/min)",
    "Z Retract Speed(mm/min)",
    "Rest Time After Retract(s)",
];

/// Reads ELEGOO's published resin settings spreadsheet.
#[derive(Clone, Copy, Debug, Default)]
pub struct ElegooSettingsReader;

/// Everything that can stop this reader.
#[derive(Debug)]
pub enum ElegooSettingsError {
    /// The workbook could not be read.
    Sheet(SheetReadError),
    /// No row carries the header this reader knows, so the sheet is not
    /// the one it reads or its shape has changed.
    HeaderNotFound { path: PathBuf },
    /// A row names a resin whose name cannot be an id.
    InvalidId {
        resin: String,
        colour: String,
        source: sc_print_primitives::InvalidProfileId,
    },
}

impl fmt::Display for ElegooSettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sheet(err) => write!(f, "{err}"),
            Self::HeaderNotFound { path } => write!(
                f,
                "{} carries no row with the expected header, starting {:?}",
                path.display(),
                HEADER[0],
            ),
            Self::InvalidId {
                resin,
                colour,
                source,
            } => write!(f, "{resin:?} in {colour:?} cannot be named: {source}"),
        }
    }
}

impl std::error::Error for ElegooSettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sheet(err) => Some(err),
            Self::HeaderNotFound { .. } => None,
            Self::InvalidId { source, .. } => Some(source),
        }
    }
}

impl ProfileReader for ElegooSettingsReader {
    type Error = ElegooSettingsError;

    fn read(&self, inputs: &[&Path]) -> Result<SourceReading, Self::Error> {
        let mut run = Run::default();
        for path in inputs {
            let sheet = Sheet::open(path).map_err(ElegooSettingsError::Sheet)?;
            run.absorb(&sheet, path)?;
        }
        Ok(run.finish())
    }
}

/// One resin at one layer height, and the machines it is calibrated on.
struct Resin {
    identity: MaterialIdentity,
    height: PositiveFiniteMm,
    blocks: BTreeMap<CalibrationTarget, Slot>,
    /// What the sheet's Notes column says for each machine.
    notes: BTreeMap<CalibrationTarget, String>,
}

/// What is known about one machine's calibration for one resin.
enum Slot {
    /// Every row agreed, or there was only one.
    Agreed(Box<MaterialPerPrinter>),
    /// Rows disagreed, so the resin carries no block for the machine.
    Disagreed,
}

/// One read in progress.
#[derive(Default)]
struct Run {
    resins: BTreeMap<MaterialId, Resin>,
    skipped: BTreeMap<Skipped, usize>,
    conflicts: Vec<(MaterialId, PrinterId)>,
}

/// A reason rows were passed over, counted rather than listed one by
/// one: the sheet states thousands of rows and a note per row would
/// bury the ones that matter.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Skipped {
    MachineNotShipped(String),
    ModelNotRecognised(String),
    NoResin,
    NoTravel(String),
    Unreadable(String),
}

impl Run {
    /// Read every data row of `sheet`.
    fn absorb(&mut self, sheet: &Sheet, path: &Path) -> Result<(), ElegooSettingsError> {
        let start = header_row(sheet).ok_or_else(|| ElegooSettingsError::HeaderNotFound {
            path: path.to_path_buf(),
        })?;
        for row in sheet.rows().iter().skip(start + 1) {
            self.absorb_row(row)?;
        }
        Ok(())
    }

    /// Read one data row, or count why it was passed over.
    fn absorb_row(&mut self, row: &[String]) -> Result<(), ElegooSettingsError> {
        let at = |index: usize| {
            row.get(index)
                .map(String::as_str)
                .unwrap_or_default()
                .trim()
        };
        let (model, resin, note, colour) = (at(0), at(1), at(2), at(3));
        if model.is_empty() {
            return Ok(());
        }
        if resin.is_empty() {
            return self.skip(Skipped::NoResin);
        }
        let printers = match machines_named_by(model) {
            Machines::Ours(ids) => ids,
            Machines::NotOurs => return self.skip(Skipped::MachineNotShipped(model.to_owned())),
            Machines::Unknown => return self.skip(Skipped::ModelNotRecognised(model.to_owned())),
        };
        let Some(height) = span(at(6)).and_then(|height| mm(height.low())) else {
            return self.skip(Skipped::Unreadable(model.to_owned()));
        };
        let Some(block) = block_of(at(4), at(5), at(7), at(8), at(9), at(10)) else {
            return self.skip(Skipped::NoTravel(model.to_owned()));
        };

        let id = material_id_of(resin, colour, height)?;
        let entry = self.resins.entry(id.clone()).or_insert_with(|| Resin {
            identity: identity_of(&id, resin, colour, height),
            height,
            blocks: BTreeMap::new(),
            notes: BTreeMap::new(),
        });
        for printer in printers {
            let id_of_printer = PrinterId::new(printer).expect("the map holds only ids we ship");
            let target = CalibrationTarget::from_printer(&id_of_printer);
            if !note.is_empty() {
                entry
                    .notes
                    .entry(target.clone())
                    .or_insert_with(|| tidy(note));
            }
            match entry.blocks.get(&target) {
                Some(Slot::Agreed(seen)) if **seen != block => {
                    entry.blocks.insert(target, Slot::Disagreed);
                    self.conflicts.push((id.clone(), id_of_printer));
                }
                Some(_) => {}
                None => {
                    entry.blocks.insert(target, Slot::Agreed(Box::new(block)));
                }
            }
        }
        Ok(())
    }

    /// Count one passed-over row.
    fn skip(&mut self, reason: Skipped) -> Result<(), ElegooSettingsError> {
        *self.skipped.entry(reason).or_default() += 1;
        Ok(())
    }

    /// Turn what was read into profiles, and what was not into notes.
    fn finish(self) -> SourceReading {
        let mut notes: Vec<SourceNote> = self
            .skipped
            .into_iter()
            .map(|(reason, rows)| reason.into_note(rows))
            .collect();
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
                let block_notes: BTreeMap<CalibrationTarget, String> = resin
                    .notes
                    .into_iter()
                    .filter(|(target, _)| per_printer.contains_key(target))
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
                        notes: block_notes,
                    })
                })
            })
            .collect();
        SourceReading { profiles, notes }
    }
}

impl Skipped {
    /// The note that says how many rows this passed over, and why.
    fn into_note(self, rows: usize) -> SourceNote {
        match self {
            Self::MachineNotShipped(model) => {
                SourceNote::RowsForAMachineWeDoNotShip { model, rows }
            }
            Self::ModelNotRecognised(model) => SourceNote::RowsForAnUnknownModel { model, rows },
            Self::NoResin => SourceNote::RowsNamingNoResin { rows },
            Self::NoTravel(model) => SourceNote::RowsStatingNoTravel { model, rows },
            Self::Unreadable(model) => SourceNote::RowsWeCannotRead { model, rows },
        }
    }
}

/// Which row carries the header, if any does.
fn header_row(sheet: &Sheet) -> Option<usize> {
    sheet.rows().iter().position(|row| {
        HEADER
            .iter()
            .enumerate()
            .all(|(index, expected)| row.get(index).is_some_and(|cell| cell.trim() == *expected))
    })
}

/// The calibration one row states, or nothing when it states no travel.
///
/// A machine that plans its own motion leaves the travel cells empty,
/// and a block without a lift cannot resolve, so such a row yields no
/// block at all rather than a partial one.
fn block_of(
    bottom: &str,
    normal: &str,
    distance: &str,
    lift_speed: &str,
    retract_speed: &str,
    rest: &str,
) -> Option<MaterialPerPrinter> {
    let (first_distance, second_distance) = stages(distance)?;
    let (first_lift, second_lift) = stages(lift_speed)?;
    let (fast_descent, final_approach) = stages(retract_speed)?;
    let splits = second_distance > 0.0;

    // Going down, the platform crosses the second stage's distance
    // first and fast, then makes the final approach slowly, so the
    // sheet's retract pair is its lift pair reversed.
    let second = splits
        .then(|| {
            Some(SecondTravelStage {
                lift_distance_mm: mm(second_distance)?,
                lift_speed_mm_min: speed(second_lift)?,
                retract_speed_mm_min: speed(fast_descent)?,
            })
        })
        .flatten();
    let rise = if splits {
        first_distance
    } else {
        first_distance + second_distance
    };
    let retract = if splits { final_approach } else { fast_descent };

    Some(MaterialPerPrinter {
        normal_exposure_sec: Some(sec(span(normal)?.low())?),
        bottom_exposure_sec: Some(sec(span(bottom)?.high())?),
        bottom_layer_count: None,
        wait_before_cure_sec: span(rest).map(Span::low).and_then(wait),
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: None,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: Some(mm(rise)?),
        lift_speed_mm_min: Some(speed(first_lift)?),
        retract_speed_mm_min: Some(speed(retract)?),
        bottom_lift_distance_mm: None,
        bottom_lift_speed_mm_min: None,
        bottom_retract_speed_mm_min: None,
        light_pwm: None,
        bottom_light_pwm: None,
        second_travel: second,
        bottom_second_travel: None,
    })
}

/// Name a resin after the product, the colour it is sold in, and the
/// layer height its exposures were measured at.
///
/// The height belongs in the name because the sheet publishes some
/// combinations twice, once per height, with exposures that differ.
fn material_id_of(
    resin: &str,
    colour: &str,
    height: PositiveFiniteMm,
) -> Result<MaterialId, ElegooSettingsError> {
    let slug = format!(
        "{}-{}-{}-{}",
        slug(BRAND),
        slug(resin),
        slug(colour),
        micron_token(height),
    );
    MaterialId::new(&slug).map_err(|source| ElegooSettingsError::InvalidId {
        resin: resin.to_owned(),
        colour: colour.to_owned(),
        source,
    })
}

/// How a resin identifies itself once its id is settled.
fn identity_of(
    id: &MaterialId,
    resin: &str,
    colour: &str,
    height: PositiveFiniteMm,
) -> MaterialIdentity {
    let name = format!("{resin} ({colour}, {} um)", microns(height));
    MaterialIdentity {
        id: id.clone(),
        brand: NonEmptyString::new(BRAND).expect("a constant that is not empty"),
        family: NonEmptyString::new(family_of(resin)).expect("a constant that is not empty"),
        name: NonEmptyString::new(&name).expect("a resin and a colour are both non-empty"),
        profile_version: Version::new(1, 0, 0),
    }
}

/// Which of the library's families a resin type belongs to.
///
/// The vocabulary is the one the bundled profiles already use, so a
/// resin sorts next to its equivalents rather than into a family of its
/// own. Water washing is a cleaning property rather than a mechanical
/// one, so those sit with the standard resins, as the bundle's other
/// water washable resins do.
fn family_of(resin: &str) -> &'static str {
    let resin = resin.to_ascii_lowercase();
    if resin.contains("abs-like") {
        "abs-like"
    } else if resin.contains("tough") {
        "tough"
    } else if resin.contains("castable") || resin.contains("thermochromic") {
        "other"
    } else if resin.contains("dental") {
        "engineering"
    } else {
        "standard"
    }
}

/// A name as an id fragment, keeping every distinction the vendor draws.
///
/// A `+` becomes a word rather than vanishing: the sheet sells "3.0",
/// "3.0 Pro" and "3.0+" as 3 products, and dropping the sign would give
/// 2 of them one id.
fn slug(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .replace('+', " plus ")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// A layer height in microns, as a whole number.
fn microns(height: PositiveFiniteMm) -> u32 {
    (height.value() * 1000.0).round() as u32
}

/// A layer height as the token an id carries.
fn micron_token(height: PositiveFiniteMm) -> String {
    format!("{}um", microns(height))
}

/// A length the schema accepts, or nothing when the sheet's number is
/// not one.
fn mm(value: f64) -> Option<PositiveFiniteMm> {
    PositiveFiniteMm::new(value).ok()
}

/// A speed the schema accepts.
fn speed(value: f64) -> Option<PositiveFiniteMmPerMin> {
    PositiveFiniteMmPerMin::new(value).ok()
}

/// An exposure the schema accepts.
fn sec(value: f64) -> Option<PositiveFiniteSec> {
    PositiveFiniteSec::new(value).ok()
}

/// Fold `value` onto one line, turning the line breaks a cell may carry
/// into single spaces so a note reads as a sentence, and each full-width
/// form into the ASCII character it stands for.
fn tidy(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(ascii_of_full_width)
        .collect()
}

/// Map a full-width form to the ASCII character it stands for, leaving
/// any other character as it is.
fn ascii_of_full_width(c: char) -> char {
    match c {
        '\u{ff01}'..='\u{ff5e}' => char::from_u32(u32::from(c) - 0xfee0).unwrap_or(c),
        _ => c,
    }
}

/// A pause the schema accepts, which unlike an exposure may be 0.
fn wait(value: f64) -> Option<NonNegativeFiniteSec> {
    NonNegativeFiniteSec::new(value).ok()
}

#[cfg(test)]
mod elegoo_settings_tests {
    use super::*;

    /// Rows copied from the published sheet, so the fixtures state what
    /// the vendor states rather than what is convenient.
    const MARS_5: [&str; 11] = [
        "ELEGOO Mars 5",
        "Standard Resin",
        "",
        "Grey",
        "27-32",
        "2.5-3",
        "0.05",
        "2+3",
        "75+230",
        "230+75",
        "0.5",
    ];
    const MARS_3: [&str; 11] = [
        "ELEGOO Mars 3/Mars 3 Pro",
        "Standard Resin",
        "",
        "Grey",
        "25-30",
        "2.5-3",
        "0.05",
        "5+0",
        "80+0",
        "210+0",
        "0.5",
    ];
    const MARS_5_ULTRA: [&str; 11] = [
        "ELEGOO Mars 5 Ultra",
        "Standard Resin",
        "",
        "Grey",
        "27-32",
        "2.5-3",
        "0.05",
        "/",
        "/",
        "/",
        "/",
    ];

    fn sheet_of(rows: &[[&str; 11]]) -> Sheet {
        let mut all = vec![HEADER.iter().map(|cell| (*cell).to_owned()).collect()];
        all.extend(
            rows.iter()
                .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect()),
        );
        Sheet::from_rows(all)
    }

    fn read(rows: &[[&str; 11]]) -> SourceReading {
        let mut run = Run::default();
        run.absorb(&sheet_of(rows), Path::new("settings.xlsx"))
            .unwrap();
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
    fn a_row_becomes_a_resin_named_for_its_product_colour_and_height() {
        let reading = read(&[MARS_5]);

        let profile = only_material(&reading);
        assert_eq!(
            profile.identity.id.as_str(),
            "elegoo-standard-resin-grey-50um",
        );
        assert_eq!(profile.identity.brand.as_str(), "ELEGOO");
        assert_eq!(profile.identity.family.as_str(), "standard");
        assert_eq!(
            profile.identity.name.as_str(),
            "Standard Resin (Grey, 50 um)"
        );
        assert_eq!(
            profile.intent.target_layer_height_mm,
            Some(PositiveFiniteMm::new(0.05).unwrap()),
        );
    }

    #[test]
    fn a_bottom_exposure_takes_the_high_end_and_a_normal_one_the_low() {
        let reading = read(&[MARS_5]);

        let block = block(only_material(&reading), "elegoo-mars-5");
        assert_eq!(
            block.bottom_exposure_sec,
            Some(PositiveFiniteSec::new(32.0).unwrap()),
        );
        assert_eq!(
            block.normal_exposure_sec,
            Some(PositiveFiniteSec::new(2.5).unwrap()),
        );
    }

    #[test]
    fn a_split_travel_keeps_its_stages_and_crosses_the_retract_speeds() {
        let reading = read(&[MARS_5]);

        let block = block(only_material(&reading), "elegoo-mars-5");
        // The sheet says 2+3 at 75+230 up and 230+75 down: the first
        // stage rises 2 mm at 75, the second 3 mm at 230, and the
        // descent crosses 230 over the second stage before approaching
        // at 75.
        assert_eq!(
            block.lift_distance_mm,
            Some(PositiveFiniteMm::new(2.0).unwrap()),
        );
        assert_eq!(
            block.lift_speed_mm_min,
            Some(PositiveFiniteMmPerMin::new(75.0).unwrap()),
        );
        assert_eq!(
            block.retract_speed_mm_min,
            Some(PositiveFiniteMmPerMin::new(75.0).unwrap()),
        );
        assert_eq!(
            block.second_travel,
            Some(SecondTravelStage {
                lift_distance_mm: PositiveFiniteMm::new(3.0).unwrap(),
                lift_speed_mm_min: PositiveFiniteMmPerMin::new(230.0).unwrap(),
                retract_speed_mm_min: PositiveFiniteMmPerMin::new(230.0).unwrap(),
            }),
        );
    }

    #[test]
    fn a_machine_that_moves_in_one_go_gets_the_whole_rise_and_no_second_stage() {
        let reading = read(&[MARS_3]);

        let block = block(only_material(&reading), "elegoo-mars-3");
        assert_eq!(
            block.lift_distance_mm,
            Some(PositiveFiniteMm::new(5.0).unwrap()),
        );
        assert_eq!(
            block.retract_speed_mm_min,
            Some(PositiveFiniteMmPerMin::new(210.0).unwrap()),
        );
        assert_eq!(block.second_travel, None);
    }

    #[test]
    fn one_row_naming_2_machines_calibrates_both() {
        let reading = read(&[MARS_3]);

        let profile = only_material(&reading);
        assert_eq!(profile.per_printer.len(), 2);
        assert_eq!(
            block(profile, "elegoo-mars-3"),
            block(profile, "elegoo-mars-3-pro"),
        );
    }

    #[test]
    fn the_rest_after_a_retract_becomes_the_wait_before_the_next_cure() {
        let reading = read(&[MARS_5]);

        assert_eq!(
            block(only_material(&reading), "elegoo-mars-5").wait_before_cure_sec,
            Some(NonNegativeFiniteSec::new(0.5).unwrap()),
        );
    }

    #[test]
    fn a_machine_that_states_no_travel_gets_no_block_and_is_reported() {
        let reading = read(&[MARS_5_ULTRA]);

        assert!(reading.profiles.is_empty());
        assert_eq!(
            reading.notes,
            vec![SourceNote::RowsStatingNoTravel {
                model: "ELEGOO Mars 5 Ultra".to_owned(),
                rows: 1,
            }],
        );
    }

    #[test]
    fn the_same_resin_at_2_heights_becomes_2_profiles() {
        let mut thick = MARS_5;
        thick[6] = "0.1";
        let reading = read(&[MARS_5, thick]);

        let mut ids: Vec<&str> = reading
            .profiles
            .iter()
            .map(|profile| match profile {
                ConvertedProfile::Material(material) => material.identity.id.as_str(),
                ConvertedProfile::Printer(_) => unreachable!("this reader writes no printer"),
            })
            .collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![
                "elegoo-standard-resin-grey-100um",
                "elegoo-standard-resin-grey-50um",
            ],
        );
    }

    #[test]
    fn rows_that_agree_merge_without_a_word() {
        let reading = read(&[MARS_5, MARS_5]);

        assert_eq!(reading.profiles.len(), 1);
        assert_eq!(reading.notes, Vec::new());
    }

    #[test]
    fn rows_that_disagree_leave_the_machine_uncalibrated_and_say_so() {
        let mut other = MARS_5;
        other[5] = "3-3.5";
        let reading = read(&[MARS_5, other]);

        assert!(reading.profiles.is_empty());
        assert_eq!(
            reading.notes,
            vec![SourceNote::RowsDisagree {
                material: MaterialId::new("elegoo-standard-resin-grey-50um").unwrap(),
                printer: PrinterId::new("elegoo-mars-5").unwrap(),
            }],
        );
    }

    #[test]
    fn a_row_carrying_a_note_writes_it_above_the_block_it_calibrates() {
        let mut noted = MARS_5;
        noted[2] = "NOTE: \nThis parameter works for Standard Resin+ Fast Speed(Hollow model, wall thickness\u{ff1e}2mm)";
        let reading = read(&[noted]);

        let profile = only_material(&reading);
        let mars_5 = CalibrationTarget::from_printer(&PrinterId::new("elegoo-mars-5").unwrap());
        assert_eq!(
            profile.notes.get(&mars_5).map(String::as_str),
            Some(
                "NOTE: This parameter works for Standard Resin+ Fast Speed(Hollow model, wall thickness>2mm)"
            ),
        );
        assert_eq!(reading.notes, Vec::new());
    }

    #[test]
    fn a_note_on_a_row_naming_2_machines_goes_above_both_blocks() {
        let mut noted = MARS_3;
        noted[2] = "NOTE: This parameter works for Standard Resin+ Normal Speed";
        let reading = read(&[noted]);

        let profile = only_material(&reading);
        assert_eq!(profile.notes.len(), 2, "{:?}", profile.notes);
    }

    #[test]
    fn a_note_leaves_with_a_block_the_rows_disagree_on() {
        let mut noted = MARS_5;
        noted[2] = "NOTE: This parameter works for Standard Resin+ Normal Speed";
        let mut other = MARS_5;
        other[5] = "3-3.5";
        let reading = read(&[noted, other, MARS_3]);

        let profile = only_material(&reading);
        assert!(profile.notes.is_empty(), "{:?}", profile.notes);
    }

    #[test]
    fn a_row_naming_no_resin_is_dropped_and_counted() {
        let mut nameless = MARS_5;
        nameless[1] = "";
        let reading = read(&[nameless, MARS_5]);

        assert_eq!(reading.profiles.len(), 1);
        assert_eq!(
            reading.notes,
            vec![SourceNote::RowsNamingNoResin { rows: 1 }]
        );
    }

    #[test]
    fn rows_for_machines_we_do_not_ship_are_counted_per_machine() {
        let mut old = MARS_5;
        old[0] = "ELEGOO Saturn 8K";
        let reading = read(&[old, old, MARS_5]);

        assert_eq!(reading.profiles.len(), 1);
        assert_eq!(
            reading.notes,
            vec![SourceNote::RowsForAMachineWeDoNotShip {
                model: "ELEGOO Saturn 8K".to_owned(),
                rows: 2,
            }],
        );
    }

    #[test]
    fn a_model_this_converter_never_saw_is_reported_apart_from_one_we_simply_lack() {
        let mut future = MARS_5;
        future[0] = "ELEGOO Mars 9";
        let reading = read(&[future]);

        assert_eq!(
            reading.notes,
            vec![SourceNote::RowsForAnUnknownModel {
                model: "ELEGOO Mars 9".to_owned(),
                rows: 1,
            }],
        );
    }

    #[test]
    fn a_resin_carries_no_bottom_layer_count_because_the_sheet_publishes_none() {
        let reading = read(&[MARS_5]);

        let profile = only_material(&reading);
        assert_eq!(profile.defaults, MaterialDefaults::default());
        assert_eq!(
            block(profile, "elegoo-mars-5").bottom_layer_count,
            None,
            "no column of the sheet states one",
        );
    }

    #[test]
    fn a_sheet_without_the_expected_header_is_refused() {
        let sheet = Sheet::from_rows(vec![vec!["Printer".to_owned(), "Resin".to_owned()]]);
        let mut run = Run::default();

        let err = run.absorb(&sheet, Path::new("other.xlsx")).unwrap_err();

        assert!(
            matches!(err, ElegooSettingsError::HeaderNotFound { .. }),
            "{err}",
        );
    }

    #[test]
    fn the_3_products_the_vendor_numbers_alike_keep_3_ids() {
        let ids: Vec<String> = [
            "ABS-Like Resin 3.0",
            "ABS-Like Resin 3.0 Pro",
            "ABS-Like Resin 3.0+",
        ]
        .into_iter()
        .map(|resin| {
            material_id_of(resin, "Grey", PositiveFiniteMm::new(0.05).unwrap())
                .unwrap()
                .as_str()
                .to_owned()
        })
        .collect();

        assert_eq!(
            ids,
            vec![
                "elegoo-abs-like-resin-3-0-grey-50um",
                "elegoo-abs-like-resin-3-0-pro-grey-50um",
                "elegoo-abs-like-resin-3-0-plus-grey-50um",
            ],
        );
    }

    #[test]
    fn a_resin_sorts_into_the_family_its_kind_belongs_to() {
        assert_eq!(family_of("Standard Resin"), "standard");
        assert_eq!(family_of("Water Washable Resin 2.0"), "standard");
        assert_eq!(family_of("8K ABS-Like Resin 3.0"), "abs-like");
        assert_eq!(family_of("Tough Photopolymer Resin"), "tough");
        assert_eq!(family_of("Dental Castable UV Resin"), "other");
        assert_eq!(family_of("Dental Model UV Resin"), "engineering");
        assert_eq!(family_of("Thermochromic Resin"), "other");
    }
}
