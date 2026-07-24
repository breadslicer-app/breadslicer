/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Window-independent form logic for the resin profile editor.
//!
//! Parses the editor's text fields into typed profile values and seeds
//! the fields back from loaded values. Every field is optional: an
//! empty text means "unset" and round-trips to an absent TOML key
//! (never a zero). Scale compensation is the one composite: its three
//! axes are set together or not at all.

use std::fmt;
use std::path::Path;

use sc_print_primitives::{
    PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, ScaleCompensation,
    ScaleCompensationPercent,
};
use sc_print_profiles::{
    MaterialDefaults, MaterialPerPrinter, MaterialProfile, save_material_extension_to_library,
    save_material_profile_to_library,
};

/// The raw texts of the six calibration value fields, straight out of
/// the editor inputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CalibrationFormInput {
    pub normal_exposure_sec: String,
    pub bottom_exposure_sec: String,
    pub bottom_layer_count: String,
    pub lift_distance_mm: String,
    pub lift_speed_mm_min: String,
    pub retract_speed_mm_min: String,
}

/// The raw texts of the defaults fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DefaultsFormInput {
    pub scale_x_pct: String,
    pub scale_y_pct: String,
    pub scale_z_pct: String,
    pub bottom_layer_count: String,
}

/// Why a form value could not be accepted. Rendered straight into the
/// editor's inline error message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InvalidFormValue {
    NotANumber {
        field: &'static str,
    },
    OutOfRange {
        field: &'static str,
    },
    /// Scale compensation had some axes filled and some empty; the
    /// three are set together or not at all.
    PartialScaleCompensation,
}

impl fmt::Display for InvalidFormValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber { field } => write!(f, "{field} must be a number"),
            Self::OutOfRange { field } => write!(f, "{field} is out of range"),
            Self::PartialScaleCompensation => write!(
                f,
                "Fill all three scale compensation axes, or leave all three empty",
            ),
        }
    }
}

impl std::error::Error for InvalidFormValue {}

/// Parse the six calibration fields into a per-printer block. Empty
/// fields stay unset; a block with every field empty is legal (the
/// block's presence is itself the calibration assertion).
pub(crate) fn parse_calibration_form(
    input: &CalibrationFormInput,
) -> Result<MaterialPerPrinter, InvalidFormValue> {
    Ok(MaterialPerPrinter {
        normal_exposure_sec: parse_optional_sec("Normal exposure", &input.normal_exposure_sec)?,
        bottom_exposure_sec: parse_optional_sec("Bottom exposure", &input.bottom_exposure_sec)?,
        bottom_layer_count: parse_optional_count("Bottom layer count", &input.bottom_layer_count)?,
        lift_distance_mm: parse_optional_mm("Lift distance", &input.lift_distance_mm)?,
        lift_speed_mm_min: parse_optional_speed("Lift speed", &input.lift_speed_mm_min)?,
        retract_speed_mm_min: parse_optional_speed("Retract speed", &input.retract_speed_mm_min)?,
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
            x: parse_scale_axis("Scale compensation x", axes[0])?,
            y: parse_scale_axis("Scale compensation y", axes[1])?,
            z: parse_scale_axis("Scale compensation z", axes[2])?,
        }),
        _ => return Err(InvalidFormValue::PartialScaleCompensation),
    };
    Ok(MaterialDefaults {
        scale_compensation_pct,
        bottom_layer_count: parse_optional_count("Bottom layer count", &input.bottom_layer_count)?,
    })
}

/// Seed the calibration fields from a loaded block: set values render
/// as plain numbers, unset ones as empty texts.
pub(crate) fn calibration_form_from_block(block: &MaterialPerPrinter) -> CalibrationFormInput {
    CalibrationFormInput {
        normal_exposure_sec: fmt_opt_f64(block.normal_exposure_sec.map(|v| v.value())),
        bottom_exposure_sec: fmt_opt_f64(block.bottom_exposure_sec.map(|v| v.value())),
        bottom_layer_count: fmt_opt_u32(block.bottom_layer_count),
        lift_distance_mm: fmt_opt_f64(block.lift_distance_mm.map(|v| v.value())),
        lift_speed_mm_min: fmt_opt_f64(block.lift_speed_mm_min.map(|v| v.value())),
        retract_speed_mm_min: fmt_opt_f64(block.retract_speed_mm_min.map(|v| v.value())),
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

fn parse_f64(field: &'static str, text: &str) -> Result<f64, InvalidFormValue> {
    text.trim()
        .parse::<f64>()
        .map_err(|_| InvalidFormValue::NotANumber { field })
}

fn parse_optional_sec(
    field: &'static str,
    text: &str,
) -> Result<Option<PositiveFiniteSec>, InvalidFormValue> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    PositiveFiniteSec::new(parse_f64(field, text)?)
        .map(Some)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

fn parse_optional_mm(
    field: &'static str,
    text: &str,
) -> Result<Option<PositiveFiniteMm>, InvalidFormValue> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    PositiveFiniteMm::new(parse_f64(field, text)?)
        .map(Some)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

fn parse_optional_speed(
    field: &'static str,
    text: &str,
) -> Result<Option<PositiveFiniteMmPerMin>, InvalidFormValue> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    PositiveFiniteMmPerMin::new(parse_f64(field, text)?)
        .map(Some)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

fn parse_optional_count(field: &'static str, text: &str) -> Result<Option<u32>, InvalidFormValue> {
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
    field: &'static str,
    text: &str,
) -> Result<ScaleCompensationPercent, InvalidFormValue> {
    ScaleCompensationPercent::new(parse_f64(field, text)?)
        .map_err(|_| InvalidFormValue::OutOfRange { field })
}

#[cfg(test)]
mod tests;
