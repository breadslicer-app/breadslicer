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

use sc_print_primitives::{
    NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct MaterialPerPrinter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal_exposure_sec: Option<PositiveFiniteSec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_exposure_sec: Option<PositiveFiniteSec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_layer_count: Option<u32>,
    /// Pause after the platform returns, before the light comes on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_before_cure_sec: Option<NonNegativeFiniteSec>,
    /// Pause after the light goes off, before the lift starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_after_cure_sec: Option<NonNegativeFiniteSec>,
    /// Pause at the top of the lift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_after_lift_sec: Option<NonNegativeFiniteSec>,
    /// The bottom layers' pause before the light comes on; absent
    /// means they hold the same pause as the normal layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_wait_before_cure_sec: Option<NonNegativeFiniteSec>,
    /// The bottom layers' pause after the light goes off; absent
    /// means they hold the same pause as the normal layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_wait_after_cure_sec: Option<NonNegativeFiniteSec>,
    /// The bottom layers' pause at the top of the lift; absent means
    /// they hold the same pause as the normal layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_wait_after_lift_sec: Option<NonNegativeFiniteSec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lift_distance_mm: Option<PositiveFiniteMm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
}
