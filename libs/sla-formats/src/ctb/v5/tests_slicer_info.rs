/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::layout::{Layout, SLICER_INFO_SIZE, VariableSections};
use super::slicer_info::write_slicer_info;
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

/// A whole number of minutes, so the conversion is visible without a
/// remainder confusing it.
const CREATED_SEC: u64 = 1_788_721_620;
const CREATED_MIN: u32 = 29_812_027;

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
        slab_top_mm: mm(0.05),
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
        light_pwm: LightPwm::FULL,
    }
}

fn program(aa: AaLevel) -> PrintProgram {
    PrintProgram {
        raster: raster(aa),
        machine_name: NonEmptyString::new("ELEGOO Jupiter SE").unwrap(),
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
        machine_name_bytes: 18,
        resin_parameters_bytes: 64,
        layer_count: 7,
    })
}

fn written(program: &PrintProgram) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_slicer_info(program, &layout(), &mut bytes).unwrap();
    bytes
}

fn u8_at(bytes: &[u8], offset: usize) -> u8 {
    bytes[offset]
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn the_block_is_the_size_the_layout_reserves_for_it() {
    assert_eq!(
        written(&program(AaLevel::Off)).len(),
        SLICER_INFO_SIZE as usize
    );
}

#[test]
fn the_second_lift_stage_comes_from_the_first_and_last_layers() {
    let bytes = written(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 0), 1.75);
    assert_eq!(f32_at(&bytes, 4), 219.0);
    assert_eq!(f32_at(&bytes, 8), 2.75);
    assert_eq!(f32_at(&bytes, 12), 243.0);
}

#[test]
fn the_second_retract_stage_is_a_normal_layers() {
    let bytes = written(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 16), 3.25);
    assert_eq!(f32_at(&bytes, 20), 189.0);
}

#[test]
fn a_single_stage_machine_leaves_every_second_stage_slot_at_zero() {
    let mut program = program(AaLevel::Off);
    for layer in &mut program.layers {
        layer.second_lift = None;
        layer.second_retract = None;
    }
    let bytes = written(&program);

    assert!(
        bytes[0..24].iter().all(|byte| *byte == 0),
        "{:?}",
        &bytes[0..24],
    );
}

#[test]
fn the_machine_name_is_located_by_the_layout_that_placed_it() {
    let bytes = written(&program(AaLevel::Off));
    let layout = layout();

    assert_eq!(u32_at(&bytes, 28), layout.machine_name);
    assert_eq!(u32_at(&bytes, 32), layout.variable.machine_name_bytes);
}

#[test]
fn the_anti_aliasing_flag_and_level_agree_with_each_other() {
    let off = written(&program(AaLevel::Off));
    let on = written(&program(AaLevel::X4));

    assert_eq!(u8_at(&off, 36), 0x07);
    assert_eq!(u32_at(&off, 44), 1);
    assert_eq!(u8_at(&on, 36), 0x0F);
    assert_eq!(u32_at(&on, 44), 4);
}

#[test]
fn every_layer_is_allowed_to_override_the_header() {
    let bytes = written(&program(AaLevel::Off));

    assert_eq!(u8_at(&bytes, 37), 0);
    assert_eq!(u8_at(&bytes, 38), 0);
    assert_eq!(u8_at(&bytes, 39), 0x50);
}

#[test]
fn the_timestamp_is_counted_in_minutes_here() {
    assert_eq!(u32_at(&written(&program(AaLevel::Off)), 40), CREATED_MIN);
}

#[test]
fn the_software_version_is_the_one_this_revision_reports() {
    assert_eq!(u32_at(&written(&program(AaLevel::Off)), 48), 0x0200_0000);
}

#[test]
fn the_waits_land_in_the_slots_that_name_their_motion() {
    let bytes = written(&program(AaLevel::Off));

    assert_eq!(f32_at(&bytes, 24), 1.1, "rest at the top of the lift");
    assert_eq!(f32_at(&bytes, 52), 0.3, "rest once the platform is back");
    assert_eq!(f32_at(&bytes, 56), 1.1, "the same rest as offset 24");
}

#[test]
fn the_transition_count_and_the_v4_address_close_the_block() {
    let bytes = written(&program(AaLevel::Off));

    assert_eq!(u32_at(&bytes, 60), 5);
    assert_eq!(u32_at(&bytes, 64), layout().print_parameters_v4);
    assert!(bytes[68..].iter().all(|byte| *byte == 0));
}

#[test]
fn an_empty_program_still_writes_a_whole_block() {
    let mut empty = program(AaLevel::Off);
    empty.layers.clear();
    let bytes = written(&empty);

    assert_eq!(bytes.len(), SLICER_INFO_SIZE as usize);
    assert_eq!(u32_at(&bytes, 64), layout().print_parameters_v4);
}
