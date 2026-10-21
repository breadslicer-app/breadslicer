/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
};

use super::layer_record::{ImageLocation, write_layer_record, write_table_record};
use super::layout::{LAYER_RECORD_SIZE, LAYER_TABLE_RECORD_SIZE};
use crate::print_program::{LayerProgram, MotionStage};

const PAGE_SIZE: u64 = 1 << 32;

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

fn table_record(layer: &LayerProgram, image: ImageLocation) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_table_record(layer, image, &mut bytes).unwrap();
    bytes
}

fn layer_record(layer: &LayerProgram, image: ImageLocation) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_layer_record(layer, image, &mut bytes).unwrap();
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
    let image = ImageLocation::new(1_000, 64);

    assert_eq!(
        table_record(&layer(), image).len(),
        LAYER_TABLE_RECORD_SIZE as usize,
    );
    assert_eq!(
        layer_record(&layer(), image).len(),
        LAYER_RECORD_SIZE as usize,
    );
}

#[test]
fn the_fuller_record_opens_with_the_table_entry_byte_for_byte() {
    let image = ImageLocation::new(1_000, 64);
    let table = table_record(&layer(), image);

    assert_eq!(
        &layer_record(&layer(), image)[..LAYER_TABLE_RECORD_SIZE as usize],
        &table[..],
    );
}

#[test]
fn the_table_entry_states_the_layers_height_and_exposure() {
    let bytes = table_record(&layer(), ImageLocation::new(1_000, 64));

    assert_eq!(f32_at(&bytes, 0), 0.35);
    assert_eq!(f32_at(&bytes, 4), 2.5);
}

#[test]
fn the_light_off_time_is_zero_because_the_waits_carry_the_pause() {
    let bytes = table_record(&layer(), ImageLocation::new(1_000, 64));

    assert_eq!(f32_at(&bytes, 8), 0.0);
}

#[test]
fn the_table_entry_locates_the_image_and_counts_it() {
    let bytes = table_record(&layer(), ImageLocation::new(1_000, 64));

    assert_eq!(u32_at(&bytes, 12), 1_000);
    assert_eq!(u32_at(&bytes, 16), 64);
    assert_eq!(u32_at(&bytes, 20), 0, "one page is enough below 4 GiB");
}

#[test]
fn an_address_past_four_gibibytes_moves_into_the_next_page() {
    let bytes = table_record(&layer(), ImageLocation::new(PAGE_SIZE + 17, 64));

    assert_eq!(u32_at(&bytes, 12), 17);
    assert_eq!(u32_at(&bytes, 20), 1);
}

#[test]
fn the_last_address_of_a_page_stays_in_it() {
    let bytes = table_record(&layer(), ImageLocation::new(PAGE_SIZE - 1, 64));

    assert_eq!(u32_at(&bytes, 12), u32::MAX);
    assert_eq!(u32_at(&bytes, 20), 0);
}

#[test]
fn both_records_declare_the_fuller_records_size() {
    let image = ImageLocation::new(1_000, 64);

    assert_eq!(
        u32_at(&table_record(&layer(), image), 24),
        LAYER_RECORD_SIZE
    );
    assert_eq!(
        u32_at(&layer_record(&layer(), image), 24),
        LAYER_RECORD_SIZE
    );
}

#[test]
fn the_table_entry_ends_in_reserved_bytes() {
    let bytes = table_record(&layer(), ImageLocation::new(1_000, 64));

    assert!(bytes[28..].iter().all(|byte| *byte == 0));
}

#[test]
fn the_total_size_covers_the_record_and_the_image_it_precedes() {
    let bytes = layer_record(&layer(), ImageLocation::new(1_000, 64));

    assert_eq!(u32_at(&bytes, 36), LAYER_RECORD_SIZE + 64);
}

#[test]
fn the_lift_height_is_both_stages_added_together() {
    let bytes = layer_record(&layer(), ImageLocation::new(1_000, 64));

    assert_eq!(f32_at(&bytes, 40), 8.25);
    assert_eq!(f32_at(&bytes, 48), 2.75, "the second stage on its own");
}

#[test]
fn the_motion_slots_follow_the_layers_own_program() {
    let bytes = layer_record(&layer(), ImageLocation::new(1_000, 64));

    assert_eq!(f32_at(&bytes, 44), 91.0);
    assert_eq!(f32_at(&bytes, 52), 243.0);
    assert_eq!(f32_at(&bytes, 56), 137.0);
    assert_eq!(f32_at(&bytes, 60), 3.25);
    assert_eq!(f32_at(&bytes, 64), 189.0);
}

#[test]
fn the_waits_land_in_the_slots_that_name_their_motion() {
    let bytes = layer_record(&layer(), ImageLocation::new(1_000, 64));

    assert_eq!(f32_at(&bytes, 68), 0.25, "once the light goes off");
    assert_eq!(f32_at(&bytes, 72), 1.25, "at the top of the lift");
    assert_eq!(f32_at(&bytes, 76), 0.5, "once the platform is back");
}

#[test]
fn the_duty_cycle_closes_the_record_as_a_float() {
    let bytes = layer_record(&layer(), ImageLocation::new(1_000, 64));

    assert_eq!(f32_at(&bytes, 80), 102.0);
}

#[test]
fn a_single_stage_machine_leaves_its_second_stage_slots_at_zero() {
    let mut layer = layer();
    layer.second_lift = None;
    layer.second_retract = None;
    let bytes = layer_record(&layer, ImageLocation::new(1_000, 64));

    assert_eq!(f32_at(&bytes, 40), 5.5, "the only stage is the whole lift");
    assert_eq!(f32_at(&bytes, 48), 0.0);
    assert_eq!(f32_at(&bytes, 52), 0.0);
    assert_eq!(f32_at(&bytes, 60), 0.0);
    assert_eq!(f32_at(&bytes, 64), 0.0);
}
