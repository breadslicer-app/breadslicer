/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Tools → Calibrate resin (#78): a card that places the XY coupon on the
//! plate and, once it is printed, walks through reading it with calipers, a
//! picture per step, and says what the numbers mean. Each reading is kept
//! with its printer and resin in `calibration-log.toml`; the corrections it
//! points to are applied by hand for now (scale compensation, exposure).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bread_calibration::coupon::{HOLES_MM, PINS_MM, SLITS_MM};
use bread_calibration::fit::{GOOD_OFFSET_MM, GOOD_SCALE};
use bread_calibration::{Coupon, Doubt, Exposure, Slit, Step, Verdict, XyReadings, XyResult};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::i18n::ctr;
use crate::preferences_bus::PreferencesBus;
use crate::viewport::ViewportController;
use crate::{Calib, CalibLine, MainWindow};

/// The readings typed in the card: the temperature, the blocks along X
/// (16, 8, 4) and along Y, the bits in the holes, the foot at the bottom and
/// near the top, the holes of 3 and 4 mm read from the inside.
const FIELDS: usize = 18;
const TEMPERATURE: usize = 0;
const BLOCKS_X: usize = 1;
const BLOCKS_Y: usize = 4;
const BITS: usize = 7;
const FOOT_BOTTOM: usize = 14;
const FOOT_TOP: usize = 15;
const HOLE_JAWS: usize = 16;
/// Readings of each printer and resin pair kept in the log.
const KEPT_PER_PAIR: usize = 20;

/// One reading of a coupon, as the log keeps it. A reading not made is 0;
/// the thinnest pin is -1 when none stood, a slit or the thinnest hole -1
/// when all closed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CalibrationRecord {
    pub printer: String,
    pub resin: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub temperature_c: f32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default)]
    pub blocks_x: [f32; 3],
    #[serde(default)]
    pub blocks_y: [f32; 3],
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hole_light: f32,
    #[serde(default)]
    pub hole_jaws: [f32; 2],
    #[serde(default)]
    pub hole_bits: [f32; 7],
    #[serde(default, skip_serializing_if = "is_zero")]
    pub thinnest_pin: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub slit_x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub slit_y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub foot_bottom: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub foot_top: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn read_or_none(v: f32) -> Option<f32> {
    (v > 0.0).then_some(v)
}

fn slit_of(v: f32) -> Slit {
    if v < 0.0 {
        Slit::AllClosed
    } else if v > 0.0 {
        Slit::Open(v)
    } else {
        Slit::NotRead
    }
}

impl CalibrationRecord {
    pub(crate) fn readings(&self) -> XyReadings {
        XyReadings {
            blocks_x: self.blocks_x.map(read_or_none),
            blocks_y: self.blocks_y.map(read_or_none),
            hole_light: slit_of(self.hole_light),
            hole_jaws: self.hole_jaws.map(read_or_none),
            hole_bits: self.hole_bits.map(read_or_none),
            thinnest_pin: read_or_none(self.thinnest_pin),
            slit_x: slit_of(self.slit_x),
            slit_y: slit_of(self.slit_y),
            foot_bottom: read_or_none(self.foot_bottom),
            foot_top: read_or_none(self.foot_top),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct LogFile {
    #[serde(default, rename = "calibration")]
    records: Vec<CalibrationRecord>,
}

/// Where the log lives: `None` in tests and when the platform gives no
/// data folder.
fn log_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(
        sc_app_paths::project_dirs()?
            .data_local_dir()
            .join("calibration-log.toml"),
    )
}

fn read_log(path: &Path) -> Vec<CalibrationRecord> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str::<LogFile>(&text).ok())
        .map(|log| log.records)
        .unwrap_or_default()
}

/// Adds `record` to the log at `path`, dropping its pair's oldest past
/// [`KEPT_PER_PAIR`]; written to a temporary file first.
pub(crate) fn record_to(path: &Path, record: CalibrationRecord) -> std::io::Result<()> {
    let mut records = read_log(path);
    records.push(record);
    let mut seen = std::collections::HashMap::<(String, String), usize>::new();
    let mut kept = Vec::new();
    for r in records.into_iter().rev() {
        let count = seen
            .entry((r.printer.clone(), r.resin.clone()))
            .or_default();
        *count += 1;
        if *count <= KEPT_PER_PAIR {
            kept.push(r);
        }
    }
    kept.reverse();
    let text = toml::to_string(&LogFile { records: kept })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

/// The latest reading of `printer` with `resin` in the log at `path`.
pub(crate) fn latest_of(path: &Path, printer: &str, resin: &str) -> Option<CalibrationRecord> {
    read_log(path)
        .into_iter()
        .rev()
        .find(|r| r.printer == printer && r.resin == resin)
}

/// A number with `decimals`, in the language's way of writing it.
fn number(value: f32, decimals: usize) -> String {
    crate::numbers::shown(&format!("{value:.decimals$}"))
}

fn signed(value: f32, decimals: usize) -> String {
    crate::numbers::shown(&format!("{value:+.decimals$}"))
}

fn line(label: String, value: String, tone: i32) -> CalibLine {
    CalibLine {
        label: label.into(),
        value: value.into(),
        tone,
    }
}

/// What the card says about a result: its heading, its lines and the advice
/// under them. `exposure_sec` is the resin's normal exposure, when known.
pub(crate) fn describe(
    result: &XyResult,
    pin_none: bool,
    exposure_sec: Option<f64>,
) -> (String, Vec<CalibLine>, String) {
    let heading = match result.verdict {
        Verdict::NotEnough => ctr!(
            "Calibration",
            "Not enough readings: two blocks along one axis are needed"
        ),
        Verdict::Good => ctr!(
            "Calibration",
            "Well calibrated: the piece comes out as drawn"
        ),
        Verdict::Adjust => ctr!("Calibration", "Worth adjusting"),
        Verdict::Suspicious => ctr!("Calibration", "Check a reading before trusting the rest"),
    }
    .to_string();
    let mut lines = Vec::new();
    for doubt in &result.doubts {
        let (what, read) = match *doubt {
            Doubt::BlockX { side, read } => (
                ctr!("Calibration", "Block of {} mm along X", number(side, 0)),
                read,
            ),
            Doubt::BlockY { side, read } => (
                ctr!("Calibration", "Block of {} mm along Y", number(side, 0)),
                read,
            ),
            Doubt::Foot { read } => (ctr!("Calibration", "The column"), read),
            Doubt::HoleJaws { hole, read } => (
                ctr!(
                    "Calibration",
                    "Hole of {} mm, from the inside",
                    number(hole, 0)
                ),
                read,
            ),
            Doubt::HoleBit { hole, bit } => (
                ctr!("Calibration", "Bit in the hole of {} mm", number(hole, 1)),
                bit,
            ),
        };
        lines.push(line(
            ctr!("Calibration", "Check: {}", what).to_string(),
            format!("{} mm", number(read, 2)),
            3,
        ));
    }
    for (axis, fit) in [("X", result.x), ("Y", result.y)] {
        let Some(fit) = fit else {
            continue;
        };
        let scale_tone = if (fit.scale - 1.0).abs() <= GOOD_SCALE {
            1
        } else {
            2
        };
        lines.push(line(
            ctr!("Calibration", "Scale along {}", axis).to_string(),
            format!("{} %", signed((fit.scale - 1.0) * 100.0, 2)),
            scale_tone,
        ));
        let wall_tone = if fit.offset_mm.abs() <= GOOD_OFFSET_MM {
            1
        } else {
            2
        };
        lines.push(line(
            ctr!("Calibration", "Each wall along {}", axis).to_string(),
            format!("{} mm", signed(fit.offset_mm, 3)),
            wall_tone,
        ));
    }
    if let Some(closing) = result.hole_closing_mm {
        lines.push(line(
            ctr!("Calibration", "The holes close by").to_string(),
            format!("{} mm", number(closing, 2)),
            if closing > 0.15 { 2 } else { 0 },
        ));
    }
    let thinnest_hole = match result.hole_light {
        Slit::NotRead => None,
        Slit::Open(v) => Some((format!("{} mm", number(v, 1)), 0)),
        Slit::AllClosed => Some((ctr!("Calibration", "none").to_string(), 2)),
    };
    if let Some((value, tone)) = thinnest_hole {
        lines.push(line(
            ctr!("Calibration", "Thinnest hole the light comes through").to_string(),
            value,
            tone,
        ));
    }
    if let Some(foot) = result.foot_mm {
        lines.push(line(
            ctr!("Calibration", "Elephant's foot, per side").to_string(),
            format!("{} mm", number(foot, 2)),
            if foot > 0.1 { 2 } else { 1 },
        ));
    }
    if pin_none {
        lines.push(line(
            ctr!("Calibration", "Thinnest pin that stood").to_string(),
            ctr!("Calibration", "none").to_string(),
            2,
        ));
    } else if let Some(pin) = result.thinnest_pin {
        lines.push(line(
            ctr!("Calibration", "Thinnest pin that stood").to_string(),
            format!("{} mm", number(pin, 1)),
            0,
        ));
    }
    for (axis, slit) in [("X", result.slit_x), ("Y", result.slit_y)] {
        let value = match slit {
            Slit::NotRead => continue,
            Slit::Open(v) => format!("{} mm", number(v, 1)),
            Slit::AllClosed => ctr!("Calibration", "all closed").to_string(),
        };
        lines.push(line(
            ctr!("Calibration", "Thinnest slit open along {}", axis).to_string(),
            value,
            if slit == Slit::AllClosed { 2 } else { 0 },
        ));
    }

    let mut advice = Vec::new();
    if result.verdict == Verdict::Suspicious {
        advice.push(ctr!(
            "Calibration",
            "A reading is more than 1 mm off what was drawn: go back and check it, it is likely a slip."
        )
        .to_string());
    }
    let comp: Vec<String> = [("X", result.x), ("Y", result.y)]
        .iter()
        .filter_map(|(axis, fit)| {
            fit.filter(|f| (f.scale - 1.0).abs() > GOOD_SCALE)
                .map(|f| format!("{axis} {} %", signed(f.compensation_pct(), 2)))
        })
        .collect();
    if !comp.is_empty() && result.verdict != Verdict::Suspicious {
        advice.push(
            ctr!(
                "Calibration",
                "Scale: set the resin's scale compensation to {} (Printer & resin, edit the resin).",
                comp.join(", ")
            )
            .to_string(),
        );
    }
    if result.verdict != Verdict::Suspicious {
        match result.exposure {
            Some(Exposure::Over) => advice.push(match exposure_sec {
                Some(sec) => ctr!(
                    "Calibration",
                    "The walls grow: there is light to spare. Try the normal exposure at {} s instead of {} s and print the piece again.",
                    number((sec * 0.9) as f32, 2),
                    number(sec as f32, 2)
                )
                .to_string(),
                None => ctr!(
                    "Calibration",
                    "The walls grow: there is light to spare. Try a normal exposure 10 % shorter and print the piece again."
                )
                .to_string(),
            }),
            Some(Exposure::Under) => advice.push(match exposure_sec {
                Some(sec) => ctr!(
                    "Calibration",
                    "The walls shrink: the light falls short. Try the normal exposure at {} s instead of {} s and print the piece again.",
                    number((sec * 1.1) as f32, 2),
                    number(sec as f32, 2)
                )
                .to_string(),
                None => ctr!(
                    "Calibration",
                    "The walls shrink: the light falls short. Try a normal exposure 10 % longer and print the piece again."
                )
                .to_string(),
            }),
            Some(Exposure::Right) => advice.push(
                ctr!("Calibration", "The exposure is right for the walls.").to_string(),
            ),
            None => {}
        }
    }
    if result.foot_mm.is_some_and(|f| f > 0.1) && result.verdict != Verdict::Suspicious {
        advice.push(
            ctr!(
                "Calibration",
                "The first layers spread: a shorter bottom exposure, or fewer bottom layers, makes the foot smaller."
            )
            .to_string(),
        );
    }
    (heading, lines, advice.join("\n\n"))
}

struct Wizard {
    window: slint::Weak<MainWindow>,
    ctrl: Rc<ViewportController>,
    prefs: Rc<PreferencesBus>,
    open_files: Rc<dyn Fn(Vec<PathBuf>)>,
    texts: Rc<VecModel<SharedString>>,
    bad: Rc<VecModel<bool>>,
    values: RefCell<[Option<f32>; FIELDS]>,
    /// The printer and resin read for, by id.
    pair: RefCell<Option<(String, String)>>,
    pictures: RefCell<Option<ModelRc<Image>>>,
}

fn chosen(options: ModelRc<SharedString>, index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|i| options.row_data(i))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

impl Wizard {
    fn pictures(&self) -> ModelRc<Image> {
        self.pictures
            .borrow_mut()
            .get_or_insert_with(|| {
                let coupon = Coupon::xy();
                let images: Vec<Image> = [
                    Step::Overview,
                    Step::BlocksX,
                    Step::BlocksY,
                    Step::Holes,
                    Step::Pins,
                    Step::Slits,
                    Step::Foot,
                ]
                .iter()
                .map(|&step| {
                    Image::load_from_svg_data(bread_calibration::picture(&coupon, step).as_bytes())
                        .unwrap_or_default()
                })
                .collect();
                ModelRc::new(VecModel::from(images))
            })
            .clone()
    }

    fn open(&self) {
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        let selection = self.ctrl.document().print_selection();
        let snapshot = self.prefs.snapshot();
        let printer = selection.printer.or(snapshot.default_printer);
        let resin = selection.resin.or(snapshot.default_resin);
        let pair = printer
            .zip(resin)
            .map(|(p, r)| (p.as_str().to_owned(), r.as_str().to_owned()));
        let printer_name = chosen(
            ui.get_slice_printer_options(),
            ui.get_slice_printer_current_index(),
        );
        let resin_name = chosen(
            ui.get_slice_resin_options(),
            ui.get_slice_resin_current_index(),
        );
        let calib = ui.global::<Calib>();
        calib.set_pair(
            if pair.is_some() && !printer_name.is_empty() {
                format!("{printer_name} · {resin_name}")
            } else {
                ctr!(
                    "Calibration",
                    "Pick a printer and a resin first to keep the reading"
                )
                .to_string()
            }
            .into(),
        );
        let last = pair.as_ref().and_then(|(p, r)| {
            let record = latest_of(&log_path()?, p, r)?;
            let verdict = record.readings().result().verdict;
            let said = match verdict {
                Verdict::Good => ctr!("Calibration", "well calibrated"),
                Verdict::Adjust => ctr!("Calibration", "worth adjusting"),
                _ => ctr!("Calibration", "incomplete"),
            };
            Some(ctr!(
                "Calibration",
                "Last reading with this printer and resin: {}, {}.",
                record.at,
                said
            ))
        });
        calib.set_last(last.unwrap_or_default().into());
        *self.pair.borrow_mut() = pair;
        calib.set_can_save(self.pair.borrow().is_some());

        for i in 0..FIELDS {
            self.texts.set_row_data(i, SharedString::new());
            self.bad.set_row_data(i, false);
        }
        *self.values.borrow_mut() = [None; FIELDS];
        calib.set_texts(ModelRc::from(self.texts.clone()));
        calib.set_bad(ModelRc::from(self.bad.clone()));
        calib.set_pictures(self.pictures());
        calib.set_notes(SharedString::new());
        calib.set_pin(-1);
        calib.set_hole(-1);
        calib.set_slit_x(-1);
        calib.set_slit_y(-1);
        calib.set_saved(false);
        calib.set_page(0);
        calib.set_open(true);
    }

    fn edited(&self, field: i32, text: &str) {
        let Ok(field) = usize::try_from(field) else {
            return;
        };
        if field >= FIELDS {
            return;
        }
        let trimmed = text.trim();
        let value = crate::numbers::parse_number(trimmed)
            .map(|v| v as f32)
            .filter(|v| v.is_finite() && *v > 0.0);
        self.texts.set_row_data(field, text.into());
        self.bad
            .set_row_data(field, !trimmed.is_empty() && value.is_none());
        self.values.borrow_mut()[field] = value;
    }

    /// The readings in the card.
    fn record(&self, ui: &MainWindow) -> CalibrationRecord {
        let calib = ui.global::<Calib>();
        let v = *self.values.borrow();
        let take = |i: usize| v[i].unwrap_or(0.0);
        let pin = calib.get_pin();
        let thinnest_pin = match usize::try_from(pin) {
            Ok(i) if i < PINS_MM.len() => PINS_MM[i],
            Ok(_) => -1.0,
            Err(_) => 0.0,
        };
        let opening = |sizes: &[f32], index: i32| match usize::try_from(index) {
            Ok(i) if i < sizes.len() => sizes[i],
            Ok(_) => -1.0,
            Err(_) => 0.0,
        };
        let slit = |index: i32| opening(&SLITS_MM, index);
        let (printer, resin) = self.pair.borrow().clone().unwrap_or_default();
        CalibrationRecord {
            printer,
            resin,
            at: crate::print_log::now_text(),
            temperature_c: take(TEMPERATURE),
            notes: calib.get_notes().trim().to_owned(),
            blocks_x: std::array::from_fn(|k| take(BLOCKS_X + k)),
            blocks_y: std::array::from_fn(|k| take(BLOCKS_Y + k)),
            hole_light: opening(&HOLES_MM, calib.get_hole()),
            hole_jaws: std::array::from_fn(|k| take(HOLE_JAWS + k)),
            hole_bits: std::array::from_fn(|k| take(BITS + k)),
            thinnest_pin,
            slit_x: slit(calib.get_slit_x()),
            slit_y: slit(calib.get_slit_y()),
            foot_bottom: take(FOOT_BOTTOM),
            foot_top: take(FOOT_TOP),
        }
    }

    /// The resin's normal exposure, for the advice.
    fn exposure_sec(&self) -> Option<f64> {
        let (printer, resin) = self.pair.borrow().clone()?;
        let printer = sc_print_primitives::PrinterId::new(&printer).ok()?;
        let resin = sc_print_primitives::MaterialId::new(&resin).ok()?;
        let (printer, material) =
            crate::slice_projection::load_selected_profiles(&printer, &resin).ok()?;
        let resolved = sc_print_profiles::resolve(
            &printer,
            &material,
            &sc_print_profiles::SceneOverrides::default(),
        )
        .ok()?;
        Some(resolved.material_resolved.normal_exposure_sec.value())
    }

    fn show_result(&self) {
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        let record = self.record(&ui);
        let result = record.readings().result();
        let (heading, lines, advice) =
            describe(&result, record.thinnest_pin < 0.0, self.exposure_sec());
        let calib = ui.global::<Calib>();
        calib.set_result_heading(heading.into());
        calib.set_result_lines(ModelRc::new(VecModel::from(lines)));
        calib.set_result_advice(advice.into());
        calib.set_result_verdict(match result.verdict {
            Verdict::NotEnough => 0,
            Verdict::Good => 1,
            Verdict::Adjust => 2,
            Verdict::Suspicious => 3,
        });
        calib.set_saved(false);
    }

    fn save(&self) {
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        if self.pair.borrow().is_none() {
            return;
        }
        let record = self.record(&ui);
        let saved = match log_path() {
            Some(path) => match record_to(&path, record) {
                Ok(()) => true,
                Err(err) => {
                    eprintln!("calibration: the log could not be written: {err}");
                    false
                }
            },
            None => true,
        };
        ui.global::<Calib>().set_saved(saved);
    }

    /// Writes the coupon where the app keeps its data and adds it to the
    /// scene, like an imported model.
    fn place_coupon(&self) {
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        let dir = if cfg!(test) {
            std::env::temp_dir().join("breadslicer-calibration-test")
        } else {
            match sc_app_paths::project_dirs() {
                Some(dirs) => dirs.data_local_dir().join("calibration"),
                None => std::env::temp_dir().join("breadslicer-calibration"),
            }
        };
        let name = ctr!("Calibration", "XY calibration piece");
        let path = dir.join(format!("{name}.stl"));
        let written = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, Coupon::xy().to_stl()));
        if let Err(err) = written {
            eprintln!("calibration: the piece could not be written: {err}");
            return;
        }
        ui.global::<Calib>().set_open(false);
        (self.open_files)(vec![path]);
    }
}

/// Wires Tools → Calibrate resin.
pub(crate) fn wire(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
    prefs: Rc<PreferencesBus>,
    open_files: Rc<dyn Fn(Vec<PathBuf>)>,
) {
    let wizard = Rc::new(Wizard {
        window: ui.as_weak(),
        ctrl,
        prefs,
        open_files,
        texts: Rc::new(VecModel::from(vec![SharedString::new(); FIELDS])),
        bad: Rc::new(VecModel::from(vec![false; FIELDS])),
        values: RefCell::new([None; FIELDS]),
        pair: RefCell::new(None),
        pictures: RefCell::new(None),
    });
    let calib = ui.global::<Calib>();
    calib.set_texts(ModelRc::from(wizard.texts.clone()));
    calib.set_bad(ModelRc::from(wizard.bad.clone()));

    let w = wizard.clone();
    ui.on_calibration_requested(move || w.open());
    let w = wizard.clone();
    calib.on_edited(move |field, text| w.edited(field, text.as_str()));
    let w = wizard.clone();
    calib.on_show_result(move || w.show_result());
    let w = wizard.clone();
    calib.on_save(move || w.save());
    calib.on_place_coupon(move || wizard.place_coupon());
}

#[cfg(test)]
mod tests {
    use super::*;
    use bread_calibration::coupon::BLOCKS_MM;
    use bread_calibration::fit::JAW_HOLES;

    fn printed(scale: f32, offset: f32) -> [f32; 3] {
        BLOCKS_MM.map(|l| scale * l + 2.0 * offset)
    }

    #[test]
    fn a_record_round_trips_through_the_log() {
        let dir = std::env::temp_dir().join(format!("calibration-log-{}", std::process::id()));
        let path = dir.join("calibration-log.toml");
        let _ = std::fs::remove_file(&path);
        let record = CalibrationRecord {
            printer: "anycubic-photon-mono-m7-pro".into(),
            resin: "siraya-fast".into(),
            at: "2026-11-24 15:00".into(),
            temperature_c: 22.0,
            blocks_x: printed(1.004, 0.05),
            slit_x: -1.0,
            thinnest_pin: 1.0,
            ..Default::default()
        };
        record_to(&path, record.clone()).unwrap();
        let back = latest_of(&path, "anycubic-photon-mono-m7-pro", "siraya-fast").unwrap();
        assert_eq!(back, record);
        assert_eq!(back.readings().slit_x, Slit::AllClosed);
        assert!(latest_of(&path, "anycubic-photon-mono-m7-pro", "other").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_keeps_the_latest_readings_of_each_pair() {
        let dir = std::env::temp_dir().join(format!("calibration-cap-{}", std::process::id()));
        let path = dir.join("calibration-log.toml");
        let _ = std::fs::remove_file(&path);
        for k in 0..KEPT_PER_PAIR + 3 {
            let record = CalibrationRecord {
                printer: "p".into(),
                resin: "r".into(),
                at: format!("{k}"),
                ..Default::default()
            };
            record_to(&path, record).unwrap();
        }
        let records = read_log(&path);
        assert_eq!(records.len(), KEPT_PER_PAIR);
        assert_eq!(records.last().unwrap().at, format!("{}", KEPT_PER_PAIR + 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn walls_that_grow_ask_for_less_light_from_the_exposure_in_use() {
        let record = CalibrationRecord {
            blocks_x: printed(1.0, 0.08),
            blocks_y: printed(1.005, 0.07),
            ..Default::default()
        };
        let (heading, lines, advice) = describe(&record.readings().result(), false, Some(2.0));
        assert!(heading.contains("adjusting"), "{heading}");
        assert_eq!(lines.len(), 4);
        assert!(advice.contains("1.80 s instead of 2.00 s"), "{advice}");
        assert!(advice.contains("Y -0.50 %"), "{advice}");
    }

    #[test]
    fn the_coupon_reads_as_a_model_and_not_as_supports() {
        let coupon = Coupon::xy();
        let (scene, _) =
            sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(coupon.to_stl())).unwrap();
        let mesh = scene.leaf_mesh_arc(scene.roots()[0]).unwrap();
        assert!(mesh.is_sliceable(), "the coupon slices as it is");
        assert!(
            sc_mesh_core::presupported::find_supports(&mesh).is_none(),
            "nothing on the coupon reads as supports"
        );
    }

    #[test]
    fn the_card_reads_a_coupon_and_places_it_on_the_plate() {
        use crate::preferences_bus::PreferencesSnapshot;
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        use sc_print_primitives::{MaterialId, PrinterId};

        on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot {
                default_printer: Some(PrinterId::new("elegoo-saturn-4-ultra").expect("valid id")),
                default_resin: Some(
                    MaterialId::new("siraya-tech-blu-easy-grey").expect("valid id"),
                ),
                ..PreferencesSnapshot::for_tests()
            });
            let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&window));
            let opened = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
            let into = opened.clone();
            wire(
                &window,
                ctrl,
                bus,
                Rc::new(move |paths| into.borrow_mut().extend(paths)),
            );

            window.invoke_calibration_requested();
            let calib = window.global::<Calib>();
            assert!(calib.get_open());
            assert!(calib.get_can_save());
            assert_eq!(calib.get_page(), 0);
            assert_eq!(calib.get_pictures().row_count(), 7);

            // Typed with a comma or a point; a slip shows and is fixed.
            for (field, text) in [(1, "16,18"), (2, "8.18"), (3, "4,18"), (4, "16.16")] {
                calib.invoke_edited(field, text.into());
            }
            calib.invoke_edited(5, "8,1x".into());
            assert_eq!(calib.get_bad().row_data(5), Some(true));
            calib.invoke_edited(5, "8,16".into());
            assert_eq!(calib.get_bad().row_data(5), Some(false));
            calib.set_pin(1);
            calib.set_slit_x(4);
            // The holes read without drill bits: light, then the inside jaws.
            calib.set_hole(2);
            calib.invoke_edited(16, "2,86".into());
            calib.invoke_edited(17, "3,84".into());
            calib.invoke_show_result();
            assert_eq!(calib.get_result_verdict(), 2, "the walls grow 0.08–0.09 mm");
            let lines = calib.get_result_lines();
            assert!(
                lines
                    .iter()
                    .any(|l| l.label.contains("wall along X") && l.value.contains("+0.090")),
                "{:?}",
                lines.iter().collect::<Vec<_>>()
            );
            assert!(calib.get_result_advice().contains("light to spare"));
            assert!(
                lines
                    .iter()
                    .any(|l| l.label.contains("holes close") && l.value.contains("0.15")),
                "the inside jaws read how much the holes close"
            );
            assert!(
                lines
                    .iter()
                    .any(|l| l.label.contains("light comes through") && l.value.contains("1.0")),
                "the thinnest hole open is told"
            );
            calib.invoke_save();
            assert!(calib.get_saved());

            // Opening again starts over.
            window.invoke_calibration_requested();
            assert_eq!(calib.get_texts().row_data(1).as_deref(), Some(""));
            assert!(!calib.get_saved());

            calib.invoke_place_coupon();
            assert!(!calib.get_open());
            let paths = opened.borrow();
            assert_eq!(paths.len(), 1);
            assert!(
                paths[0].exists(),
                "the piece is written before it is opened"
            );
        });
    }

    /// With `BREADSLICER_UI_SHOTS` naming a folder, writes each page of the
    /// card, in English and Spanish.
    #[test]
    fn the_pictures_of_the_card_are_written_when_asked() {
        use crate::preferences_bus::PreferencesSnapshot;
        use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};

        let Some(dir) = std::env::var_os("BREADSLICER_UI_SHOTS").map(PathBuf::from) else {
            return;
        };
        std::fs::create_dir_all(&dir).expect("the folder is made");
        for language in ["en", "es"] {
            let dir = dir.clone();
            on_fresh_slint_thread(move || {
                let window = MainWindow::new().expect("a main window");
                slint::select_bundled_translation(language).expect("bundled language");
                window.show().expect("the window shows");
                window
                    .window()
                    .set_size(slint::LogicalSize::new(1600.0, 1000.0));
                let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
                let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&window));
                wire(&window, ctrl, bus, Rc::new(|_| {}));
                window.invoke_calibration_requested();
                let calib = window.global::<Calib>();
                for (field, text) in [
                    (1, "16,18"),
                    (2, "8,18"),
                    (3, "4,2"),
                    (4, "16,1"),
                    (5, "8,12"),
                    (14, "8,5"),
                    (15, "8,1"),
                ] {
                    calib.invoke_edited(field, text.into());
                }
                calib.set_pin(1);
                calib.set_hole(2);
                calib.invoke_edited(16, "2,86".into());
                calib.set_slit_x(1);
                calib.set_slit_y(2);
                for page in 0..=8 {
                    if page == 8 {
                        calib.invoke_show_result();
                    }
                    calib.set_page(page);
                    render_frames();
                    let shot = window.window().take_snapshot().expect("a snapshot");
                    image::RgbaImage::from_raw(
                        shot.width(),
                        shot.height(),
                        shot.as_bytes().to_vec(),
                    )
                    .expect("RGBA")
                    .save(dir.join(format!("{language}-calibration-{page}.png")))
                    .expect("the picture is written");
                }
            });
        }
    }

    #[test]
    fn hole_sizes_and_fields_line_up() {
        assert_eq!(HOLES_MM.len(), FOOT_BOTTOM - BITS);
        assert_eq!(FIELDS, HOLE_JAWS + JAW_HOLES.len());
        assert_eq!(
            JAW_HOLES.map(|k| HOLES_MM[k]),
            [3.0, 4.0],
            "the card says 3 and 4"
        );
    }
}
