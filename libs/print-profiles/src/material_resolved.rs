/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resolved material-side values, ready to drive a print.
//!
//! [`MaterialResolved`] is the output of the resolver's
//! material-side composition: pick a calibrated value, fall back to
//! a material default, derive from intent, or apply a scene
//! override. Most fields are required; the resolver errors with
//! [`crate::resolve_error::ResolveError::MissingRequiredField`]
//! when no resolution path supplies one. The waits and the light
//! PWMs are the exceptions: an uncalibrated wait is a wait of zero,
//! since a resin that names no pause is asking for none, and an
//! uncalibrated PWM is full power, since a resin that names no duty
//! cycle is asking for all of it. An uncalibrated bottom wait takes
//! the normal-layer value, since a resin that draws no distinction
//! between the two wants one pause everywhere.
//!
//! Each field uses the per-unit primitive that carries the
//! positive-finite invariant on the type, matching the section
//! types in [`crate::material_per_printer::MaterialPerPrinter`] and
//! [`crate::material_defaults::MaterialDefaults`].

use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
    ScaleCompensation,
};

use crate::material_identity::MaterialIdentity;

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialResolved {
    pub identity: MaterialIdentity,
    pub layer_height_mm: PositiveFiniteMm,
    pub normal_exposure_sec: PositiveFiniteSec,
    pub bottom_exposure_sec: PositiveFiniteSec,
    pub bottom_layer_count: u32,
    /// Pause after the platform returns, before the light comes on.
    pub wait_before_cure_sec: NonNegativeFiniteSec,
    /// Pause after the light goes off, before the lift starts.
    pub wait_after_cure_sec: NonNegativeFiniteSec,
    /// Pause at the top of the lift.
    pub wait_after_lift_sec: NonNegativeFiniteSec,
    /// The bottom layers' pause before the light comes on.
    pub bottom_wait_before_cure_sec: NonNegativeFiniteSec,
    /// The bottom layers' pause after the light goes off.
    pub bottom_wait_after_cure_sec: NonNegativeFiniteSec,
    /// The bottom layers' pause at the top of the lift.
    pub bottom_wait_after_lift_sec: NonNegativeFiniteSec,
    pub lift_distance_mm: PositiveFiniteMm,
    pub lift_speed_mm_min: PositiveFiniteMmPerMin,
    pub retract_speed_mm_min: PositiveFiniteMmPerMin,
    /// How hard the light is driven on a normal layer.
    pub light_pwm: LightPwm,
    /// How hard the light is driven on a bottom layer.
    pub bottom_light_pwm: LightPwm,
    pub scale_compensation_pct: ScaleCompensation,
}
