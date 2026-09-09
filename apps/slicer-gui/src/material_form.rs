/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Window-independent form logic for the resin profile editor.
//!
//! Parses the editor's text fields into typed profile values and seeds
//! the fields back from loaded values. Every field is optional: an
//! empty text means "unset" and round-trips to an absent TOML key
//! (never a zero). Two groups are composites, filled together or left
//! empty together: scale compensation's three axes, and each travel
//! stage's distance with its two speeds.

use std::fmt;
use std::path::Path;

use slint::SharedString;

use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
    ScaleCompensation, ScaleCompensationPercent,
};
use sc_print_profiles::{
    MaterialDefaults, MaterialPerPrinter, MaterialProfile, SecondTravelStage,
    save_material_extension_to_library, save_material_profile_to_library,
};

use crate::i18n::ctr;

/// The raw texts of the calibration value fields, straight out of the
/// editor inputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CalibrationFormInput {
    pub normal_exposure_sec: String,
    pub bottom_exposure_sec: String,
    pub bottom_layer_count: String,
    pub wait_before_cure_sec: String,
    pub wait_after_cure_sec: String,
    pub wait_after_lift_sec: String,
    pub bottom_wait_before_cure_sec: String,
    pub bottom_wait_after_cure_sec: String,
    pub bottom_wait_after_lift_sec: String,
    pub lift_distance_mm: String,
    pub lift_speed_mm_min: String,
    pub retract_speed_mm_min: String,
    pub second_lift_distance_mm: String,
    pub second_lift_speed_mm_min: String,
    pub second_retract_speed_mm_min: String,
    pub bottom_lift_distance_mm: String,
    pub bottom_lift_speed_mm_min: String,
    pub bottom_retract_speed_mm_min: String,
    pub bottom_second_lift_distance_mm: String,
    pub bottom_second_lift_speed_mm_min: String,
    pub bottom_second_retract_speed_mm_min: String,
    pub light_pwm: String,
    pub bottom_light_pwm: String,
}

/// The raw texts of the defaults fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DefaultsFormInput {
    pub scale_x_pct: String,
    pub scale_y_pct: String,
    pub scale_z_pct: String,
    pub bottom_layer_count: String,
    pub wait_before_cure_sec: String,
    pub wait_after_cure_sec: String,
    pub wait_after_lift_sec: String,
    pub light_pwm: String,
    pub bottom_light_pwm: String,
}

/// The editor field a form error refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormField {
    NormalExposure,
    BottomExposure,
    BottomLayerCount,
    WaitBeforeCure,
    WaitAfterCure,
    WaitAfterLift,
    BottomWaitBeforeCure,
    BottomWaitAfterCure,
    BottomWaitAfterLift,
    LiftDistance,
    LiftSpeed,
    RetractSpeed,
    SecondLiftDistance,
    SecondLiftSpeed,
    SecondRetractSpeed,
    BottomLiftDistance,
    BottomLiftSpeed,
    BottomRetractSpeed,
    BottomSecondLiftDistance,
    BottomSecondLiftSpeed,
    BottomSecondRetractSpeed,
    LightPwm,
    BottomLightPwm,
    ScaleCompensationX,
    ScaleCompensationY,
    ScaleCompensationZ,
}

impl FormField {
    /// English field name, for log / `Display` presentation.
    fn english_name(self) -> &'static str {
        match self {
            Self::NormalExposure => "Normal exposure",
            Self::BottomExposure => "Bottom exposure",
            Self::BottomLayerCount => "Bottom layer count",
            Self::WaitBeforeCure => "Wait before cure",
            Self::WaitAfterCure => "Wait after cure",
            Self::WaitAfterLift => "Wait after lift",
            Self::BottomWaitBeforeCure => "Bottom wait before cure",
            Self::BottomWaitAfterCure => "Bottom wait after cure",
            Self::BottomWaitAfterLift => "Bottom wait after lift",
            Self::LiftDistance => "Lift distance",
            Self::LiftSpeed => "Lift speed",
            Self::RetractSpeed => "Retract speed",
            Self::SecondLiftDistance => "Second lift distance",
            Self::SecondLiftSpeed => "Second lift speed",
            Self::SecondRetractSpeed => "Second retract speed",
            Self::BottomLiftDistance => "Bottom lift distance",
            Self::BottomLiftSpeed => "Bottom lift speed",
            Self::BottomRetractSpeed => "Bottom retract speed",
            Self::BottomSecondLiftDistance => "Bottom second lift distance",
            Self::BottomSecondLiftSpeed => "Bottom second lift speed",
            Self::BottomSecondRetractSpeed => "Bottom second retract speed",
            Self::LightPwm => "Light power",
            Self::BottomLightPwm => "Bottom light power",
            Self::ScaleCompensationX => "Scale compensation x",
            Self::ScaleCompensationY => "Scale compensation y",
            Self::ScaleCompensationZ => "Scale compensation z",
        }
    }

    /// Field name in the user's language, for the dialogs.
    fn translated_name(self) -> SharedString {
        match self {
            Self::NormalExposure => ctr!("MaterialForm", "Normal exposure"),
            Self::BottomExposure => ctr!("MaterialForm", "Bottom exposure"),
            Self::BottomLayerCount => ctr!("MaterialForm", "Bottom layer count"),
            Self::WaitBeforeCure => ctr!("MaterialForm", "Wait before cure"),
            Self::WaitAfterCure => ctr!("MaterialForm", "Wait after cure"),
            Self::WaitAfterLift => ctr!("MaterialForm", "Wait after lift"),
            Self::BottomWaitBeforeCure => ctr!("MaterialForm", "Bottom wait before cure"),
            Self::BottomWaitAfterCure => ctr!("MaterialForm", "Bottom wait after cure"),
            Self::BottomWaitAfterLift => ctr!("MaterialForm", "Bottom wait after lift"),
            Self::LiftDistance => ctr!("MaterialForm", "Lift distance"),
            Self::LiftSpeed => ctr!("MaterialForm", "Lift speed"),
            Self::RetractSpeed => ctr!("MaterialForm", "Retract speed"),
            Self::SecondLiftDistance => ctr!("MaterialForm", "Second lift distance"),
            Self::SecondLiftSpeed => ctr!("MaterialForm", "Second lift speed"),
            Self::SecondRetractSpeed => ctr!("MaterialForm", "Second retract speed"),
            Self::BottomLiftDistance => ctr!("MaterialForm", "Bottom lift distance"),
            Self::BottomLiftSpeed => ctr!("MaterialForm", "Bottom lift speed"),
            Self::BottomRetractSpeed => ctr!("MaterialForm", "Bottom retract speed"),
            Self::BottomSecondLiftDistance => {
                ctr!("MaterialForm", "Bottom second lift distance")
            }
            Self::BottomSecondLiftSpeed => ctr!("MaterialForm", "Bottom second lift speed"),
            Self::BottomSecondRetractSpeed => {
                ctr!("MaterialForm", "Bottom second retract speed")
            }
            Self::LightPwm => ctr!("MaterialForm", "Light power"),
            Self::BottomLightPwm => ctr!("MaterialForm", "Bottom light power"),
            Self::ScaleCompensationX => ctr!("MaterialForm", "Scale compensation x"),
            Self::ScaleCompensationY => ctr!("MaterialForm", "Scale compensation y"),
            Self::ScaleCompensationZ => ctr!("MaterialForm", "Scale compensation z"),
        }
    }
}

/// Which layers a block's second travel stage covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SecondStage {
    Normal,
    Bottom,
}

impl SecondStage {
    /// The fields holding this stage's distance, lift speed and
    /// retract speed, in that order.
    fn fields(self) -> [FormField; 3] {
        match self {
            Self::Normal => [
                FormField::SecondLiftDistance,
                FormField::SecondLiftSpeed,
                FormField::SecondRetractSpeed,
            ],
            Self::Bottom => [
                FormField::BottomSecondLiftDistance,
                FormField::BottomSecondLiftSpeed,
                FormField::BottomSecondRetractSpeed,
            ],
        }
    }

    /// English stage name, for log / `Display` presentation.
    fn english_name(self) -> &'static str {
        match self {
            Self::Normal => "the second travel stage",
            Self::Bottom => "the bottom second travel stage",
        }
    }

    /// Stage name in the user's language, for the dialogs.
    fn translated_name(self) -> SharedString {
        match self {
            Self::Normal => ctr!("MaterialForm", "the second travel stage"),
            Self::Bottom => ctr!("MaterialForm", "the bottom second travel stage"),
        }
    }
}

/// Why a form value could not be accepted. `Display` keeps the English
/// text for logs; the dialogs show [`Self::user_message`] instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InvalidFormValue {
    NotANumber {
        field: FormField,
    },
    OutOfRange {
        field: FormField,
    },
    /// Scale compensation had some axes filled and some empty; the
    /// three are set together or not at all.
    PartialScaleCompensation,
    /// A second travel stage had some of its three values filled and
    /// some empty. A distance the platform crosses at no stated speed
    /// is not a move, so the three go together.
    PartialSecondTravelStage {
        stage: SecondStage,
    },
}

impl InvalidFormValue {
    /// The message shown in a dialog's inline error line, in the
    /// user's language.
    pub(crate) fn user_message(&self) -> SharedString {
        match self {
            Self::NotANumber { field } => {
                ctr!(
                    "MaterialForm",
                    "{} must be a number",
                    field.translated_name()
                )
            }
            Self::OutOfRange { field } => {
                ctr!(
                    "MaterialForm",
                    "{} is out of range",
                    field.translated_name()
                )
            }
            Self::PartialScaleCompensation => ctr!(
                "MaterialForm",
                "Fill all three scale compensation axes, or leave all three empty",
            ),
            Self::PartialSecondTravelStage { stage } => ctr!(
                "MaterialForm",
                "Fill all three values of {}, or leave all three empty",
                stage.translated_name()
            ),
        }
    }
}

impl fmt::Display for InvalidFormValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber { field } => write!(f, "{} must be a number", field.english_name()),
            Self::OutOfRange { field } => write!(f, "{} is out of range", field.english_name()),
            Self::PartialScaleCompensation => write!(
                f,
                "Fill all three scale compensation axes, or leave all three empty",
            ),
            Self::PartialSecondTravelStage { stage } => write!(
                f,
                "Fill all three values of {}, or leave all three empty",
                stage.english_name()
            ),
        }
    }
}

impl std::error::Error for InvalidFormValue {}

/// Parse the calibration fields into a per-printer block. Empty
/// fields stay unset; a block with every field empty is legal (the
/// block's presence is itself the calibration assertion).
pub(crate) fn parse_calibration_form(
    input: &CalibrationFormInput,
) -> Result<MaterialPerPrinter, InvalidFormValue> {
    Ok(MaterialPerPrinter {
        normal_exposure_sec: parse_optional_sec(
            FormField::NormalExposure,
            &input.normal_exposure_sec,
        )?,
        bottom_exposure_sec: parse_optional_sec(
            FormField::BottomExposure,
            &input.bottom_exposure_sec,
        )?,
        bottom_layer_count: parse_optional_count(
            FormField::BottomLayerCount,
            &input.bottom_layer_count,
        )?,
        wait_before_cure_sec: parse_optional_wait(
            FormField::WaitBeforeCure,
            &input.wait_before_cure_sec,
        )?,
        wait_after_cure_sec: parse_optional_wait(
            FormField::WaitAfterCure,
            &input.wait_after_cure_sec,
        )?,
        wait_after_lift_sec: parse_optional_wait(
            FormField::WaitAfterLift,
            &input.wait_after_lift_sec,
        )?,
        bottom_wait_before_cure_sec: parse_optional_wait(
            FormField::BottomWaitBeforeCure,
            &input.bottom_wait_before_cure_sec,
        )?,
        bottom_wait_after_cure_sec: parse_optional_wait(
            FormField::BottomWaitAfterCure,
            &input.bottom_wait_after_cure_sec,
        )?,
        bottom_wait_after_lift_sec: parse_optional_wait(
            FormField::BottomWaitAfterLift,
            &input.bottom_wait_after_lift_sec,
        )?,
        lift_distance_mm: parse_optional_mm(FormField::LiftDistance, &input.lift_distance_mm)?,
        lift_speed_mm_min: parse_optional_speed(FormField::LiftSpeed, &input.lift_speed_mm_min)?,
        retract_speed_mm_min: parse_optional_speed(
            FormField::RetractSpeed,
            &input.retract_speed_mm_min,
        )?,
        bottom_lift_distance_mm: parse_optional_mm(
            FormField::BottomLiftDistance,
            &input.bottom_lift_distance_mm,
        )?,
        bottom_lift_speed_mm_min: parse_optional_speed(
            FormField::BottomLiftSpeed,
            &input.bottom_lift_speed_mm_min,
        )?,
        bottom_retract_speed_mm_min: parse_optional_speed(
            FormField::BottomRetractSpeed,
            &input.bottom_retract_speed_mm_min,
        )?,
        light_pwm: parse_optional_pwm(FormField::LightPwm, &input.light_pwm)?,
        bottom_light_pwm: parse_optional_pwm(FormField::BottomLightPwm, &input.bottom_light_pwm)?,
        second_travel: parse_optional_second_stage(
            SecondStage::Normal,
            &input.second_lift_distance_mm,
            &input.second_lift_speed_mm_min,
            &input.second_retract_speed_mm_min,
        )?,
        bottom_second_travel: parse_optional_second_stage(
            SecondStage::Bottom,
            &input.bottom_second_lift_distance_mm,
            &input.bottom_second_lift_speed_mm_min,
            &input.bottom_second_retract_speed_mm_min,
        )?,
    })
}

/// Parse the defaults fields. The three scale compensation axes must
/// be filled together or left empty together.
pub(crate) fn parse_defaults_form(
    input: &DefaultsFormInput,
) -> Result<MaterialDefaults, InvalidFormValue> {
    let axes = [
        input.scale_x_pct.trim(),
        input.scale_y_pct.trim(),
        input.scale_z_pct.trim(),
    ];
    let filled = axes.iter().filter(|axis| !axis.is_empty()).count();
    let scale_compensation_pct = match filled {
        0 => None,
        3 => Some(ScaleCompensation {
            x: parse_scale_axis(FormField::ScaleCompensationX, axes[0])?,
            y: parse_scale_axis(FormField::ScaleCompensationY, axes[1])?,
            z: parse_scale_axis(FormField::ScaleCompensationZ, axes[2])?,
        }),
        _ => return Err(InvalidFormValue::PartialScaleCompensation),
    };
    Ok(MaterialDefaults {
        scale_compensation_pct,
        bottom_layer_count: parse_optional_count(
            FormField::BottomLayerCount,
            &input.bottom_layer_count,
        )?,
        wait_before_cure_sec: parse_optional_wait(
            FormField::WaitBeforeCure,
            &input.wait_before_cure_sec,
        )?,
        wait_after_cure_sec: parse_optional_wait(
            FormField::WaitAfterCure,
            &input.wait_after_cure_sec,
        )?,
        wait_after_lift_sec: parse_optional_wait(
            FormField::WaitAfterLift,
            &input.wait_after_lift_sec,
        )?,
        light_pwm: parse_optional_pwm(FormField::LightPwm, &input.light_pwm)?,
        bottom_light_pwm: parse_optional_pwm(FormField::BottomLightPwm, &input.bottom_light_pwm)?,
    })
}

/// Seed the calibration fields from a loaded block: set values render
/// as plain numbers, unset ones as empty texts.
pub(crate) fn calibration_form_from_block(block: &MaterialPerPrinter) -> CalibrationFormInput {
    CalibrationFormInput {
        normal_exposure_sec: fmt_opt_f64(block.normal_exposure_sec.map(|v| v.value())),
        bottom_exposure_sec: fmt_opt_f64(block.bottom_exposure_sec.map(|v| v.value())),
        bottom_layer_count: fmt_opt_u32(block.bottom_layer_count),
        wait_before_cure_sec: fmt_opt_f64(block.wait_before_cure_sec.map(|v| v.value())),
        wait_after_cure_sec: fmt_opt_f64(block.wait_after_cure_sec.map(|v| v.value())),
        wait_after_lift_sec: fmt_opt_f64(block.wait_after_lift_sec.map(|v| v.value())),
        bottom_wait_before_cure_sec: fmt_opt_f64(
            block.bottom_wait_before_cure_sec.map(|v| v.value()),
        ),
        bottom_wait_after_cure_sec: fmt_opt_f64(
            block.bottom_wait_after_cure_sec.map(|v| v.value()),
        ),
        bottom_wait_after_lift_sec: fmt_opt_f64(
            block.bottom_wait_after_lift_sec.map(|v| v.value()),
        ),
        lift_distance_mm: fmt_opt_f64(block.lift_distance_mm.map(|v| v.value())),
        lift_speed_mm_min: fmt_opt_f64(block.lift_speed_mm_min.map(|v| v.value())),
        retract_speed_mm_min: fmt_opt_f64(block.retract_speed_mm_min.map(|v| v.value())),
        second_lift_distance_mm: fmt_opt_f64(
            block.second_travel.map(|s| s.lift_distance_mm.value()),
        ),
        second_lift_speed_mm_min: fmt_opt_f64(
            block.second_travel.map(|s| s.lift_speed_mm_min.value()),
        ),
        second_retract_speed_mm_min: fmt_opt_f64(
            block.second_travel.map(|s| s.retract_speed_mm_min.value()),
        ),
        bottom_lift_distance_mm: fmt_opt_f64(block.bottom_lift_distance_mm.map(|v| v.value())),
        bottom_lift_speed_mm_min: fmt_opt_f64(block.bottom_lift_speed_mm_min.map(|v| v.value())),
        bottom_retract_speed_mm_min: fmt_opt_f64(
            block.bottom_retract_speed_mm_min.map(|v| v.value()),
        ),
        bottom_second_lift_distance_mm: fmt_opt_f64(
            block
                .bottom_second_travel
                .map(|s| s.lift_distance_mm.value()),
        ),
        bottom_second_lift_speed_mm_min: fmt_opt_f64(
            block
                .bottom_second_travel
                .map(|s| s.lift_speed_mm_min.value()),
        ),
        bottom_second_retract_speed_mm_min: fmt_opt_f64(
            block
                .bottom_second_travel
                .map(|s| s.retract_speed_mm_min.value()),
        ),
        light_pwm: fmt_opt_f64(block.light_pwm.map(|v| v.percent())),
        bottom_light_pwm: fmt_opt_f64(block.bottom_light_pwm.map(|v| v.percent())),
    }
}

/// Seed the defaults fields from loaded defaults.
pub(crate) fn defaults_form_from(defaults: &MaterialDefaults) -> DefaultsFormInput {
    let (x, y, z) = defaults
        .scale_compensation_pct
        .map(|scale| {
            (
                fmt_f64(scale.x.value()),
                fmt_f64(scale.y.value()),
                fmt_f64(scale.z.value()),
            )
        })
        .unwrap_or_default();
    DefaultsFormInput {
        scale_x_pct: x,
        scale_y_pct: y,
        scale_z_pct: z,
        bottom_layer_count: fmt_opt_u32(defaults.bottom_layer_count),
        wait_before_cure_sec: fmt_opt_f64(defaults.wait_before_cure_sec.map(|v| v.value())),
        wait_after_cure_sec: fmt_opt_f64(defaults.wait_after_cure_sec.map(|v| v.value())),
        wait_after_lift_sec: fmt_opt_f64(defaults.wait_after_lift_sec.map(|v| v.value())),
        light_pwm: fmt_opt_f64(defaults.light_pwm.map(|v| v.percent())),
        bottom_light_pwm: fmt_opt_f64(defaults.bottom_light_pwm.map(|v| v.percent())),
    }
}

/// Which on-disk file an edited profile writes back to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditedProfileKind {
    /// A full custom profile: `<library>/materials/<id>.toml`.
    Full,
    /// An extension: `<library>/materials/extensions/<id>.toml`.
    Extension,
}

/// Write an edited profile back through the writer matching its kind.
/// The error is the user-facing message for the editor's inline error
/// line.
pub(crate) fn save_edited_profile(
    library_dir: &Path,
    profile: &MaterialProfile,
    kind: EditedProfileKind,
) -> Result<(), String> {
    match kind {
        EditedProfileKind::Full => {
            save_material_profile_to_library(library_dir, profile).map_err(|err| err.to_string())
        }
        EditedProfileKind::Extension => {
            save_material_extension_to_library(library_dir, profile).map_err(|err| err.to_string())
        }
    }
}

fn fmt_f64(value: f64) -> String {
    format!("{value}")
}

fn fmt_opt_f64(value: Option<f64>) -> String {
    value.map(fmt_f64).unwrap_or_default()
}

fn fmt_opt_u32(value: Option<u32>) -> String {
    value.map(|v| format!("{v}")).unwrap_or_default()
}

fn parse_f64(field: FormField, text: &str) -> Result<f64, InvalidFormValue> {
    text.trim()
        .parse::<f64>()
        .map_err(|_| InvalidFormValue::NotANumber { field })
}

fn parse_optional_sec(
    field: FormField,
    text: &str,
) -> Result<Option<PositiveFiniteSec>, InvalidFormValue> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    PositiveFiniteSec::new(parse_f64(field, text)?)
        .map(Some)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

fn parse_optional_wait(
    field: FormField,
    text: &str,
) -> Result<Option<NonNegativeFiniteSec>, InvalidFormValue> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    NonNegativeFiniteSec::new(parse_f64(field, text)?)
        .map(Some)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

fn parse_mm(field: FormField, text: &str) -> Result<PositiveFiniteMm, InvalidFormValue> {
    PositiveFiniteMm::new(parse_f64(field, text)?)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

fn parse_optional_mm(
    field: FormField,
    text: &str,
) -> Result<Option<PositiveFiniteMm>, InvalidFormValue> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    parse_mm(field, text).map(Some)
}

fn parse_speed(field: FormField, text: &str) -> Result<PositiveFiniteMmPerMin, InvalidFormValue> {
    PositiveFiniteMmPerMin::new(parse_f64(field, text)?)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

fn parse_optional_speed(
    field: FormField,
    text: &str,
) -> Result<Option<PositiveFiniteMmPerMin>, InvalidFormValue> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    parse_speed(field, text).map(Some)
}

fn parse_optional_second_stage(
    stage: SecondStage,
    distance_mm: &str,
    lift_speed_mm_min: &str,
    retract_speed_mm_min: &str,
) -> Result<Option<SecondTravelStage>, InvalidFormValue> {
    let texts = [
        distance_mm.trim(),
        lift_speed_mm_min.trim(),
        retract_speed_mm_min.trim(),
    ];
    let [distance_field, lift_speed_field, retract_speed_field] = stage.fields();
    match texts.iter().filter(|text| !text.is_empty()).count() {
        0 => Ok(None),
        3 => Ok(Some(SecondTravelStage {
            lift_distance_mm: parse_mm(distance_field, texts[0])?,
            lift_speed_mm_min: parse_speed(lift_speed_field, texts[1])?,
            retract_speed_mm_min: parse_speed(retract_speed_field, texts[2])?,
        })),
        _ => Err(InvalidFormValue::PartialSecondTravelStage { stage }),
    }
}

fn parse_optional_pwm(field: FormField, text: &str) -> Result<Option<LightPwm>, InvalidFormValue> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    LightPwm::new(parse_f64(field, text)?)
        .map(Some)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

fn parse_optional_count(field: FormField, text: &str) -> Result<Option<u32>, InvalidFormValue> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u32>()
        .map(Some)
        .map_err(|_| InvalidFormValue::NotANumber { field })
}

fn parse_scale_axis(
    field: FormField,
    text: &str,
) -> Result<ScaleCompensationPercent, InvalidFormValue> {
    ScaleCompensationPercent::new(parse_f64(field, text)?)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

#[cfg(test)]
mod tests;
