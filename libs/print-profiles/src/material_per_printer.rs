/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-printer calibrated values for a material profile.
//!
//! [`MaterialPerPrinter`] mirrors the `[per_printer."<id>"]` section
//! of a material-profile TOML file: the values a vendor has
//! calibrated for a specific printer. Every field is optional, and a
//! missing field falls through to the resin-level
//! [`super::material_defaults::MaterialDefaults`] (when present) and
//! then to intent-derivation. The presence of a `[per_printer.<id>]`
//! section is itself the calibration assertion; the fields it
//! carries are the values the vendor stands behind for that printer.

use sc_print_primitives::{PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct MaterialPerPrinter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal_exposure_sec: Option<PositiveFiniteSec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_exposure_sec: Option<PositiveFiniteSec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_layer_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lift_distance_mm: Option<PositiveFiniteMm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
}
