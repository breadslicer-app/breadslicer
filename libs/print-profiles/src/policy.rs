/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-field constraint policy table and the autosnap warning
//! threshold.
//!
//! [`policies_for`] returns the list of [`ConstraintPolicy`] entries
//! applicable to a [`ResolvedField`] given a [`PrinterConstraints`].
//! Each policy is self-contained: it carries the bounds extracted
//! from the printer profile and can apply itself to a raw `f64`
//! value via [`ConstraintPolicy::apply`].
//!
//! Coverage today (the table grows as new fields gain constraints):
//!
//! - `LayerHeightMm`: snap into `layer_height_range_mm` first, then
//!   quantise to `z_step_mm`. Either or both constraints may be
//!   present.
//! - `LiftSpeedMmMin`: reject if below the minimum of
//!   `lift_speed_range_mm_min`, snap down to the maximum if above.
//! - `RetractSpeedMmMin`: same shape as lift speed, keyed by
//!   `retract_speed_range_mm_min`.
//! - `LiftDistanceMm`: snap to `lift_distance_max_mm` if above.
//! - Other fields: no constraint applies today.

use crate::printer_constraints::PrinterConstraints;
use crate::resolve_warning::SnapReason;
use crate::resolved_field::ResolvedField;

pub(crate) const AUTOSNAP_WARNING_THRESHOLD: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ConstraintPolicy {
    SnapToMultiple { step: f64 },
    SnapIntoRange { min: f64, max: f64 },
    SnapAtMaximum { max: f64 },
    RejectBelowMinSnapAtMaximum { min: f64, max: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ApplyResult {
    Unchanged,
    Snapped { snapped: f64, reason: SnapReason },
    RejectedBelow { minimum: f64 },
}

impl ConstraintPolicy {
    pub(crate) fn apply(self, value: f64) -> ApplyResult {
        match self {
            Self::SnapToMultiple { step } => {
                let multiples = (value / step).round();
                // A non-finite quotient (subnormal step) means the
                // grid is finer than f64 resolution at this
                // magnitude, so the value already sits on it within
                // representation error.
                if !multiples.is_finite() {
                    return ApplyResult::Unchanged;
                }
                // 0 multiples is not a printable height: values below
                // half a step snap UP to the first multiple instead of
                // quantising to 0. This substitution is survivable by
                // design for now, even though it can change the height
                // by a large factor (the Autosnap warning fires above
                // the threshold). Two pieces are missing before this
                // can become the rejection it arguably should be:
                // frontends do not surface resolve warnings yet, and
                // there is no remediation tooling to fix the offending
                // profile value in-app, so a hard error today would
                // strand the user without a recovery path. Once both
                // exist, reject here and let the user decide.
                let snapped = multiples.max(1.0) * step;
                // Near the f64 range limit rounding up can overflow
                // the product; pass the value through rather than
                // emit a non-finite height.
                if !snapped.is_finite() {
                    return ApplyResult::Unchanged;
                }
                if snapped == value {
                    ApplyResult::Unchanged
                } else {
                    ApplyResult::Snapped {
                        snapped,
                        reason: SnapReason::QuantisedToMultiple { step },
                    }
                }
            }
            Self::SnapIntoRange { min, max } => {
                if value < min {
                    ApplyResult::Snapped {
                        snapped: min,
                        reason: SnapReason::ClampedIntoRange { min, max },
                    }
                } else if value > max {
                    ApplyResult::Snapped {
                        snapped: max,
                        reason: SnapReason::ClampedIntoRange { min, max },
                    }
                } else {
                    ApplyResult::Unchanged
                }
            }
            Self::SnapAtMaximum { max } => {
                if value > max {
                    ApplyResult::Snapped {
                        snapped: max,
                        reason: SnapReason::ClampedAtMaximum { max },
                    }
                } else {
                    ApplyResult::Unchanged
                }
            }
            Self::RejectBelowMinSnapAtMaximum { min, max } => {
                if value < min {
                    ApplyResult::RejectedBelow { minimum: min }
                } else if value > max {
                    ApplyResult::Snapped {
                        snapped: max,
                        reason: SnapReason::ClampedAtMaximum { max },
                    }
                } else {
                    ApplyResult::Unchanged
                }
            }
        }
    }
}

pub(crate) fn policies_for(
    field: ResolvedField,
    constraints: &PrinterConstraints,
) -> Vec<ConstraintPolicy> {
    let mut out = Vec::new();
    match field {
        ResolvedField::LayerHeightMm => {
            if let Some(range) = constraints.layer_height_range_mm {
                out.push(ConstraintPolicy::SnapIntoRange {
                    min: range.min().value(),
                    max: range.max().value(),
                });
            }
            if let Some(step) = constraints.z_step_mm {
                out.push(ConstraintPolicy::SnapToMultiple { step: step.value() });
            }
        }
        ResolvedField::LiftSpeedMmMin => {
            if let Some(range) = constraints.lift_speed_range_mm_min {
                out.push(ConstraintPolicy::RejectBelowMinSnapAtMaximum {
                    min: range.min().value(),
                    max: range.max().value(),
                });
            }
        }
        ResolvedField::RetractSpeedMmMin => {
            if let Some(range) = constraints.retract_speed_range_mm_min {
                out.push(ConstraintPolicy::RejectBelowMinSnapAtMaximum {
                    min: range.min().value(),
                    max: range.max().value(),
                });
            }
        }
        ResolvedField::LiftDistanceMm => {
            if let Some(max) = constraints.lift_distance_max_mm {
                out.push(ConstraintPolicy::SnapAtMaximum { max: max.value() });
            }
        }
        ResolvedField::NormalExposureSec
        | ResolvedField::BottomExposureSec
        | ResolvedField::BottomLayerCount
        | ResolvedField::WaitBeforeCureSec
        | ResolvedField::WaitAfterCureSec
        | ResolvedField::WaitAfterLiftSec
        | ResolvedField::ScaleCompensationPct => {}
    }
    out
}

pub(crate) fn relative_delta_exceeds(before: f64, after: f64, threshold: f64) -> bool {
    ((after - before) / before).abs() > threshold
}
