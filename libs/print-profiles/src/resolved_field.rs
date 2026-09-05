/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Identifier for each scalar field the resolver fills in a
//! resolved print configuration.
//!
//! Used as the key for warnings, errors, the per-field provenance
//! table, and the constraint policy table. The `as_str`
//! representation is stable and matches the TOML field name on the
//! corresponding profile section.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResolvedField {
    LayerHeightMm,
    NormalExposureSec,
    BottomExposureSec,
    BottomLayerCount,
    WaitBeforeCureSec,
    WaitAfterCureSec,
    WaitAfterLiftSec,
    BottomWaitBeforeCureSec,
    BottomWaitAfterCureSec,
    BottomWaitAfterLiftSec,
    LiftDistanceMm,
    LiftSpeedMmMin,
    RetractSpeedMmMin,
    LightPwm,
    BottomLightPwm,
    ScaleCompensationPct,
}

impl ResolvedField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LayerHeightMm => "layer_height_mm",
            Self::NormalExposureSec => "normal_exposure_sec",
            Self::BottomExposureSec => "bottom_exposure_sec",
            Self::BottomLayerCount => "bottom_layer_count",
            Self::WaitBeforeCureSec => "wait_before_cure_sec",
            Self::WaitAfterCureSec => "wait_after_cure_sec",
            Self::WaitAfterLiftSec => "wait_after_lift_sec",
            Self::BottomWaitBeforeCureSec => "bottom_wait_before_cure_sec",
            Self::BottomWaitAfterCureSec => "bottom_wait_after_cure_sec",
            Self::BottomWaitAfterLiftSec => "bottom_wait_after_lift_sec",
            Self::LiftDistanceMm => "lift_distance_mm",
            Self::LiftSpeedMmMin => "lift_speed_mm_min",
            Self::RetractSpeedMmMin => "retract_speed_mm_min",
            Self::LightPwm => "light_pwm",
            Self::BottomLightPwm => "bottom_light_pwm",
            Self::ScaleCompensationPct => "scale_compensation_pct",
        }
    }
}

impl std::fmt::Display for ResolvedField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
