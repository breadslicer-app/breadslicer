/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Which of a print's values nobody actually chose.

use std::fmt;

use sc_print_profiles::{
    FallbackLevel, MaterialResolved, ResolveWarning, ResolvedField, ResolvedPrintConfig,
};

/// What the resolver put in place of a value no profile named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackKind {
    /// The layer runs straight through, with no pause.
    NoDwell,
    /// The panel is driven at full power.
    FullPower,
    /// The bottom layers take the normal layers' value.
    NormalCounterpart,
    /// The resin's vendor published no figure, so a value typical of
    /// the material class stands in.
    TypicalResin,
    /// Nothing about the resin stated it, so the machine's own
    /// expectation stands in.
    MachineDefault,
    /// Neither the resin nor the machine stated it, so this slicer's
    /// own figure stands in.
    SlicerDefault,
    /// Nobody measured the shrinkage, so the print runs the dimensions
    /// as modelled.
    NoCompensation,
}

impl fmt::Display for FallbackKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoDwell => "no pause",
            Self::FullPower => "full power",
            Self::NormalCounterpart => "the normal-layer value",
            Self::TypicalResin => "a typical photopolymer",
            Self::MachineDefault => "the machine's own default",
            Self::SlicerDefault => "this slicer's default",
            Self::NoCompensation => "no compensation",
        })
    }
}

/// One value the print will run that no profile supplied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FallbackUsed {
    /// The value that was not calibrated.
    pub field: ResolvedField,
    /// The number the machine will run, in the field's own unit.
    pub resolved_to: f64,
    /// Where that number came from.
    pub kind: FallbackKind,
}

impl FallbackKind {
    /// Say what was looked for and not found, which is calibration for
    /// a machine setting but a published figure for a resin's own
    /// properties.
    fn nothing_found(self) -> &'static str {
        match self {
            Self::NoDwell
            | Self::FullPower
            | Self::NormalCounterpart
            | Self::MachineDefault
            | Self::SlicerDefault
            | Self::NoCompensation => "no calibrated value",
            Self::TypicalResin => "no published figure",
        }
    }
}

impl fmt::Display for FallbackUsed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}, using {} ({})",
            self.field,
            self.kind.nothing_found(),
            self.resolved_to,
            self.kind,
        )
    }
}

/// Reads the value a field resolved to, or `None` when the print
/// carries no such value at all, as a machine moving in one stage
/// carries no second one.
type ResolvedValue = fn(&MaterialResolved) -> Option<f64>;

/// Every value the resolver is willing to substitute, in the order
/// a report lists them: the normal-layer pauses, the bottom-layer
/// pauses, the bottom-layer travel including its second stage, the
/// two duties, then the resin's own properties. The required fields
/// are absent because resolution fails outright without them.
const FALLBACKS: [(ResolvedField, FallbackKind, ResolvedValue); 15] = [
    (
        ResolvedField::WaitBeforeCureSec,
        FallbackKind::NoDwell,
        |material| Some(material.wait_before_cure_sec.value()),
    ),
    (
        ResolvedField::WaitAfterCureSec,
        FallbackKind::NoDwell,
        |material| Some(material.wait_after_cure_sec.value()),
    ),
    (
        ResolvedField::WaitAfterLiftSec,
        FallbackKind::NoDwell,
        |material| Some(material.wait_after_lift_sec.value()),
    ),
    (
        ResolvedField::BottomWaitBeforeCureSec,
        FallbackKind::NormalCounterpart,
        |material| Some(material.bottom_wait_before_cure_sec.value()),
    ),
    (
        ResolvedField::BottomWaitAfterCureSec,
        FallbackKind::NormalCounterpart,
        |material| Some(material.bottom_wait_after_cure_sec.value()),
    ),
    (
        ResolvedField::BottomWaitAfterLiftSec,
        FallbackKind::NormalCounterpart,
        |material| Some(material.bottom_wait_after_lift_sec.value()),
    ),
    (
        ResolvedField::BottomLiftDistanceMm,
        FallbackKind::NormalCounterpart,
        |material| Some(material.bottom_lift_distance_mm.value()),
    ),
    (
        ResolvedField::BottomLiftSpeedMmMin,
        FallbackKind::NormalCounterpart,
        |material| Some(material.bottom_lift_speed_mm_min.value()),
    ),
    (
        ResolvedField::BottomRetractSpeedMmMin,
        FallbackKind::NormalCounterpart,
        |material| Some(material.bottom_retract_speed_mm_min.value()),
    ),
    (
        ResolvedField::BottomSecondLiftDistanceMm,
        FallbackKind::NormalCounterpart,
        |material| {
            material
                .bottom_second_travel
                .map(|stage| stage.lift_distance_mm.value())
        },
    ),
    (
        ResolvedField::BottomSecondLiftSpeedMmMin,
        FallbackKind::NormalCounterpart,
        |material| {
            material
                .bottom_second_travel
                .map(|stage| stage.lift_speed_mm_min.value())
        },
    ),
    (
        ResolvedField::BottomSecondRetractSpeedMmMin,
        FallbackKind::NormalCounterpart,
        |material| {
            material
                .bottom_second_travel
                .map(|stage| stage.retract_speed_mm_min.value())
        },
    ),
    (
        ResolvedField::LightPwm,
        FallbackKind::FullPower,
        |material| Some(material.light_pwm.percent()),
    ),
    (
        ResolvedField::BottomLightPwm,
        FallbackKind::FullPower,
        |material| Some(material.bottom_light_pwm.percent()),
    ),
    (
        ResolvedField::CuredDensityGPerCm3,
        FallbackKind::TypicalResin,
        |material| Some(material.cured_density_g_per_cm3.value()),
    ),
];

/// List the values this print will run that no profile chose.
///
/// A value taken from the material's `[defaults]` block was chosen
/// by whoever wrote the resin, so it is not listed here; only the
/// substitutions the resolver makes on its own are.
pub fn fallbacks_used(config: &ResolvedPrintConfig) -> Vec<FallbackUsed> {
    let mut used: Vec<FallbackUsed> = FALLBACKS
        .into_iter()
        .filter(|(field, _, _)| {
            !config
                .overrides_applied
                .iter()
                .any(|record| record.field == *field)
        })
        .filter_map(|(field, kind, resolved_value)| {
            Some(FallbackUsed {
                field,
                resolved_to: resolved_value(&config.material_resolved)?,
                kind,
            })
        })
        .collect();
    used.extend(uncalibrated(config));
    used
}

/// The values the resolver filled because nothing about the resin
/// stated them.
///
/// These are not in [`FALLBACKS`] because the table pairs a field with
/// one substitution, while these have levels: the machine may state a
/// figure, and this slicer answers when it does not. The resolver knows
/// which level answered and says so in its warnings.
fn uncalibrated(config: &ResolvedPrintConfig) -> Vec<FallbackUsed> {
    config
        .warnings
        .iter()
        .filter_map(|warning| match warning {
            ResolveWarning::UncalibratedFallback { field, from } => Some(FallbackUsed {
                field: *field,
                resolved_to: uncalibrated_value(*field, &config.material_resolved)?,
                kind: match from {
                    FallbackLevel::Machine => FallbackKind::MachineDefault,
                    FallbackLevel::Slicer => FallbackKind::SlicerDefault,
                    FallbackLevel::Unadjusted => FallbackKind::NoCompensation,
                },
            }),
            _ => None,
        })
        .collect()
}

/// What one of those fields resolved to.
fn uncalibrated_value(field: ResolvedField, material: &MaterialResolved) -> Option<f64> {
    match field {
        ResolvedField::BottomLayerCount => Some(f64::from(material.bottom_layer_count)),
        // Compound, but every axis carries the same figure whenever
        // this fallback is the one that filled it.
        ResolvedField::ScaleCompensationPct => Some(material.scale_compensation_pct.x.value()),
        _ => None,
    }
}

/// Render the uncalibrated values as the block a frontend shows
/// before it writes a printer file: a counting header, then one
/// indented line per value.
///
/// Empty when nothing fell back, so a caller writes every line it
/// gets without testing for the nothing-to-say case.
pub fn fallback_report_lines(config: &ResolvedPrintConfig) -> Vec<String> {
    let used = fallbacks_used(config);
    if used.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(used.len() + 1);
    lines.push(format!(
        "warning: {} values for this print were chosen by no profile:",
        used.len(),
    ));
    lines.extend(used.iter().map(|fallback| format!("  {fallback}")));
    lines
}
