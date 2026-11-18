/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The forms of Printer & resin (#151): a printer's or a resin's every
//! setting, each in a fixed slot, empty ones showing "—".
//!
//! A printer or a resin of the user's own is saved where it is. One that
//! ships is saved as the user's own copy, under a name of its own, and the
//! one that ships stays as it is. A resin keeps its settings per printer:
//! the form shows the block of the printer chosen first, and the others
//! the resin has, each a chip to switch to; a block the resin did not have
//! is added when any of its slots is filled.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use sc_print_primitives::{MaterialId, NonEmptyString, PrinterId};
use sc_print_profiles::{
    CalibrationTarget, MaterialIntent, MaterialPhysical, MaterialProfile, PrinterConstraints,
    PrinterProfile, PrinterTimeCalibration, save_printer_profile_to_library,
};
use sc_sla_formats::PrinterFileFormat;
use slint::{ModelRc, SharedString, VecModel};

use crate::i18n::ctr;
use crate::material_form::{
    CalibrationFormInput, DefaultsFormInput, EditedProfileKind, calibration_form_from_block,
    defaults_form_from, parse_calibration_form, parse_defaults_form, save_edited_profile,
};
use crate::print_format::format_label;
use crate::printer_form::{PrinterDraft, build_printer, draft_of, panel_of, trim_number};
use crate::profile_forking::{ForkParts, build_fork};
use crate::{FormField, FormSection};

/// A number as the form shows it: at most 4 decimals, no trailing zeros.
fn tidy(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// A number typed in a slot, tidied the same way; text that is no number
/// stays as typed.
fn tidy_text(text: &str) -> String {
    text.trim()
        .parse::<f64>()
        .map(tidy)
        .unwrap_or_else(|_| text.trim().to_owned())
}

fn number(key: &str, label: impl Into<SharedString>, unit: &str, value: &str) -> FormField {
    FormField {
        key: key.into(),
        label: label.into(),
        unit: unit.into(),
        value: value.into(),
        kind: 0,
        on: false,
        choices: ModelRc::default(),
        choice: 0,
    }
}

fn check(key: &str, label: impl Into<SharedString>, on: bool) -> FormField {
    FormField {
        kind: 1,
        on,
        ..number(key, label, "", "")
    }
}

fn choice(key: &str, label: impl Into<SharedString>, choices: &[String], at: usize) -> FormField {
    FormField {
        kind: 2,
        choices: ModelRc::new(VecModel::from(
            choices
                .iter()
                .map(|c| SharedString::from(c.as_str()))
                .collect::<Vec<_>>(),
        )),
        choice: at as i32,
        ..number(key, label, "", "")
    }
}

fn section(title: impl Into<SharedString>, fields: Vec<FormField>) -> FormSection {
    FormSection {
        title: title.into(),
        fields: ModelRc::new(VecModel::from(fields)),
    }
}

/// What a form shows, ready for the window.
pub(crate) struct Shown {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) chips: Vec<String>,
    pub(crate) chip: usize,
    pub(crate) left: Vec<FormSection>,
    pub(crate) right: Vec<FormSection>,
    pub(crate) summary: String,
    pub(crate) note: String,
    pub(crate) save_label: String,
}

/// A saved form: what it wrote, for the picker to choose it.
pub(crate) enum Saved {
    Printer(PrinterId),
    Resin(MaterialId),
}

/// A form being filled.
pub(crate) struct Form {
    kind: Kind,
    texts: HashMap<String, String>,
    checks: HashMap<String, bool>,
    picks: HashMap<String, usize>,
}

enum Kind {
    Printer {
        /// The printer the form started from.
        base: Option<PrinterProfile>,
        /// The user's own printer being changed, which keeps its id.
        editing: Option<PrinterId>,
        formats: Vec<PrinterFileFormat>,
        /// The latest prints exported for it (#45), whose real times fit
        /// its print time.
        prints: Vec<crate::print_log::PrintRecord>,
    },
    Resin {
        base: MaterialProfile,
        /// The user's own resin, saved where it is; otherwise a copy.
        mine: bool,
        /// The printers it has settings for, the one chosen first, with
        /// their names, and each block's slots as typed.
        targets: Vec<(CalibrationTarget, String, HashMap<String, String>)>,
        /// Which block the slots show.
        at: usize,
    },
}

/// The keys of a calibration block's slots.
const BLOCK_KEYS: [&str; 23] = [
    "normal_exposure_sec",
    "bottom_exposure_sec",
    "bottom_layer_count",
    "wait_before_cure_sec",
    "wait_after_cure_sec",
    "wait_after_lift_sec",
    "bottom_wait_before_cure_sec",
    "bottom_wait_after_cure_sec",
    "bottom_wait_after_lift_sec",
    "lift_distance_mm",
    "lift_speed_mm_min",
    "retract_speed_mm_min",
    "second_lift_distance_mm",
    "second_lift_speed_mm_min",
    "second_retract_speed_mm_min",
    "bottom_lift_distance_mm",
    "bottom_lift_speed_mm_min",
    "bottom_retract_speed_mm_min",
    "bottom_second_lift_distance_mm",
    "bottom_second_lift_speed_mm_min",
    "bottom_second_retract_speed_mm_min",
    "light_pwm",
    "bottom_light_pwm",
];

fn block_texts(input: &CalibrationFormInput) -> HashMap<String, String> {
    let i = input;
    [
        &i.normal_exposure_sec,
        &i.bottom_exposure_sec,
        &i.bottom_layer_count,
        &i.wait_before_cure_sec,
        &i.wait_after_cure_sec,
        &i.wait_after_lift_sec,
        &i.bottom_wait_before_cure_sec,
        &i.bottom_wait_after_cure_sec,
        &i.bottom_wait_after_lift_sec,
        &i.lift_distance_mm,
        &i.lift_speed_mm_min,
        &i.retract_speed_mm_min,
        &i.second_lift_distance_mm,
        &i.second_lift_speed_mm_min,
        &i.second_retract_speed_mm_min,
        &i.bottom_lift_distance_mm,
        &i.bottom_lift_speed_mm_min,
        &i.bottom_retract_speed_mm_min,
        &i.bottom_second_lift_distance_mm,
        &i.bottom_second_lift_speed_mm_min,
        &i.bottom_second_retract_speed_mm_min,
        &i.light_pwm,
        &i.bottom_light_pwm,
    ]
    .into_iter()
    .zip(BLOCK_KEYS)
    .map(|(v, k)| (k.to_owned(), tidy_text(v)))
    .collect()
}

fn block_input(texts: &HashMap<String, String>) -> CalibrationFormInput {
    let g = |k: &str| texts.get(k).cloned().unwrap_or_default();
    CalibrationFormInput {
        normal_exposure_sec: g("normal_exposure_sec"),
        bottom_exposure_sec: g("bottom_exposure_sec"),
        bottom_layer_count: g("bottom_layer_count"),
        wait_before_cure_sec: g("wait_before_cure_sec"),
        wait_after_cure_sec: g("wait_after_cure_sec"),
        wait_after_lift_sec: g("wait_after_lift_sec"),
        bottom_wait_before_cure_sec: g("bottom_wait_before_cure_sec"),
        bottom_wait_after_cure_sec: g("bottom_wait_after_cure_sec"),
        bottom_wait_after_lift_sec: g("bottom_wait_after_lift_sec"),
        lift_distance_mm: g("lift_distance_mm"),
        lift_speed_mm_min: g("lift_speed_mm_min"),
        retract_speed_mm_min: g("retract_speed_mm_min"),
        second_lift_distance_mm: g("second_lift_distance_mm"),
        second_lift_speed_mm_min: g("second_lift_speed_mm_min"),
        second_retract_speed_mm_min: g("second_retract_speed_mm_min"),
        bottom_lift_distance_mm: g("bottom_lift_distance_mm"),
        bottom_lift_speed_mm_min: g("bottom_lift_speed_mm_min"),
        bottom_retract_speed_mm_min: g("bottom_retract_speed_mm_min"),
        bottom_second_lift_distance_mm: g("bottom_second_lift_distance_mm"),
        bottom_second_lift_speed_mm_min: g("bottom_second_lift_speed_mm_min"),
        bottom_second_retract_speed_mm_min: g("bottom_second_retract_speed_mm_min"),
        light_pwm: g("light_pwm"),
        bottom_light_pwm: g("bottom_light_pwm"),
    }
}

/// The value at `path` of `value` serialised, as the form shows it.
fn read(value: &toml::Value, path: &[&str]) -> String {
    let mut at = value;
    for key in path {
        match at.get(key) {
            Some(next) => at = next,
            None => return String::new(),
        }
    }
    match at {
        toml::Value::Float(f) => tidy(*f),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

fn toml_of<T: serde::Serialize>(value: &T) -> toml::Value {
    toml::Value::try_from(value).unwrap_or(toml::Value::Table(toml::Table::new()))
}

impl Form {
    /// A printer's form: on `profile`, the user's own when `mine`, or a
    /// copy of it; with none, an empty form for a printer by hand.
    pub(crate) fn printer(profile: Option<PrinterProfile>, mine: bool) -> Form {
        let formats = crate::printer_form::writable_formats();
        let mut form = Form {
            kind: Kind::Printer {
                editing: profile
                    .as_ref()
                    .filter(|_| mine)
                    .map(|p| p.identity.id.clone()),
                base: profile.clone(),
                formats: formats.clone(),
                prints: match (&profile, crate::print_log::log_path()) {
                    (Some(p), Some(log)) => {
                        crate::print_log::latest_of(&log, p.identity.id.as_str())
                    }
                    _ => Vec::new(),
                },
            },
            texts: HashMap::new(),
            checks: HashMap::new(),
            picks: HashMap::new(),
        };
        let draft = profile.as_ref().map(draft_of).unwrap_or(PrinterDraft {
            grayscale: true,
            ..PrinterDraft::default()
        });
        let name = match (&profile, mine) {
            (Some(_), false) => ctr!("PrinterPicker", "{} (mine)", draft.name).to_string(),
            _ => draft.name.clone(),
        };
        let t = &mut form.texts;
        t.insert("maker".into(), draft.manufacturer.clone());
        t.insert(
            "family".into(),
            profile
                .as_ref()
                .map(|p| p.identity.family.as_str().to_owned())
                .unwrap_or_default(),
        );
        t.insert("name".into(), name);
        t.insert("res_x".into(), draft.resolution_x.clone());
        t.insert("res_y".into(), draft.resolution_y.clone());
        t.insert("px_x".into(), draft.pixel_x_um.clone());
        t.insert("px_y".into(), draft.pixel_y_um.clone());
        t.insert("plate_x".into(), draft.plate_x_mm.clone());
        t.insert("plate_y".into(), draft.plate_y_mm.clone());
        t.insert("height".into(), draft.height_mm.clone());
        if let Some(p) = &profile {
            let caps = &p.capabilities;
            t.insert(
                "transition".into(),
                caps.transition_layers
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            );
            t.insert(
                "bottom_count".into(),
                p.defaults
                    .bottom_layer_count
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            );
            let c = toml_of(&p.constraints);
            t.insert("z_step".into(), read(&c, &["z_step_mm"]));
            t.insert(
                "layer_min".into(),
                read(&c, &["layer_height_range_mm", "min"]),
            );
            t.insert(
                "layer_max".into(),
                read(&c, &["layer_height_range_mm", "max"]),
            );
            t.insert(
                "lift_min".into(),
                read(&c, &["lift_speed_range_mm_min", "min"]),
            );
            t.insert(
                "lift_max".into(),
                read(&c, &["lift_speed_range_mm_min", "max"]),
            );
            t.insert(
                "retract_min".into(),
                read(&c, &["retract_speed_range_mm_min", "min"]),
            );
            t.insert(
                "retract_max".into(),
                read(&c, &["retract_speed_range_mm_min", "max"]),
            );
            t.insert("lift_far".into(), read(&c, &["lift_distance_max_mm"]));
            t.insert("light".into(), read(&c, &["light_intensity_mw_per_cm2"]));
            let tc = toml_of(&p.time_calibration);
            t.insert("time_mult".into(), read(&tc, &["print_time_multiplier"]));
            t.insert("time_offset".into(), read(&tc, &["print_time_offset_sec"]));
            t.insert("time_layer".into(), read(&tc, &["per_layer_overhead_sec"]));
            t.insert("time_error".into(), read(&tc, &["print_time_error_sec"]));
            form.checks.insert("two_stage".into(), caps.two_stage_lift);
        }
        form.checks.insert("mirror_x".into(), draft.mirror_x);
        form.checks.insert("mirror_y".into(), draft.mirror_y);
        form.checks.insert("turned".into(), draft.turned);
        form.checks.insert("grey".into(), draft.grayscale);
        let at = draft
            .format
            .and_then(|f| formats.iter().position(|x| *x == f))
            .unwrap_or(0);
        form.picks.insert("format".into(), at);
        form
    }

    /// A resin's form on `profile`, the user's own when `mine`; its block
    /// for `chosen` (with the printers' names from `printer_name`) shows
    /// first.
    pub(crate) fn resin(
        profile: MaterialProfile,
        mine: bool,
        chosen: Option<&PrinterId>,
        printer_name: impl Fn(&PrinterId) -> String,
    ) -> Form {
        let mut targets: Vec<(CalibrationTarget, String, HashMap<String, String>)> = Vec::new();
        let label = |t: &CalibrationTarget| match t {
            CalibrationTarget::Generic => ctr!("PrinterPicker", "Any printer").to_string(),
            CalibrationTarget::Printer(id) => printer_name(id.as_printer_id()),
        };
        if let Some(id) = chosen {
            let target = CalibrationTarget::from_printer(id);
            let texts = profile
                .per_printer
                .get(&target)
                .map(|b| block_texts(&calibration_form_from_block(b)))
                .unwrap_or_default();
            targets.push((target.clone(), label(&target), texts));
        }
        for (target, block) in &profile.per_printer {
            if targets.iter().any(|(t, _, _)| t == target) {
                continue;
            }
            targets.push((
                target.clone(),
                label(target),
                block_texts(&calibration_form_from_block(block)),
            ));
        }
        let mut texts = HashMap::new();
        let d = defaults_form_from(&profile.defaults);
        for (k, v) in [
            ("d_scale_x", &d.scale_x_pct),
            ("d_scale_y", &d.scale_y_pct),
            ("d_scale_z", &d.scale_z_pct),
            ("d_bottom_count", &d.bottom_layer_count),
            ("d_wait_before", &d.wait_before_cure_sec),
            ("d_wait_after", &d.wait_after_cure_sec),
            ("d_wait_lift", &d.wait_after_lift_sec),
            ("d_light", &d.light_pwm),
            ("d_bottom_light", &d.bottom_light_pwm),
        ] {
            texts.insert(k.to_owned(), tidy_text(v));
        }
        let intent = toml_of(&profile.intent);
        texts.insert(
            "layer_height".into(),
            read(&intent, &["target_layer_height_mm"]),
        );
        texts.insert(
            "energy".into(),
            read(&intent, &["target_energy_density_mj_per_cm2"]),
        );
        let physical = toml_of(&profile.physical);
        texts.insert(
            "liquid_density".into(),
            read(&physical, &["liquid_density_g_per_cm3"]),
        );
        texts.insert(
            "cured_density".into(),
            read(&physical, &["cured_density_g_per_cm3"]),
        );
        texts.insert("brand".into(), profile.identity.brand.as_str().to_owned());
        texts.insert(
            "rname".into(),
            if mine {
                profile.identity.name.as_str().to_owned()
            } else {
                ctr!("PrinterPicker", "{} (mine)", profile.identity.name.as_str()).to_string()
            },
        );
        if let Some((_, _, block)) = targets.first() {
            texts.extend(block.clone());
        }
        Form {
            kind: Kind::Resin {
                base: profile,
                mine,
                targets,
                at: 0,
            },
            texts,
            checks: HashMap::new(),
            picks: HashMap::new(),
        }
    }

    pub(crate) fn edited(&mut self, key: &str, text: &str) {
        self.texts.insert(key.to_owned(), text.to_owned());
    }

    pub(crate) fn toggled(&mut self, key: &str, on: bool) {
        self.checks.insert(key.to_owned(), on);
    }

    pub(crate) fn picked(&mut self, key: &str, at: usize) {
        self.picks.insert(key.to_owned(), at);
    }

    /// Shows the block of chip `at`, keeping what the one shown holds.
    pub(crate) fn chip(&mut self, at: usize) {
        let Kind::Resin {
            targets, at: shown, ..
        } = &mut self.kind
        else {
            return;
        };
        if at >= targets.len() || at == *shown {
            return;
        }
        let kept: HashMap<String, String> = BLOCK_KEYS
            .iter()
            .map(|k| {
                (
                    (*k).to_owned(),
                    self.texts.get(*k).cloned().unwrap_or_default(),
                )
            })
            .collect();
        targets[*shown].2 = kept;
        *shown = at;
        let next = targets[at].2.clone();
        for k in BLOCK_KEYS {
            self.texts
                .insert(k.to_owned(), next.get(k).cloned().unwrap_or_default());
        }
    }

    fn t(&self, key: &str) -> String {
        self.texts.get(key).cloned().unwrap_or_default()
    }

    fn c(&self, key: &str) -> bool {
        self.checks.get(key).copied().unwrap_or(false)
    }

    /// What the window shows of the form.
    pub(crate) fn shown(&self) -> Shown {
        match &self.kind {
            Kind::Printer {
                base,
                editing,
                formats,
                ..
            } => self.shown_printer(base.as_ref(), editing.is_some(), formats),
            Kind::Resin {
                base,
                mine,
                targets,
                at,
            } => self.shown_resin(base, *mine, targets, *at),
        }
    }

    fn shown_printer(
        &self,
        base: Option<&PrinterProfile>,
        mine: bool,
        formats: &[PrinterFileFormat],
    ) -> Shown {
        // Numbers show tidied; what is not typed over saves as it was.
        let n = |key: &str, label: SharedString, unit: &str| {
            number(key, label, unit, &tidy_text(&self.t(key)))
        };
        let names: Vec<String> = formats.iter().map(|f| format_label(*f)).collect();
        let left = vec![
            section(
                ctr!("PrinterPicker", "PRINTER"),
                vec![
                    n("maker", ctr!("PrinterPicker", "Maker"), ""),
                    n("family", ctr!("PrinterPicker", "Family"), ""),
                    n("name", ctr!("PrinterPicker", "Model"), ""),
                ],
            ),
            section(
                ctr!("PrinterPicker", "SCREEN"),
                vec![
                    n("res_x", ctr!("PrinterPicker", "Pixels across (X)"), "px"),
                    n("res_y", ctr!("PrinterPicker", "Pixels deep (Y)"), "px"),
                    n("px_x", ctr!("PrinterPicker", "Pixel size X"), "µm"),
                    n("px_y", ctr!("PrinterPicker", "Pixel size Y"), "µm"),
                    n("plate_x", ctr!("PrinterPicker", "Plate X"), "mm"),
                    n("plate_y", ctr!("PrinterPicker", "Plate Y"), "mm"),
                    n("height", ctr!("PrinterPicker", "Build height"), "mm"),
                ],
            ),
            section(
                ctr!("PrinterPicker", "FILE"),
                vec![
                    choice(
                        "format",
                        ctr!("PrinterPicker", "File format"),
                        &names,
                        self.picks.get("format").copied().unwrap_or(0),
                    ),
                    check(
                        "mirror_x",
                        ctr!("PrinterPicker", "Mirror the image in X"),
                        self.c("mirror_x"),
                    ),
                    check(
                        "mirror_y",
                        ctr!("PrinterPicker", "Mirror the image in Y"),
                        self.c("mirror_y"),
                    ),
                    check(
                        "turned",
                        ctr!("PrinterPicker", "Image turned a quarter"),
                        self.c("turned"),
                    ),
                    check(
                        "grey",
                        ctr!("PrinterPicker", "Grey levels (anti-aliasing)"),
                        self.c("grey"),
                    ),
                ],
            ),
        ];
        let right = vec![
            section(
                ctr!("PrinterPicker", "MOVES"),
                vec![
                    check(
                        "two_stage",
                        ctr!("PrinterPicker", "Lifts in 2 stages"),
                        self.c("two_stage"),
                    ),
                    n("transition", ctr!("PrinterPicker", "Transition layers"), ""),
                    n("bottom_count", ctr!("PrinterPicker", "Bottom layers"), ""),
                ],
            ),
            section(
                ctr!("PrinterPicker", "LIMITS"),
                vec![
                    n("z_step", ctr!("PrinterPicker", "Z step"), "mm"),
                    n("layer_min", ctr!("PrinterPicker", "Thinnest layer"), "mm"),
                    n("layer_max", ctr!("PrinterPicker", "Thickest layer"), "mm"),
                    n("lift_min", ctr!("PrinterPicker", "Slowest lift"), "mm/min"),
                    n("lift_max", ctr!("PrinterPicker", "Fastest lift"), "mm/min"),
                    n(
                        "retract_min",
                        ctr!("PrinterPicker", "Slowest retract"),
                        "mm/min",
                    ),
                    n(
                        "retract_max",
                        ctr!("PrinterPicker", "Fastest retract"),
                        "mm/min",
                    ),
                    n("lift_far", ctr!("PrinterPicker", "Highest lift"), "mm"),
                    n("light", ctr!("PrinterPicker", "Light intensity"), "mW/cm²"),
                ],
            ),
            section(
                ctr!("PrinterPicker", "PRINT TIME"),
                vec![
                    n("time_mult", ctr!("PrinterPicker", "Time multiplier"), "×"),
                    n("time_offset", ctr!("PrinterPicker", "Time per print"), "s"),
                    n("time_layer", ctr!("PrinterPicker", "Time per layer"), "s"),
                ],
            ),
        ];
        let mut right = right;
        // The prints exported for it (#45): how long each really took fits
        // its print time.
        if !self.prints().is_empty() {
            let fields = self
                .prints()
                .iter()
                .enumerate()
                .map(|(i, print)| {
                    let key = format!("real_{i}");
                    number(
                        &key,
                        ctr!(
                            "PrinterPicker",
                            "{} · {} · estimated {}",
                            print.file,
                            print.at,
                            crate::export_print_file::format_duration(print.estimate_sec)
                        ),
                        "h:mm",
                        &self.t(&key),
                    )
                })
                .collect();
            right.push(section(
                ctr!("PrinterPicker", "HOW LONG YOUR PRINTS TOOK"),
                fields,
            ));
        }
        let draft = self.draft(formats);
        let summary = match panel_of(&draft) {
            (Some(x), Some(y)) => ctr!(
                "PrinterDialog",
                "Plate {} × {} mm, pixels of {} × {} µm.",
                trim_number(x.plate_mm),
                trim_number(y.plate_mm),
                trim_number(x.pixel_um),
                trim_number(y.pixel_um)
            )
            .to_string(),
            _ => String::new(),
        };
        let fit = self.time_fit_line();
        let summary = match (summary.is_empty(), fit.is_empty()) {
            (_, true) => summary,
            (true, false) => fit,
            (false, false) => format!("{summary}\n{fit}"),
        };
        let title = match base {
            Some(p) if mine => format!(
                "{} {}",
                p.identity.manufacturer.as_str(),
                p.identity.name.as_str()
            ),
            Some(p) => ctr!(
                "PrinterPicker",
                "A printer of your own from {} {}",
                p.identity.manufacturer.as_str(),
                p.identity.name.as_str()
            )
            .to_string(),
            None => ctr!("PrinterPicker", "A printer by hand").to_string(),
        };
        Shown {
            title,
            subtitle: ctr!("PrinterPicker", "Every setting of the printer").to_string(),
            chips: Vec::new(),
            chip: 0,
            left,
            right,
            summary,
            note: if mine {
                String::new()
            } else {
                ctr!(
                    "PrinterPicker",
                    "Give the pixel size or the plate size: the other one follows. Saving makes it a printer of your own; the one that ships stays as it is."
                )
                .to_string()
            },
            save_label: if mine {
                ctr!("PrinterPicker", "Save")
            } else {
                ctr!("PrinterPicker", "Save as my printer")
            }
            .to_string(),
        }
    }

    fn shown_resin(
        &self,
        base: &MaterialProfile,
        mine: bool,
        targets: &[(CalibrationTarget, String, HashMap<String, String>)],
        at: usize,
    ) -> Shown {
        let n = |key: &str, label: SharedString, unit: &str| number(key, label, unit, &self.t(key));
        let left = vec![
            section(
                ctr!("PrinterPicker", "RESIN"),
                vec![
                    n("brand", ctr!("PrinterPicker", "Brand"), ""),
                    n("rname", ctr!("PrinterPicker", "Name"), ""),
                ],
            ),
            section(
                ctr!("PrinterPicker", "LAYERS"),
                vec![
                    n(
                        "normal_exposure_sec",
                        ctr!("PrinterPicker", "Exposure"),
                        "s",
                    ),
                    n("light_pwm", ctr!("PrinterPicker", "Light power"), "PWM"),
                ],
            ),
            section(
                ctr!("PrinterPicker", "LIFT & RETRACT"),
                vec![
                    n(
                        "lift_distance_mm",
                        ctr!("PrinterPicker", "Lift distance"),
                        "mm",
                    ),
                    n(
                        "lift_speed_mm_min",
                        ctr!("PrinterPicker", "Lift speed"),
                        "mm/min",
                    ),
                    n(
                        "retract_speed_mm_min",
                        ctr!("PrinterPicker", "Retract speed"),
                        "mm/min",
                    ),
                    n(
                        "second_lift_distance_mm",
                        ctr!("PrinterPicker", "2nd stage distance"),
                        "mm",
                    ),
                    n(
                        "second_lift_speed_mm_min",
                        ctr!("PrinterPicker", "2nd stage lift speed"),
                        "mm/min",
                    ),
                    n(
                        "second_retract_speed_mm_min",
                        ctr!("PrinterPicker", "2nd stage retract speed"),
                        "mm/min",
                    ),
                ],
            ),
            section(
                ctr!("PrinterPicker", "WAITS"),
                vec![
                    n(
                        "wait_before_cure_sec",
                        ctr!("PrinterPicker", "Before cure"),
                        "s",
                    ),
                    n(
                        "wait_after_cure_sec",
                        ctr!("PrinterPicker", "After cure"),
                        "s",
                    ),
                    n(
                        "wait_after_lift_sec",
                        ctr!("PrinterPicker", "After lift"),
                        "s",
                    ),
                ],
            ),
        ];
        let right = vec![
            section(
                ctr!("PrinterPicker", "BOTTOM LAYERS"),
                vec![
                    n("bottom_layer_count", ctr!("PrinterPicker", "Count"), ""),
                    n(
                        "bottom_exposure_sec",
                        ctr!("PrinterPicker", "Exposure"),
                        "s",
                    ),
                    n(
                        "bottom_light_pwm",
                        ctr!("PrinterPicker", "Light power"),
                        "PWM",
                    ),
                    n(
                        "bottom_lift_distance_mm",
                        ctr!("PrinterPicker", "Lift distance"),
                        "mm",
                    ),
                    n(
                        "bottom_lift_speed_mm_min",
                        ctr!("PrinterPicker", "Lift speed"),
                        "mm/min",
                    ),
                    n(
                        "bottom_retract_speed_mm_min",
                        ctr!("PrinterPicker", "Retract speed"),
                        "mm/min",
                    ),
                    n(
                        "bottom_second_lift_distance_mm",
                        ctr!("PrinterPicker", "2nd stage distance"),
                        "mm",
                    ),
                    n(
                        "bottom_second_lift_speed_mm_min",
                        ctr!("PrinterPicker", "2nd stage lift speed"),
                        "mm/min",
                    ),
                    n(
                        "bottom_second_retract_speed_mm_min",
                        ctr!("PrinterPicker", "2nd stage retract speed"),
                        "mm/min",
                    ),
                    n(
                        "bottom_wait_before_cure_sec",
                        ctr!("PrinterPicker", "Wait before cure"),
                        "s",
                    ),
                    n(
                        "bottom_wait_after_cure_sec",
                        ctr!("PrinterPicker", "Wait after cure"),
                        "s",
                    ),
                    n(
                        "bottom_wait_after_lift_sec",
                        ctr!("PrinterPicker", "Wait after lift"),
                        "s",
                    ),
                ],
            ),
            section(
                ctr!("PrinterPicker", "ON EVERY PRINTER"),
                vec![
                    n("d_scale_x", ctr!("PrinterPicker", "Scale X"), "%"),
                    n("d_scale_y", ctr!("PrinterPicker", "Scale Y"), "%"),
                    n("d_scale_z", ctr!("PrinterPicker", "Scale Z"), "%"),
                    n("layer_height", ctr!("PrinterPicker", "Layer height"), "mm"),
                    n("energy", ctr!("PrinterPicker", "Energy"), "mJ/cm²"),
                    n(
                        "liquid_density",
                        ctr!("PrinterPicker", "Density, liquid"),
                        "g/cm³",
                    ),
                    n(
                        "cured_density",
                        ctr!("PrinterPicker", "Density, cured"),
                        "g/cm³",
                    ),
                ],
            ),
            section(
                ctr!("PrinterPicker", "WHERE A PRINTER HAS NONE"),
                vec![
                    n("d_bottom_count", ctr!("PrinterPicker", "Bottom layers"), ""),
                    n(
                        "d_wait_before",
                        ctr!("PrinterPicker", "Wait before cure"),
                        "s",
                    ),
                    n(
                        "d_wait_after",
                        ctr!("PrinterPicker", "Wait after cure"),
                        "s",
                    ),
                    n("d_wait_lift", ctr!("PrinterPicker", "Wait after lift"), "s"),
                    n("d_light", ctr!("PrinterPicker", "Light power"), "PWM"),
                    n(
                        "d_bottom_light",
                        ctr!("PrinterPicker", "Bottom light power"),
                        "PWM",
                    ),
                ],
            ),
        ];
        Shown {
            title: format!(
                "{} {}",
                base.identity.brand.as_str(),
                base.identity.name.as_str()
            ),
            subtitle: targets
                .get(at)
                .map(|(_, name, _)| ctr!("PrinterPicker", "Settings for {}", name).to_string())
                .unwrap_or_default(),
            chips: targets.iter().map(|(_, name, _)| name.clone()).collect(),
            chip: at,
            left,
            right,
            summary: String::new(),
            note: if mine {
                ctr!(
                    "PrinterPicker",
                    "Every slot is always there; an empty one shows “—”. The printer above picks whose settings show."
                )
                .to_string()
            } else {
                ctr!(
                    "PrinterPicker",
                    "From the maker's published settings. Saving makes it a resin of your own; the one that ships stays as it is."
                )
                .to_string()
            },
            save_label: if mine {
                ctr!("PrinterPicker", "Save")
            } else {
                ctr!("PrinterPicker", "Save as my resin")
            }
            .to_string(),
        }
    }

    fn draft(&self, formats: &[PrinterFileFormat]) -> PrinterDraft {
        PrinterDraft {
            manufacturer: self.t("maker"),
            name: self.t("name"),
            resolution_x: self.t("res_x"),
            resolution_y: self.t("res_y"),
            pixel_x_um: self.t("px_x"),
            pixel_y_um: self.t("px_y"),
            plate_x_mm: self.t("plate_x"),
            plate_y_mm: self.t("plate_y"),
            height_mm: self.t("height"),
            mirror_x: self.c("mirror_x"),
            mirror_y: self.c("mirror_y"),
            turned: self.c("turned"),
            grayscale: self.c("grey"),
            format: formats
                .get(self.picks.get("format").copied().unwrap_or(0))
                .copied(),
        }
    }

    /// Saves the form to the user's library: `printers` and `materials`
    /// are the ids that exist. The error is the line the form shows.
    pub(crate) fn save(
        &mut self,
        library: &Path,
        printers: &BTreeSet<PrinterId>,
        materials: &BTreeSet<MaterialId>,
    ) -> Result<Saved, String> {
        match &self.kind {
            Kind::Printer { .. } => {
                if let Some(fit) = self.time_fit() {
                    self.apply_time_fit(fit);
                }
                let (base, editing, formats) = match &self.kind {
                    Kind::Printer {
                        base,
                        editing,
                        formats,
                        ..
                    } => (base, editing, formats),
                    Kind::Resin { .. } => unreachable!("matched a printer"),
                };
                let printer =
                    self.built_printer(base.as_ref(), editing.as_ref(), formats, printers)?;
                save_printer_profile_to_library(library, &printer).map_err(|e| e.to_string())?;
                Ok(Saved::Printer(printer.identity.id))
            }
            Kind::Resin { .. } => {
                // The block shown as typed.
                if let Kind::Resin { at, .. } = &self.kind {
                    let at = *at;
                    self.chip_keep(at);
                }
                let resin = self.built_resin(materials)?;
                save_edited_profile(library, &resin, EditedProfileKind::Full)?;
                Ok(Saved::Resin(resin.identity.id))
            }
        }
    }

    /// The prints offered to time, as the form holds them.
    fn prints(&self) -> &[crate::print_log::PrintRecord] {
        match &self.kind {
            Kind::Printer { prints, .. } => prints,
            Kind::Resin { .. } => &[],
        }
    }

    /// What the real times typed for the prints say (#45).
    fn time_fit(&self) -> Option<crate::print_log::TimeFit> {
        let points: Vec<(f64, u32, f64)> = self
            .prints()
            .iter()
            .enumerate()
            .filter_map(|(i, print)| {
                let real = crate::print_log::parse_duration(&self.t(&format!("real_{i}")))?;
                Some((print.schedule_sec, print.layers, real))
            })
            .collect();
        crate::print_log::fit(&points)
    }

    /// The fit into the print time's fields: it replaces what they held.
    fn apply_time_fit(&mut self, fit: crate::print_log::TimeFit) {
        let tidy3 = |v: f64| tidy((v * 1000.0).round() / 1000.0);
        self.texts.insert("time_mult".into(), String::new());
        self.texts
            .insert("time_offset".into(), tidy3(fit.per_print_sec));
        self.texts
            .insert("time_layer".into(), tidy3(fit.per_layer_sec));
        self.texts.insert(
            "time_error".into(),
            fit.error_sec.map(|e| tidy(e.round())).unwrap_or_default(),
        );
    }

    /// The line on what the typed times make.
    fn time_fit_line(&self) -> String {
        if self.prints().is_empty() {
            return String::new();
        }
        let typed = (0..self.prints().len())
            .filter(|i| !self.t(&format!("real_{i}")).trim().is_empty())
            .count();
        match self.time_fit() {
            Some(fit) => {
                let range = fit
                    .error_sec
                    .map(|e| {
                        ctr!(
                            "PrinterPicker",
                            ", within ± {}",
                            crate::export_print_file::format_duration(e.max(1.0))
                        )
                        .to_string()
                    })
                    .unwrap_or_default();
                ctr!(
                    "PrinterPicker",
                    "From {} of your prints: {} s more a layer and {} more a print{}. Saving keeps it as the print time.",
                    fit.prints,
                    tidy((fit.per_layer_sec * 100.0).round() / 100.0),
                    crate::export_print_file::format_duration(fit.per_print_sec.abs()),
                    range
                )
                .to_string()
            }
            None if typed > 0 => ctr!(
                "PrinterPicker",
                "Write how long at least 2 prints of different heights took."
            )
            .to_string(),
            None => String::new(),
        }
    }

    fn chip_keep(&mut self, at: usize) {
        let kept: HashMap<String, String> = BLOCK_KEYS
            .iter()
            .map(|k| ((*k).to_owned(), self.t(k)))
            .collect();
        if let Kind::Resin { targets, .. } = &mut self.kind
            && let Some(target) = targets.get_mut(at)
        {
            target.2 = kept;
        }
    }

    fn built_printer(
        &self,
        base: Option<&PrinterProfile>,
        editing: Option<&PrinterId>,
        formats: &[PrinterFileFormat],
        taken: &BTreeSet<PrinterId>,
    ) -> Result<PrinterProfile, String> {
        let mut printer =
            build_printer(&self.draft(formats), base, editing, taken).map_err(|e| e.to_string())?;
        if let Ok(family) = NonEmptyString::new(self.t("family").trim()) {
            printer.identity.family = family;
        }
        printer.capabilities.two_stage_lift = self.c("two_stage");
        printer.capabilities.transition_layers =
            self.count("transition", ctr!("PrinterPicker", "Transition layers"))?;
        printer.defaults.bottom_layer_count =
            self.count("bottom_count", ctr!("PrinterPicker", "Bottom layers"))?;
        let mut constraints = toml::Table::new();
        self.put(
            &mut constraints,
            "z_step_mm",
            "z_step",
            ctr!("PrinterPicker", "Z step"),
        )?;
        self.put_range(
            &mut constraints,
            "layer_height_range_mm",
            ("layer_min", "layer_max"),
            ctr!("PrinterPicker", "Layer thickness"),
        )?;
        self.put_range(
            &mut constraints,
            "lift_speed_range_mm_min",
            ("lift_min", "lift_max"),
            ctr!("PrinterPicker", "Lift speed"),
        )?;
        self.put_range(
            &mut constraints,
            "retract_speed_range_mm_min",
            ("retract_min", "retract_max"),
            ctr!("PrinterPicker", "Retract speed"),
        )?;
        self.put(
            &mut constraints,
            "lift_distance_max_mm",
            "lift_far",
            ctr!("PrinterPicker", "Highest lift"),
        )?;
        self.put(
            &mut constraints,
            "light_intensity_mw_per_cm2",
            "light",
            ctr!("PrinterPicker", "Light intensity"),
        )?;
        printer.constraints = toml::Value::Table(constraints)
            .try_into::<PrinterConstraints>()
            .map_err(|e| {
                ctr!("PrinterPicker", "The limits do not hold: {}", e.message()).to_string()
            })?;
        let mut time = toml::Table::new();
        self.put(
            &mut time,
            "print_time_multiplier",
            "time_mult",
            ctr!("PrinterPicker", "Time multiplier"),
        )?;
        self.put(
            &mut time,
            "print_time_offset_sec",
            "time_offset",
            ctr!("PrinterPicker", "Time per print"),
        )?;
        self.put(
            &mut time,
            "per_layer_overhead_sec",
            "time_layer",
            ctr!("PrinterPicker", "Time per layer"),
        )?;
        self.put(
            &mut time,
            "print_time_error_sec",
            "time_error",
            ctr!("PrinterPicker", "Time per print"),
        )?;
        printer.time_calibration = toml::Value::Table(time)
            .try_into::<PrinterTimeCalibration>()
            .map_err(|e| {
                ctr!(
                    "PrinterPicker",
                    "The print time does not hold: {}",
                    e.message()
                )
                .to_string()
            })?;
        Ok(printer)
    }

    /// A whole number of slot `key`, none when empty.
    fn count(&self, key: &str, label: SharedString) -> Result<Option<u32>, String> {
        let text = self.t(key);
        if text.trim().is_empty() {
            return Ok(None);
        }
        text.trim()
            .parse::<u32>()
            .map(Some)
            .map_err(|_| ctr!("PrinterPicker", "{} is not a whole number.", label).to_string())
    }

    fn real(&self, key: &str, label: &SharedString) -> Result<Option<f64>, String> {
        let text = self.t(key);
        if text.trim().is_empty() {
            return Ok(None);
        }
        text.trim()
            .parse::<f64>()
            .map(Some)
            .map_err(|_| ctr!("PrinterPicker", "{} is not a number.", label).to_string())
    }

    fn put(
        &self,
        table: &mut toml::Table,
        name: &str,
        key: &str,
        label: SharedString,
    ) -> Result<(), String> {
        if let Some(v) = self.real(key, &label)? {
            table.insert(name.to_owned(), toml::Value::Float(v));
        }
        Ok(())
    }

    fn put_range(
        &self,
        table: &mut toml::Table,
        name: &str,
        (lo, hi): (&str, &str),
        label: SharedString,
    ) -> Result<(), String> {
        match (self.real(lo, &label)?, self.real(hi, &label)?) {
            (None, None) => Ok(()),
            (Some(min), Some(max)) => {
                let mut range = toml::Table::new();
                range.insert("min".into(), toml::Value::Float(min));
                range.insert("max".into(), toml::Value::Float(max));
                table.insert(name.to_owned(), toml::Value::Table(range));
                Ok(())
            }
            _ => Err(ctr!("PrinterPicker", "{} takes both ends, or neither.", label).to_string()),
        }
    }

    fn built_resin(&self, existing: &BTreeSet<MaterialId>) -> Result<MaterialProfile, String> {
        let Kind::Resin {
            base,
            mine,
            targets,
            ..
        } = &self.kind
        else {
            return Err(String::new());
        };
        let mut per_printer = base.per_printer.clone();
        for (target, _, texts) in targets {
            let filled = texts.values().any(|v| !v.trim().is_empty());
            if !filled && !base.per_printer.contains_key(target) {
                continue;
            }
            let block = parse_calibration_form(&block_input(texts))
                .map_err(|e| e.user_message().to_string())?;
            per_printer.insert(target.clone(), block);
        }
        let defaults = parse_defaults_form(&DefaultsFormInput {
            scale_x_pct: self.t("d_scale_x"),
            scale_y_pct: self.t("d_scale_y"),
            scale_z_pct: self.t("d_scale_z"),
            bottom_layer_count: self.t("d_bottom_count"),
            wait_before_cure_sec: self.t("d_wait_before"),
            wait_after_cure_sec: self.t("d_wait_after"),
            wait_after_lift_sec: self.t("d_wait_lift"),
            light_pwm: self.t("d_light"),
            bottom_light_pwm: self.t("d_bottom_light"),
        })
        .map_err(|e| e.user_message().to_string())?;
        let mut intent = toml::Table::new();
        self.put(
            &mut intent,
            "target_layer_height_mm",
            "layer_height",
            ctr!("PrinterPicker", "Layer height"),
        )?;
        self.put(
            &mut intent,
            "target_energy_density_mj_per_cm2",
            "energy",
            ctr!("PrinterPicker", "Energy"),
        )?;
        let intent: MaterialIntent = toml::Value::Table(intent).try_into().map_err(|e| {
            ctr!("PrinterPicker", "The resin does not hold: {}", e.message()).to_string()
        })?;
        let mut physical = toml::Table::new();
        self.put(
            &mut physical,
            "liquid_density_g_per_cm3",
            "liquid_density",
            ctr!("PrinterPicker", "Density, liquid"),
        )?;
        self.put(
            &mut physical,
            "cured_density_g_per_cm3",
            "cured_density",
            ctr!("PrinterPicker", "Density, cured"),
        )?;
        let physical: MaterialPhysical = toml::Value::Table(physical).try_into().map_err(|e| {
            ctr!("PrinterPicker", "The resin does not hold: {}", e.message()).to_string()
        })?;
        let mut resin = if *mine {
            let mut resin = base.clone();
            resin.identity.brand = NonEmptyString::new(self.t("brand").trim())
                .map_err(|_| ctr!("PrinterPicker", "The resin needs a brand.").to_string())?;
            resin.identity.name = NonEmptyString::new(self.t("rname").trim())
                .map_err(|_| ctr!("PrinterPicker", "The resin needs a name.").to_string())?;
            resin
        } else {
            build_fork(
                base,
                &ForkParts {
                    intent: true,
                    defaults: true,
                    blocks: base.per_printer.keys().cloned().collect(),
                },
                &self.t("rname"),
                &self.t("brand"),
                existing,
            )
            .map_err(|e| e.user_message().to_string())?
        };
        resin.per_printer = per_printer;
        resin.defaults = defaults;
        resin.intent = intent;
        resin.physical = physical;
        Ok(resin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_print_profiles::ProfileLibraries;

    /// How long the prints exported for a printer really took fit its
    /// print time (#45): what each layer and each print cost on top of the
    /// schedule, and how far the prints came from it.
    #[test]
    fn the_real_times_of_its_prints_fit_a_printer_s_print_time() {
        let base = m7_pro();
        let mut form = Form::printer(Some(base.clone()), true);
        // 1.2 s a layer and 150 s a print on top of each schedule.
        let prints: Vec<(f64, u32)> = vec![(2000.0, 300), (5000.0, 800), (8000.0, 1500)];
        if let Kind::Printer {
            prints: offered, ..
        } = &mut form.kind
        {
            *offered = prints
                .iter()
                .map(|&(schedule_sec, layers)| crate::print_log::PrintRecord {
                    printer: base.identity.id.as_str().to_owned(),
                    file: format!("{layers}.pwsz"),
                    at: "2026-11-18 14:05".into(),
                    layers,
                    schedule_sec,
                    estimate_sec: schedule_sec,
                })
                .collect();
        }
        let shown = form.shown();
        let keys: Vec<String> = shown
            .right
            .iter()
            .flat_map(|s| slint::Model::iter(&s.fields).map(|f| f.key.to_string()))
            .collect();
        assert!(keys.iter().any(|k| k == "real_2"), "{keys:?}");
        for (i, &(schedule, layers)) in prints.iter().enumerate() {
            let real = schedule + 1.2 * f64::from(layers) + 150.0;
            form.edited(&format!("real_{i}"), &format!("{}", (real / 60.0)));
        }
        assert!(
            form.shown().summary.contains("3"),
            "{}",
            form.shown().summary
        );
        let dir = tempfile::tempdir().unwrap();
        let Saved::Printer(id) = form
            .save(dir.path(), &BTreeSet::new(), &BTreeSet::new())
            .unwrap()
        else {
            panic!("a printer");
        };
        let saved = ProfileLibraries::new(Some(dir.path().into()), Vec::new())
            .load_printer_by_id(&id)
            .unwrap();
        let time = saved.time_calibration;
        assert_eq!(time.print_time_multiplier, None);
        assert!((time.per_layer_overhead_sec.unwrap().value() - 1.2).abs() < 1e-3);
        assert!((time.print_time_offset_sec.unwrap().value() - 150.0).abs() < 1e-2);
        assert!(time.print_time_error_sec.unwrap().value() < 1.0);
    }

    fn m7_pro() -> PrinterProfile {
        ProfileLibraries::platform_default()
            .load_printer_by_id(&PrinterId::new("anycubic-photon-mono-m7-pro").unwrap())
            .unwrap()
    }

    /// A printer that ships, saved, becomes a printer of the user's own
    /// with every slot as it was, and what was changed.
    #[test]
    fn a_printer_s_every_setting_comes_back_and_a_change_is_kept() {
        let base = m7_pro();
        let mut form = Form::printer(Some(base.clone()), false);
        let shown = form.shown();
        let fields: Vec<String> = shown
            .left
            .iter()
            .chain(&shown.right)
            .flat_map(|s| slint::Model::iter(&s.fields).map(|f| f.key.to_string()))
            .collect();
        for key in [
            "two_stage",
            "transition",
            "time_layer",
            "z_step",
            "lift_far",
        ] {
            assert!(fields.iter().any(|k| k == key), "{key} in {fields:?}");
        }
        assert_eq!(form.t("transition"), "6");
        assert_eq!(form.t("time_layer"), "1.14");
        assert_eq!(tidy_text(&form.t("px_x")), "16.8", "shown tidied");
        form.edited("transition", "4");
        form.edited("z_step", "0.01");
        let dir = tempfile::tempdir().unwrap();
        let taken: BTreeSet<PrinterId> = [base.identity.id.clone()].into();
        let Saved::Printer(id) = form.save(dir.path(), &taken, &BTreeSet::new()).unwrap() else {
            panic!("a printer");
        };
        assert_ne!(id, base.identity.id, "a copy of its own");
        let saved = ProfileLibraries::new(Some(dir.path().into()), Vec::new())
            .load_printer_by_id(&id)
            .unwrap();
        assert_eq!(saved.capabilities.transition_layers, Some(4));
        assert!(saved.capabilities.two_stage_lift);
        assert_eq!(saved.time_calibration, base.time_calibration);
        assert_eq!(saved.hardware, base.hardware);
        assert!(saved.constraints.z_step_mm.is_some());
    }

    /// A resin that ships, saved, becomes a resin of the user's own with
    /// its blocks, and a slot changed on the chosen printer's block.
    #[test]
    fn a_resin_saved_keeps_its_blocks_and_the_change() {
        let libraries = ProfileLibraries::platform_default();
        let catalog = crate::profile_catalog::ProfileCatalog::from_libraries(&libraries);
        let m7 = PrinterId::new("anycubic-photon-mono-m7-pro").unwrap();
        let target = CalibrationTarget::from_printer(&m7);
        let entry = catalog
            .materials()
            .iter()
            .find(|m| m.calibrated_printers.contains(&target))
            .expect("a resin set for the M7 Pro");
        let base = libraries.load_material_by_id(&entry.id).unwrap();
        let mut form = Form::resin(base.clone(), false, Some(&m7), |id| id.as_str().to_owned());
        assert!(
            !form.t("normal_exposure_sec").is_empty(),
            "its exposure shows"
        );
        form.edited("normal_exposure_sec", "2.5");
        let dir = tempfile::tempdir().unwrap();
        let existing: BTreeSet<MaterialId> = [base.identity.id.clone()].into();
        let Saved::Resin(id) = form.save(dir.path(), &BTreeSet::new(), &existing).unwrap() else {
            panic!("a resin");
        };
        assert_ne!(id, base.identity.id);
        let saved = ProfileLibraries::new(Some(dir.path().into()), Vec::new())
            .load_material_by_id(&id)
            .unwrap();
        assert_eq!(saved.per_printer.len(), base.per_printer.len());
        let block = &saved.per_printer[&target];
        assert_eq!(block.normal_exposure_sec.map(|s| s.value()), Some(2.5));
    }
}
