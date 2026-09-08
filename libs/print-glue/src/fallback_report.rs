/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Which of a print's values nobody actually chose.

use std::fmt;

use sc_print_profiles::{MaterialResolved, ResolvedField, ResolvedPrintConfig};

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
}

impl fmt::Display for FallbackKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoDwell => "no pause",
            Self::FullPower => "full power",
            Self::NormalCounterpart => "the normal-layer value",
            Self::TypicalResin => "a typical photopolymer",
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
            Self::NoDwell | Self::FullPower | Self::NormalCounterpart => "no calibrated value",
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

type ResolvedValue = fn(&MaterialResolved) -> f64;

/// Every value the resolver is willing to substitute, in the order
/// a report lists them: the normal-layer pauses, the bottom-layer
/// pauses, the bottom-layer travel, the two duties, then the resin's
/// own properties. The required fields are absent because resolution
/// fails outright without them.
const FALLBACKS: [(ResolvedField, FallbackKind, ResolvedValue); 12] = [
    (
        ResolvedField::WaitBeforeCureSec,
        FallbackKind::NoDwell,
        |material| material.wait_before_cure_sec.value(),
    ),
    (
        ResolvedField::WaitAfterCureSec,
        FallbackKind::NoDwell,
        |material| material.wait_after_cure_sec.value(),
    ),
    (
        ResolvedField::WaitAfterLiftSec,
        FallbackKind::NoDwell,
        |material| material.wait_after_lift_sec.value(),
    ),
    (
        ResolvedField::BottomWaitBeforeCureSec,
        FallbackKind::NormalCounterpart,
        |material| material.bottom_wait_before_cure_sec.value(),
    ),
    (
        ResolvedField::BottomWaitAfterCureSec,
        FallbackKind::NormalCounterpart,
        |material| material.bottom_wait_after_cure_sec.value(),
    ),
    (
        ResolvedField::BottomWaitAfterLiftSec,
        FallbackKind::NormalCounterpart,
        |material| material.bottom_wait_after_lift_sec.value(),
    ),
    (
        ResolvedField::BottomLiftDistanceMm,
        FallbackKind::NormalCounterpart,
        |material| material.bottom_lift_distance_mm.value(),
    ),
    (
        ResolvedField::BottomLiftSpeedMmMin,
        FallbackKind::NormalCounterpart,
        |material| material.bottom_lift_speed_mm_min.value(),
    ),
    (
        ResolvedField::BottomRetractSpeedMmMin,
        FallbackKind::NormalCounterpart,
        |material| material.bottom_retract_speed_mm_min.value(),
    ),
    (
        ResolvedField::LightPwm,
        FallbackKind::FullPower,
        |material| material.light_pwm.percent(),
    ),
    (
        ResolvedField::BottomLightPwm,
        FallbackKind::FullPower,
        |material| material.bottom_light_pwm.percent(),
    ),
    (
        ResolvedField::CuredDensityGPerCm3,
        FallbackKind::TypicalResin,
        |material| material.cured_density_g_per_cm3.value(),
    ),
];

/// List the values this print will run that no profile chose.
///
/// A value taken from the material's `[defaults]` block was chosen
/// by whoever wrote the resin, so it is not listed here; only the
/// substitutions the resolver makes on its own are.
pub fn fallbacks_used(config: &ResolvedPrintConfig) -> Vec<FallbackUsed> {
    FALLBACKS
        .into_iter()
        .filter(|(field, _, _)| {
            !config
                .overrides_applied
                .iter()
                .any(|record| record.field == *field)
        })
        .map(|(field, kind, resolved_value)| FallbackUsed {
            field,
            resolved_to: resolved_value(&config.material_resolved),
            kind,
        })
        .collect()
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
