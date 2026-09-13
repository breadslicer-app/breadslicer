/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::num::NonZeroU32;

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::layout::{Layout, SETTINGS_SIZE, VariableSections};
use super::settings::{SETTINGS_BYTES, settings_block};
use crate::ctb::cipher::BLOCK_BYTES;
use crate::ctb::disclaimer::DISCLAIMER_SIZE;
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

/// A whole number of minutes, so the conversion is visible without a
/// remainder confusing it.
const CREATED_SEC: u64 = 1_788_721_620;
const CREATED_MIN: u32 = 29_812_027;

const MACHINE: &str = "ELEGOO Mars 5 Ultra";
const SIGNED_VALUE: u64 = 0;

fn key() -> NonZeroU32 {
    NonZeroU32::new(1_755_969_410).unwrap()
}

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

fn raster(aa: AaLevel) -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(153.36, 77.76, 165.0).unwrap(),
        PixelResolution::new(8520, 4320).unwrap(),
        LayerSchedule::uniform(0.05, 0.05, 2).unwrap(),
        Mirror::default(),
        aa,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

/// Bottom and normal layers whose every slot differs, so a test can
/// tell which layer and which stage a byte came from.
fn layer(bottom: bool) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(if bottom { 0.05 } else { 0.10 }),
        exposure_sec: PositiveFiniteSec::new(if bottom { 31.0 } else { 2.7 }).unwrap(),
        wait_before_cure_sec: NonNegativeFiniteSec::new(0.3).unwrap(),
        wait_after_cure_sec: NonNegativeFiniteSec::new(0.7).unwrap(),
        wait_after_lift_sec: NonNegativeFiniteSec::new(1.1).unwrap(),
        lift: if bottom {
            stage(7.25, 63.0)
        } else {
            stage(5.5, 91.0)
        },
        retract: if bottom {
            stage(6.0, 151.0)
        } else {
            stage(6.5, 137.0)
        },
        second_lift: Some(if bottom {
            stage(1.75, 219.0)
        } else {
            stage(2.75, 243.0)
        }),
        second_retract: Some(if bottom {
            stage(3.75, 171.0)
        } else {
            stage(3.25, 189.0)
        }),
        light_pwm: if bottom {
            LightPwm::FULL
        } else {
            LightPwm::new(40.0).unwrap()
        },
    }
}

fn program(aa: AaLevel) -> PrintProgram {
    PrintProgram {
        raster: raster(aa),
        machine_name: NonEmptyString::new(MACHINE).unwrap(),
        material_name: NonEmptyString::new("Magna Durable").unwrap(),
        layers: vec![layer(true), layer(false)],
        layer_height_mm: mm(0.05),
        bottom_layer_count: 3,
        transition_layer_count: 5,
        volume_mm3: VolumeMm3::new(15_000.0).unwrap(),
        mass_g: MassG::new(17.5).unwrap(),
        print_time_sec: NonNegativeFiniteSec::new(5664.0).unwrap(),
        created_at: CreationTime {
            rendered: NonEmptyString::new("2026-09-04 21:07:00").unwrap(),
            unix_sec: CREATED_SEC,
        },
        slicer_version: "0.0.12",
    }
}

fn layout() -> Layout {
    Layout::plan(VariableSections {
        large_preview_bytes: 5_000,
        small_preview_bytes: 900,
        machine_name_bytes: u32::try_from(MACHINE.len()).unwrap(),
        resin_parameters_bytes: 64,
        layer_count: 2,
    })
}

fn built(program: &PrintProgram) -> [u8; SETTINGS_BYTES] {
    settings_block(program, &layout(), key(), SIGNED_VALUE).unwrap()
}

fn u8_at(bytes: &[u8], offset: usize) -> u8 {
    bytes[offset]
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn the_block_is_the_size_the_layout_reserves_and_the_cipher_needs() {
    assert_eq!(SETTINGS_BYTES, SETTINGS_SIZE as usize);
    assert_eq!(SETTINGS_BYTES % BLOCK_BYTES, 0);
}

#[test]
fn the_signed_value_opens_the_block() {
    let bytes = settings_block(&program(AaLevel::Off), &layout(), key(), 0x6A99_9FED).unwrap();

    assert_eq!(u64_at(&bytes, 0), 0x6A99_9FED);
}

#[test]
fn the_plate_is_the_one_the_raster_was_set_up_for() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 12), 153.36);
    assert_eq!(f32_at(&bytes, 16), 77.76);
    assert_eq!(f32_at(&bytes, 20), 165.0);
    assert_eq!(u32_at(&bytes, 56), 8520);
    assert_eq!(u32_at(&bytes, 60), 4320);
}

#[test]
fn the_schedule_summary_comes_from_the_first_and_last_layers() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 32), 0.10, "the last layer's top");
    assert_eq!(f32_at(&bytes, 36), 0.05);
    assert_eq!(f32_at(&bytes, 40), 2.7);
    assert_eq!(f32_at(&bytes, 44), 31.0);
    assert_eq!(u32_at(&bytes, 52), 3);
    assert_eq!(u32_at(&bytes, 64), 2);
    assert_eq!(u32_at(&bytes, 76), 5664);
    assert_eq!(u32_at(&bytes, 188), 5);
}

#[test]
fn every_light_off_delay_is_left_at_zero_beside_the_waits() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 48), 0.0);
    assert_eq!(f32_at(&bytes, 116), 0.0);
    assert_eq!(f32_at(&bytes, 180), 0.3, "wait before cure");
    assert_eq!(f32_at(&bytes, 184), 1.1, "wait after lift");
    assert_eq!(f32_at(&bytes, 216), 0.3);
    assert_eq!(f32_at(&bytes, 220), 1.1);
    assert_eq!(f32_at(&bytes, 224), 0.7, "wait after cure");
    assert_eq!(f32_at(&bytes, 156), 1.1);
}

#[test]
fn the_lift_heights_are_both_stages_together() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 84), 7.25 + 1.75);
    assert_eq!(f32_at(&bytes, 88), 63.0);
    assert_eq!(f32_at(&bytes, 92), 5.5 + 2.75);
    assert_eq!(f32_at(&bytes, 96), 91.0);
    assert_eq!(f32_at(&bytes, 100), 137.0);
}

#[test]
fn the_second_stage_is_recorded_on_its_own() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 132), 1.75);
    assert_eq!(f32_at(&bytes, 136), 219.0);
    assert_eq!(f32_at(&bytes, 140), 2.75);
    assert_eq!(f32_at(&bytes, 144), 243.0);
    assert_eq!(f32_at(&bytes, 148), 3.25);
    assert_eq!(f32_at(&bytes, 152), 189.0);
    assert_eq!(f32_at(&bytes, 192), 151.0, "the bottom retract speed");
    assert_eq!(f32_at(&bytes, 196), 171.0);
    assert_eq!(f32_at(&bytes, 228), 3.75);
}

#[test]
fn the_material_is_reported_in_the_units_the_display_uses() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 104), 15.0, "millilitres");
    assert_eq!(f32_at(&bytes, 108), 17.5);
    assert_eq!(f32_at(&bytes, 112), 0.0, "nothing prices a print");
}

#[test]
fn each_layer_carries_its_own_light_level() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(u16_at(&bytes, 124), 102, "40 percent of 255");
    assert_eq!(u16_at(&bytes, 126), 255);
}

#[test]
fn the_key_the_images_are_obfuscated_with_is_recorded() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(u32_at(&bytes, 128), key().get());
}

#[test]
fn every_section_after_the_block_is_located_from_it() {
    let bytes = built(&program(AaLevel::Off));
    let layout = layout();

    assert_eq!(u32_at(&bytes, 8), layout.layer_pointer_table);
    assert_eq!(u32_at(&bytes, 68), layout.large_preview_record);
    assert_eq!(u32_at(&bytes, 72), layout.small_preview_record);
    assert_eq!(u32_at(&bytes, 160), layout.machine_name);
    assert_eq!(u32_at(&bytes, 164), MACHINE.len() as u32);
    assert_eq!(u32_at(&bytes, 264), layout.disclaimer);
    assert_eq!(u32_at(&bytes, 268), DISCLAIMER_SIZE);
    assert_eq!(u32_at(&bytes, 276), layout.resin_parameters);
}

#[test]
fn the_anti_alias_flag_follows_the_level() {
    assert_eq!(u8_at(&built(&program(AaLevel::Off)), 168), 0x07);
    assert_eq!(u32_at(&built(&program(AaLevel::Off)), 176), 1);
    assert_eq!(u8_at(&built(&program(AaLevel::X4)), 168), 0x0F);
    assert_eq!(u32_at(&built(&program(AaLevel::X4)), 176), 4);
}

#[test]
fn a_layer_may_override_the_summary_in_this_block() {
    assert_eq!(u8_at(&built(&program(AaLevel::Off)), 171), 0x40);
}

#[test]
fn the_file_is_stamped_in_minutes() {
    assert_eq!(u32_at(&built(&program(AaLevel::Off)), 172), CREATED_MIN);
}

#[test]
fn the_still_time_covers_every_layer() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 232), 31.0 + 2.7 + 2.0 * (0.3 + 0.7 + 1.1));
}

#[test]
fn the_last_layer_slot_holds_the_count_as_a_real_file_does() {
    assert_eq!(u32_at(&built(&program(AaLevel::Off)), 244), 2);
}

#[test]
fn the_unexplained_slots_are_written_as_a_real_file_carries_them() {
    let bytes = built(&program(AaLevel::Off));

    assert_eq!(u32_at(&bytes, 120), 1, "one pass over each layer");
    assert_eq!(f32_at(&bytes, 204), 4.0);
    assert_eq!(f32_at(&bytes, 212), 4.0);
    assert_eq!(u32_at(&bytes, 272), 1);
}

#[test]
fn every_reserved_word_is_left_at_zero() {
    let bytes = built(&program(AaLevel::Off));

    for offset in [24, 28, 200, 208, 236, 240, 248, 252, 256, 260, 280, 284] {
        assert_eq!(u32_at(&bytes, offset), 0, "offset {offset}");
    }
    assert_eq!(u16_at(&bytes, 169), 0, "the pad inside the flag word");
}

#[test]
fn a_mirrored_plate_states_the_projection_its_pixels_were_made_under() {
    let mut mirrored = program(AaLevel::Off);
    let profile = SliceProfile::new(
        BuildVolume::new(153.36, 77.76, 165.0).unwrap(),
        PixelResolution::new(8520, 4320).unwrap(),
        LayerSchedule::uniform(0.05, 0.05, 2).unwrap(),
        Mirror { x: true, y: false },
        AaLevel::Off,
        PackingMode::None,
    );
    mirrored.raster = RasterSetup::from_profile(&profile);

    assert_eq!(u32_at(&built(&program(AaLevel::Off)), 80), 0);
    assert_eq!(u32_at(&built(&mirrored), 80), 1);
}
