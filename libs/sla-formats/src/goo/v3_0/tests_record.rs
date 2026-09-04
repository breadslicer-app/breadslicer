/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
};

use super::record::{LayerValues, RECORD_SIZE, pwm_byte, write_layer};
use crate::print_program::{LayerProgram, MotionStage};

const PARK_Z_MM: f32 = 150.0;

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn speed(value: f64) -> PositiveFiniteMmPerMin {
    PositiveFiniteMmPerMin::new(value).unwrap()
}

fn wait(value: f64) -> NonNegativeFiniteSec {
    NonNegativeFiniteSec::new(value).unwrap()
}

/// A layer whose every slot holds a distinct value, so a test can
/// tell which slot a byte came from.
fn layer() -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(0.35),
        exposure_sec: PositiveFiniteSec::new(2.5).unwrap(),
        wait_before_cure_sec: wait(0.1),
        wait_after_cure_sec: wait(0.2),
        wait_after_lift_sec: wait(0.3),
        lift: MotionStage {
            distance_mm: mm(5.0),
            speed_mm_min: speed(90.0),
        },
        retract: MotionStage {
            distance_mm: mm(6.0),
            speed_mm_min: speed(150.0),
        },
        second_lift: None,
        second_retract: None,
        light_pwm: LightPwm::new(50.0).unwrap(),
    }
}

fn written(layer: &LayerProgram, image: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    write_layer(layer, PARK_Z_MM, image, &mut out).unwrap();
    out
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn a_layer_costs_its_record_its_image_and_2_delimiters() {
    let bytes = written(&layer(), &[0x55, 0x00, 0xFF]);
    assert_eq!(bytes.len(), RECORD_SIZE + 3 + 2);
}

#[test]
fn the_record_opens_unpaused_at_the_park_height() {
    let bytes = written(&layer(), &[]);
    assert_eq!(u16_at(&bytes, 0), 0);
    assert_eq!(f32_at(&bytes, 2), PARK_Z_MM);
}

#[test]
fn the_layer_height_and_exposure_follow_the_program() {
    let bytes = written(&layer(), &[]);
    assert_eq!(f32_at(&bytes, 6), 0.35);
    assert_eq!(f32_at(&bytes, 10), 2.5);
    assert_eq!(f32_at(&bytes, 14), 0.0);
}

#[test]
fn the_waits_follow_the_slot_names_the_record_uses() {
    let bytes = written(&layer(), &[]);
    assert_eq!(f32_at(&bytes, 18), 0.2);
    assert_eq!(f32_at(&bytes, 22), 0.3);
    assert_eq!(f32_at(&bytes, 26), 0.1);
}

#[test]
fn the_motion_slots_interleave_each_stage_with_its_second() {
    let bytes = written(&layer(), &[]);
    assert_eq!(f32_at(&bytes, 30), 5.0);
    assert_eq!(f32_at(&bytes, 34), 90.0);
    assert_eq!(f32_at(&bytes, 38), 0.0);
    assert_eq!(f32_at(&bytes, 42), 0.0);
    assert_eq!(f32_at(&bytes, 46), 6.0);
    assert_eq!(f32_at(&bytes, 50), 150.0);
    assert_eq!(f32_at(&bytes, 54), 0.0);
    assert_eq!(f32_at(&bytes, 58), 0.0);
}

#[test]
fn a_second_stage_reaches_its_own_slots() {
    let staged = MotionStage {
        distance_mm: mm(2.0),
        speed_mm_min: speed(60.0),
    };
    let mut layer = layer();
    layer.second_lift = Some(staged);
    layer.second_retract = Some(staged);
    let bytes = written(&layer, &[]);

    assert_eq!(f32_at(&bytes, 38), 2.0);
    assert_eq!(f32_at(&bytes, 42), 60.0);
    assert_eq!(f32_at(&bytes, 54), 2.0);
    assert_eq!(f32_at(&bytes, 58), 60.0);
}

#[test]
fn the_light_pwm_closes_the_fixed_fields() {
    assert_eq!(u16_at(&written(&layer(), &[]), 62), 128);
}

#[test]
fn a_delimiter_separates_the_fields_from_the_image_length() {
    let bytes = written(&layer(), &[]);
    assert_eq!(&bytes[64..66], &[0x0D, 0x0A]);
}

#[test]
fn the_length_field_counts_the_whole_image() {
    let image = [0x55, 0x30, 0x12, 0x34, 0x56, 0xAB];
    let bytes = written(&layer(), &image);

    assert_eq!(u32_at(&bytes, 66), 6);
    assert_eq!(&bytes[RECORD_SIZE..RECORD_SIZE + 6], &image);
}

#[test]
fn a_delimiter_closes_the_layer() {
    let bytes = written(&layer(), &[0x55, 0xFF]);
    assert_eq!(&bytes[bytes.len() - 2..], &[0x0D, 0x0A]);
}

#[test]
fn a_layer_that_is_absent_reads_as_zeros() {
    assert_eq!(LayerValues::of(None), LayerValues::default());
}

#[test]
fn the_duty_cycle_spans_the_byte_range() {
    assert_eq!(pwm_byte(LightPwm::FULL), 255);
    assert_eq!(pwm_byte(LightPwm::new(0.0).unwrap()), 0);
    assert_eq!(pwm_byte(LightPwm::new(50.0).unwrap()), 128);
    assert_eq!(pwm_byte(LightPwm::new(20.0).unwrap()), 51);
}
