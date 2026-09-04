/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! What a printer file asks the machine to do, layer by layer.
//!
//! GOO and CTB both store a full copy of the exposure, motion and
//! light settings on every layer.

use sc_print_primitives::{
    LightPwm, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin,
    PositiveFiniteSec,
};

use crate::raster_setup::RasterSetup;

/// One stage of build-platform travel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionStage {
    pub distance_mm: PositiveFiniteMm,
    pub speed_mm_min: PositiveFiniteMmPerMin,
}

/// One layer's machine record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerProgram {
    /// Top of this layer's slab, measured from the plate.
    pub slab_top_mm: PositiveFiniteMm,
    pub exposure_sec: PositiveFiniteSec,
    /// Pause after the platform returns, before the light comes on.
    pub wait_before_cure_sec: NonNegativeFiniteSec,
    /// Pause after the light goes off, before the lift starts.
    pub wait_after_cure_sec: NonNegativeFiniteSec,
    /// Pause at the top of the lift.
    pub wait_after_lift_sec: NonNegativeFiniteSec,
    pub lift: MotionStage,
    pub retract: MotionStage,
    /// Present only on machines that split the travel in two.
    pub second_lift: Option<MotionStage>,
    /// Present only on machines that split the travel in two.
    pub second_retract: Option<MotionStage>,
    pub light_pwm: LightPwm,
}

/// Everything a printer file records apart from the layer images.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintProgram {
    pub raster: RasterSetup,
    pub machine_name: NonEmptyString,
    pub material_name: NonEmptyString,
    pub layers: Vec<LayerProgram>,
    pub slicer_version: &'static str,
}
