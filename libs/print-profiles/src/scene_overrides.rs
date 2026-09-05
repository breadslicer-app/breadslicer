/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-print overrides for the resolver's material-side outputs.
//!
//! [`SceneOverrides`] mirrors the scalar fields of
//! [`crate::material_resolved::MaterialResolved`], each as an
//! `Option<T>`. The deny list of fields that may not be overridden
//! (printer hardware geometry, identity, printer constraints, and
//! printer-side hardware capabilities) is structurally enforced by
//! their absence from this struct: a caller cannot encode an
//! override on a denied field because no field exists for it.
//!
//! The struct is `Default`-able with all fields `None`, so a caller
//! that only wants to override one field builds
//! `SceneOverrides { layer_height_mm: Some(value), ..Default::default() }`.

use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
    ScaleCompensation,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneOverrides {
    pub layer_height_mm: Option<PositiveFiniteMm>,
    pub normal_exposure_sec: Option<PositiveFiniteSec>,
    pub bottom_exposure_sec: Option<PositiveFiniteSec>,
    pub bottom_layer_count: Option<u32>,
    pub wait_before_cure_sec: Option<NonNegativeFiniteSec>,
    pub wait_after_cure_sec: Option<NonNegativeFiniteSec>,
    pub wait_after_lift_sec: Option<NonNegativeFiniteSec>,
    pub bottom_wait_before_cure_sec: Option<NonNegativeFiniteSec>,
    pub bottom_wait_after_cure_sec: Option<NonNegativeFiniteSec>,
    pub bottom_wait_after_lift_sec: Option<NonNegativeFiniteSec>,
    pub lift_distance_mm: Option<PositiveFiniteMm>,
    pub lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub bottom_lift_distance_mm: Option<PositiveFiniteMm>,
    pub bottom_lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub bottom_retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub light_pwm: Option<LightPwm>,
    pub bottom_light_pwm: Option<LightPwm>,
    pub scale_compensation_pct: Option<ScaleCompensation>,
}
