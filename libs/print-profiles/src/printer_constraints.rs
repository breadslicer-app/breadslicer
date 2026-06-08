/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Constraint block of a printer profile.
//!
//! [`PrinterConstraints`] mirrors the `[constraints]` section of a
//! printer-profile TOML file: every field is optional, and a missing
//! field means "no printer-side constraint on this dimension". The
//! TOML shape per field communicates whether the constraint is a
//! quantisation step (`z_step_mm`), an upper bound expressed as a
//! single scalar (`lift_distance_max_mm`), a measured hardware
//! quantity (`light_intensity_mw_per_cm2`), or a `{ min, max }`
//! envelope (the range fields). The resolver consumes these values
//! and enforces per-field policies.

use sc_print_primitives::{
    LengthRange, PositiveFiniteMm, PositiveFiniteMwPerCm2, SpeedRangeMmPerMin,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct PrinterConstraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_step_mm: Option<PositiveFiniteMm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_height_range_mm: Option<LengthRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lift_speed_range_mm_min: Option<SpeedRangeMmPerMin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retract_speed_range_mm_min: Option<SpeedRangeMmPerMin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lift_distance_max_mm: Option<PositiveFiniteMm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_intensity_mw_per_cm2: Option<PositiveFiniteMwPerCm2>,
}
