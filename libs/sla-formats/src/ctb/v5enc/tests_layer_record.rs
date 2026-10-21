/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
};

use super::layer_record::{EncryptedRange, LayerImage, write_layer_pointer, write_layer_record};
use super::layout::{LAYER_POINTER_SIZE, LAYER_RECORD_SIZE};
use crate::ctb::page_address::PAGE_BYTES;
use crate::print_program::{LayerProgram, MotionStage};

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn speed(value: f64) -> PositiveFiniteMmPerMin {
    PositiveFiniteMmPerMin::new(value).unwrap()
}

fn stage(distance_mm: f64, speed_mm_min: f64) -> MotionStage {
    MotionStage {
        distance_mm: mm(distance_mm),
        speed_mm_min: speed(speed_mm_min),
    }
}

/// A layer whose every slot holds a distinct value, so a test can
/// tell which slot a byte came from.
fn layer() -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(0.35),
        exposure_sec: PositiveFiniteSec::new(2.5).unwrap(),
        wait_before_cure_sec: NonNegativeFiniteSec::new(0.5).unwrap(),
        wait_after_cure_sec: NonNegativeFiniteSec::new(0.25).unwrap(),
        wait_after_lift_sec: NonNegativeFiniteSec::new(1.25).unwrap(),
        lift: stage(5.5, 91.0),
        retract: stage(6.5, 137.0),
        second_lift: Some(stage(2.75, 243.0)),
        second_retract: Some(stage(3.25, 189.0)),
        light_pwm: LightPwm::new(40.0).unwrap(),
    }
}

fn image() -> LayerImage {
    LayerImage::new(1_000, 64, EncryptedRange::default())
}

fn pointer(record_at: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_layer_pointer(record_at, &mut bytes).unwrap();
    bytes
}

fn record(image: LayerImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_layer_record(&layer(), image, &mut bytes).unwrap();
    bytes
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn each_record_is_the_size_the_layout_reserves_for_it() {
    assert_eq!(pointer(1_000).len(), LAYER_POINTER_SIZE as usize);
    assert_eq!(record(image()).len(), LAYER_RECORD_SIZE as usize);
}

#[test]
fn a_pointer_carries_the_address_of_the_record_and_its_size() {
    let bytes = pointer(99_678);

    assert_eq!(u32_at(&bytes, 0), 99_678);
    assert_eq!(u32_at(&bytes, 4), 0, "one page is enough below 4 GiB");
    assert_eq!(u32_at(&bytes, 8), LAYER_RECORD_SIZE);
    assert_eq!(u32_at(&bytes, 12), 0);
}

#[test]
fn a_pointer_past_four_gibibytes_moves_into_the_next_page() {
    let bytes = pointer(PAGE_BYTES + 17);

    assert_eq!(u32_at(&bytes, 0), 17);
    assert_eq!(u32_at(&bytes, 4), 1);
}

#[test]
fn a_record_opens_with_its_own_size() {
    assert_eq!(u32_at(&record(image()), 0), LAYER_RECORD_SIZE);
}

#[test]
fn a_record_states_where_its_layer_stops_and_how_long_it_is_lit() {
    let bytes = record(image());

    assert_eq!(f32_at(&bytes, 4), 0.35);
    assert_eq!(f32_at(&bytes, 8), 2.5);
    assert_eq!(f32_at(&bytes, 12), 0.0, "the delay the waits replace");
}

#[test]
fn the_page_number_comes_before_the_data_length() {
    let bytes = record(LayerImage::new(
        PAGE_BYTES + 1_000,
        64,
        EncryptedRange::default(),
    ));

    assert_eq!(u32_at(&bytes, 16), 1_000);
    assert_eq!(u32_at(&bytes, 20), 1, "the page, where the plain record");
    assert_eq!(u32_at(&bytes, 24), 64, "has the length");
    assert_eq!(u32_at(&bytes, 28), 0);
}

#[test]
fn a_layer_with_no_encrypted_range_declares_a_length_of_zero() {
    let bytes = record(image());

    assert_eq!(u32_at(&bytes, 32), 0);
    assert_eq!(u32_at(&bytes, 36), 0);
}

#[test]
fn an_encrypted_range_is_stated_against_the_layer_s_own_image() {
    let bytes = record(LayerImage::new(
        1_000,
        4_096,
        EncryptedRange {
            offset: 512,
            length: 1_024,
        },
    ));

    assert_eq!(u32_at(&bytes, 32), 512);
    assert_eq!(u32_at(&bytes, 36), 1_024);
}

#[test]
fn the_lift_height_is_both_stages_and_the_second_follows_on_its_own() {
    let bytes = record(image());

    assert_eq!(f32_at(&bytes, 40), 5.5 + 2.75);
    assert_eq!(f32_at(&bytes, 44), 91.0);
    assert_eq!(f32_at(&bytes, 48), 2.75);
    assert_eq!(f32_at(&bytes, 52), 243.0);
}

#[test]
fn the_retract_speed_comes_before_the_second_retract() {
    let bytes = record(image());

    assert_eq!(f32_at(&bytes, 56), 137.0);
    assert_eq!(f32_at(&bytes, 60), 3.25);
    assert_eq!(f32_at(&bytes, 64), 189.0);
}

#[test]
fn each_wait_is_named_by_the_motion_it_brackets() {
    let bytes = record(image());

    assert_eq!(f32_at(&bytes, 68), 0.25, "after the cure, before the lift");
    assert_eq!(f32_at(&bytes, 72), 1.25, "after the lift");
    assert_eq!(f32_at(&bytes, 76), 0.5, "after the retract, before curing");
}

#[test]
fn the_light_level_is_stored_as_a_float_here() {
    let bytes = record(image());

    assert_eq!(f32_at(&bytes, 80), 102.0, "40 percent of 255");
    assert_eq!(u32_at(&bytes, 84), 0);
}
