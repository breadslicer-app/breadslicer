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
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
};
use serde::{Deserialize, Serialize};

use crate::second_travel_stage::SecondTravelStage;

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
    /// How far the platform rises off a bottom layer; absent means
    /// it rises as far as it does off a normal one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_lift_distance_mm: Option<PositiveFiniteMm>,
    /// How fast the platform rises off a bottom layer; absent means
    /// it rises as fast as it does off a normal one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    /// How fast the platform returns onto a bottom layer; absent
    /// means it returns as fast as it does onto a normal one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    /// How hard the light is driven on a normal layer. Average
    /// optical power is linear in it, so an exposure derived from a
    /// target energy density scales with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_pwm: Option<LightPwm>,
    /// How hard the light is driven on a bottom layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_light_pwm: Option<LightPwm>,
    /// The second stage of the travel over a normal layer; absent
    /// means the platform moves in one stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_travel: Option<SecondTravelStage>,
    /// The second stage of the travel over a bottom layer; absent
    /// means the bottom layers travel like the normal ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom_second_travel: Option<SecondTravelStage>,
}
