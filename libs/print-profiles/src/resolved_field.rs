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
    LiftDistanceMm,
    LiftSpeedMmMin,
    RetractSpeedMmMin,
    ScaleCompensationPct,
}

impl ResolvedField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LayerHeightMm => "layer_height_mm",
            Self::NormalExposureSec => "normal_exposure_sec",
            Self::BottomExposureSec => "bottom_exposure_sec",
            Self::BottomLayerCount => "bottom_layer_count",
            Self::LiftDistanceMm => "lift_distance_mm",
            Self::LiftSpeedMmMin => "lift_speed_mm_min",
            Self::RetractSpeedMmMin => "retract_speed_mm_min",
            Self::ScaleCompensationPct => "scale_compensation_pct",
        }
    }
}

impl std::fmt::Display for ResolvedField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
