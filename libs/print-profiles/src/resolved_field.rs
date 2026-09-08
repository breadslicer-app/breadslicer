/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Identifier for each scalar field the resolver fills in a
//! resolved print configuration.
//!
//! Used as the key for warnings, errors, the per-field provenance
//! table, and the constraint policy table. The `as_str`
//! representation is stable and matches the TOML key on the
//! corresponding profile section, dotted for a value that lives
//! inside a section's own table.

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
    BottomLiftDistanceMm,
    BottomLiftSpeedMmMin,
    BottomRetractSpeedMmMin,
    SecondLiftDistanceMm,
    SecondLiftSpeedMmMin,
    SecondRetractSpeedMmMin,
    BottomSecondLiftDistanceMm,
    BottomSecondLiftSpeedMmMin,
    BottomSecondRetractSpeedMmMin,
    LightPwm,
    BottomLightPwm,
    ScaleCompensationPct,
    CuredDensityGPerCm3,
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
            Self::BottomLiftDistanceMm => "bottom_lift_distance_mm",
            Self::BottomLiftSpeedMmMin => "bottom_lift_speed_mm_min",
            Self::BottomRetractSpeedMmMin => "bottom_retract_speed_mm_min",
            Self::SecondLiftDistanceMm => "second_travel.lift_distance_mm",
            Self::SecondLiftSpeedMmMin => "second_travel.lift_speed_mm_min",
            Self::SecondRetractSpeedMmMin => "second_travel.retract_speed_mm_min",
            Self::BottomSecondLiftDistanceMm => "bottom_second_travel.lift_distance_mm",
            Self::BottomSecondLiftSpeedMmMin => "bottom_second_travel.lift_speed_mm_min",
            Self::BottomSecondRetractSpeedMmMin => "bottom_second_travel.retract_speed_mm_min",
            Self::LightPwm => "light_pwm",
            Self::BottomLightPwm => "bottom_light_pwm",
            Self::ScaleCompensationPct => "scale_compensation_pct",
            Self::CuredDensityGPerCm3 => "cured_density_g_per_cm3",
        }
    }
}

impl std::fmt::Display for ResolvedField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
