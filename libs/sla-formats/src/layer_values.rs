/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! One layer's settings in the widths a printer file stores.
//!
//! GOO and CTB both record every layer in full rather than referring
//! back to their headers, and both give the second stage of the
//! platform travel its own slots, which a single-stage machine leaves
//! at zero. One projection serves both.

use sc_print_primitives::LightPwm;

use crate::print_program::LayerProgram;

/// One layer's settings, named as these containers name their slots
/// and converted to the widths they store. A layer that is absent,
/// which happens when a header summarises an empty program, reads as
/// zeros.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LayerValues {
    pub exposure_sec: f32,
    pub wait_before_lift_sec: f32,
    pub wait_after_lift_sec: f32,
    pub wait_after_retract_sec: f32,
    pub lift_distance_mm: f32,
    pub lift_speed_mm_min: f32,
    pub retract_distance_mm: f32,
    pub retract_speed_mm_min: f32,
    pub second_lift_distance_mm: f32,
    pub second_lift_speed_mm_min: f32,
    pub second_retract_distance_mm: f32,
    pub second_retract_speed_mm_min: f32,
    pub light_pwm: u16,
}

impl LayerValues {
    /// Take one layer's settings, or zeros when there is no layer.
    pub(crate) fn of(layer: Option<&LayerProgram>) -> Self {
        let Some(layer) = layer else {
            return Self::default();
        };
        let (second_lift_distance_mm, second_lift_speed_mm_min) =
            layer.second_lift.map_or((0.0, 0.0), |stage| {
                (
                    stage.distance_mm.value() as f32,
                    stage.speed_mm_min.value() as f32,
                )
            });
        let (second_retract_distance_mm, second_retract_speed_mm_min) =
            layer.second_retract.map_or((0.0, 0.0), |stage| {
                (
                    stage.distance_mm.value() as f32,
                    stage.speed_mm_min.value() as f32,
                )
            });

        Self {
            exposure_sec: layer.exposure_sec.value() as f32,
            // The containers name a wait by the motion it precedes or
            // follows; a layer program names it by the exposure.
            wait_before_lift_sec: layer.wait_after_cure_sec.value() as f32,
            wait_after_lift_sec: layer.wait_after_lift_sec.value() as f32,
            wait_after_retract_sec: layer.wait_before_cure_sec.value() as f32,
            lift_distance_mm: layer.lift.distance_mm.value() as f32,
            lift_speed_mm_min: layer.lift.speed_mm_min.value() as f32,
            retract_distance_mm: layer.retract.distance_mm.value() as f32,
            retract_speed_mm_min: layer.retract.speed_mm_min.value() as f32,
            second_lift_distance_mm,
            second_lift_speed_mm_min,
            second_retract_distance_mm,
            second_retract_speed_mm_min,
            light_pwm: pwm_byte(layer.light_pwm),
        }
    }
}

/// Convert a duty cycle to the 0..255 scale these containers store.
pub(crate) fn pwm_byte(pwm: LightPwm) -> u16 {
    (pwm.percent() / 100.0 * 255.0).round() as u16
}
