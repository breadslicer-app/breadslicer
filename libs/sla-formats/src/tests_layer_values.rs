/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
};

use crate::layer_values::{LayerValues, pwm_byte};
use crate::print_program::{LayerProgram, MotionStage};

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn speed(value: f64) -> PositiveFiniteMmPerMin {
    PositiveFiniteMmPerMin::new(value).unwrap()
}

fn wait(value: f64) -> NonNegativeFiniteSec {
    NonNegativeFiniteSec::new(value).unwrap()
}

fn layer() -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(0.4),
        exposure_sec: PositiveFiniteSec::new(2.5).unwrap(),
        wait_before_cure_sec: wait(0.3),
        wait_after_cure_sec: wait(0.7),
        wait_after_lift_sec: wait(1.1),
        lift: MotionStage {
            distance_mm: mm(5.0),
            speed_mm_min: speed(90.0),
        },
        retract: MotionStage {
            distance_mm: mm(4.0),
            speed_mm_min: speed(150.0),
        },
        second_lift: None,
        second_retract: None,
        light_pwm: LightPwm::FULL,
    }
}

#[test]
fn a_layer_that_is_absent_reads_as_zeros() {
    assert_eq!(LayerValues::of(None), LayerValues::default());
}

#[test]
fn each_wait_moves_to_the_motion_it_brackets() {
    let values = LayerValues::of(Some(&layer()));

    assert_eq!(values.wait_after_retract_sec, 0.3);
    assert_eq!(values.wait_before_lift_sec, 0.7);
    assert_eq!(values.wait_after_lift_sec, 1.1);
}

#[test]
fn the_first_stage_carries_its_distance_and_speed() {
    let values = LayerValues::of(Some(&layer()));

    assert_eq!(values.exposure_sec, 2.5);
    assert_eq!(values.lift_distance_mm, 5.0);
    assert_eq!(values.lift_speed_mm_min, 90.0);
    assert_eq!(values.retract_distance_mm, 4.0);
    assert_eq!(values.retract_speed_mm_min, 150.0);
}

#[test]
fn a_machine_without_a_second_stage_reads_zeros_in_its_slots() {
    let values = LayerValues::of(Some(&layer()));

    assert_eq!(values.second_lift_distance_mm, 0.0);
    assert_eq!(values.second_lift_speed_mm_min, 0.0);
    assert_eq!(values.second_retract_distance_mm, 0.0);
    assert_eq!(values.second_retract_speed_mm_min, 0.0);
}

#[test]
fn a_second_stage_reaches_its_own_slots() {
    let mut layer = layer();
    layer.second_lift = Some(MotionStage {
        distance_mm: mm(3.0),
        speed_mm_min: speed(240.0),
    });
    layer.second_retract = Some(MotionStage {
        distance_mm: mm(2.0),
        speed_mm_min: speed(210.0),
    });
    let values = LayerValues::of(Some(&layer));

    assert_eq!(values.second_lift_distance_mm, 3.0);
    assert_eq!(values.second_lift_speed_mm_min, 240.0);
    assert_eq!(values.second_retract_distance_mm, 2.0);
    assert_eq!(values.second_retract_speed_mm_min, 210.0);
    assert_eq!(values.lift_distance_mm, 5.0, "the first stage is untouched");
}

#[test]
fn the_duty_cycle_spans_the_byte_range() {
    assert_eq!(pwm_byte(LightPwm::FULL), 255);
    assert_eq!(pwm_byte(LightPwm::new(0.0).unwrap()), 0);
    assert_eq!(pwm_byte(LightPwm::new(50.0).unwrap()), 128);
    assert_eq!(pwm_byte(LightPwm::new(20.0).unwrap()), 51);
}

#[test]
fn a_layer_carries_its_duty_cycle_through() {
    let mut layer = layer();
    layer.light_pwm = LightPwm::new(40.0).unwrap();

    assert_eq!(LayerValues::of(Some(&layer)).light_pwm, 102);
}
