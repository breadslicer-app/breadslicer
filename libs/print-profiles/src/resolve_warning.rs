/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Non-fatal diagnostics emitted by the resolver.
//!
//! [`ResolveWarning`] is collected into a vector on the resolved
//! configuration. Each variant captures a specific class of
//! divergence between the user's stated intent and the final
//! resolved value: autosnap when a constraint snapped a value and
//! the relative delta exceeded the warning threshold, derivation
//! from material intent when no calibrated value was available, a
//! material-default fallback, the use of the generic machine's
//! calibration block in place of one for the printer itself, and a
//! calibrated value the printer cannot act on.

use crate::resolved_field::ResolvedField;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolveWarning {
    Autosnap {
        field: ResolvedField,
        requested: f64,
        snapped: f64,
        reason: SnapReason,
    },
    DerivedFromIntent {
        field: ResolvedField,
        source: IntentSource,
    },
    DefaultUsed {
        field: ResolvedField,
    },
    /// The material has no calibration block for the printer; the
    /// values came from its `generic-machine` block.
    GenericCalibrationUsed,
    /// The material calibrates a second travel stage for a printer
    /// that does not split its travel, so the stage was dropped and
    /// the platform makes the whole move at once.
    SecondTravelStageUnsupported,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapReason {
    QuantisedToMultiple { step: f64 },
    ClampedIntoRange { min: f64, max: f64 },
    ClampedAtMaximum { max: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntentSource {
    TargetEnergyDensity,
    TargetLayerHeight,
}

impl IntentSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TargetEnergyDensity => "target_energy_density_mj_per_cm2",
            Self::TargetLayerHeight => "target_layer_height_mm",
        }
    }
}

impl std::fmt::Display for IntentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Display for SnapReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuantisedToMultiple { step } => {
                write!(f, "quantised to multiple of {step}")
            }
            Self::ClampedIntoRange { min, max } => {
                write!(f, "clamped into [{min}, {max}]")
            }
            Self::ClampedAtMaximum { max } => {
                write!(f, "clamped at maximum {max}")
            }
        }
    }
}

impl std::fmt::Display for ResolveWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Autosnap {
                field,
                requested,
                snapped,
                reason,
            } => {
                write!(
                    f,
                    "field '{field}' snapped from {requested} to {snapped} ({reason})",
                )
            }
            Self::DerivedFromIntent { field, source } => {
                write!(f, "field '{field}' derived from intent {source}")
            }
            Self::DefaultUsed { field } => {
                write!(f, "field '{field}' fell back to material default")
            }
            Self::GenericCalibrationUsed => f.write_str(
                "no calibration block for this printer; the generic machine block was used",
            ),
            Self::SecondTravelStageUnsupported => f.write_str(
                "this printer does not split its travel; the calibrated second stage was dropped",
            ),
        }
    }
}
