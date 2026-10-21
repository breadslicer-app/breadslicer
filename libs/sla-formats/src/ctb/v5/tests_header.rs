/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::header::write_header;
use super::layout::{
    HEADER_SIZE, Layout, PRINT_PARAMETERS_SIZE, SLICER_INFO_SIZE, VariableSections,
};
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

const CREATED_SEC: u64 = 1_788_721_620;

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn raster(mirror: Mirror) -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(153.36, 77.76, 165.0).unwrap(),
        PixelResolution::new(8520, 4320).unwrap(),
        LayerSchedule::uniform(0.05, 0.05, 2).unwrap(),
        mirror,
        AaLevel::Off,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

/// A layer whose every slot holds a distinct value, so a test can
/// tell which slot a byte came from.
fn layer(slab_top_mm: f64, exposure_sec: f64, pwm_pct: f64) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(slab_top_mm),
        exposure_sec: PositiveFiniteSec::new(exposure_sec).unwrap(),
        wait_before_cure_sec: NonNegativeFiniteSec::new(0.3).unwrap(),
        wait_after_cure_sec: NonNegativeFiniteSec::new(0.7).unwrap(),
        wait_after_lift_sec: NonNegativeFiniteSec::new(1.1).unwrap(),
        lift: MotionStage {
            distance_mm: mm(5.5),
            speed_mm_min: PositiveFiniteMmPerMin::new(91.0).unwrap(),
        },
        retract: MotionStage {
            distance_mm: mm(6.5),
            speed_mm_min: PositiveFiniteMmPerMin::new(137.0).unwrap(),
        },
        second_lift: None,
        second_retract: None,
        light_pwm: LightPwm::new(pwm_pct).unwrap(),
    }
}

fn program(layer_count: usize) -> PrintProgram {
    PrintProgram {
        raster: raster(Mirror { x: true, y: false }),
        machine_name: NonEmptyString::new("ELEGOO Jupiter SE").unwrap(),
        material_name: NonEmptyString::new("Magna Durable").unwrap(),
        layers: (1..=layer_count)
            .map(|index| {
                let bottom = index == 1;
                layer(
                    0.05 * index as f64,
                    if bottom { 31.0 } else { 2.7 },
                    if bottom { 100.0 } else { 80.0 },
                )
            })
            .collect(),
        layer_height_mm: mm(0.05),
        bottom_layer_count: 3,
        transition_layer_count: 5,
        volume_mm3: VolumeMm3::new(15_553.1).unwrap(),
        mass_g: MassG::new(17.1085).unwrap(),
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
    write_header(program, &layout(), &mut bytes).unwrap();
    bytes
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn the_header_is_the_size_the_layout_reserves_for_it() {
    assert_eq!(written(&program(7)).len(), HEADER_SIZE as usize);
}

#[test]
fn the_magic_and_the_version_open_the_file() {
    let bytes = written(&program(7));

    assert_eq!(u32_at(&bytes, 0), 0x12FD_0106);
    assert_eq!(u32_at(&bytes, 4), 5);
}

#[test]
fn the_plate_envelope_follows_the_raster_setup() {
    let bytes = written(&program(7));

    assert_eq!(f32_at(&bytes, 8), 153.36);
    assert_eq!(f32_at(&bytes, 12), 77.76);
    assert_eq!(f32_at(&bytes, 16), 165.0);
    assert_eq!(u32_at(&bytes, 52), 8520);
    assert_eq!(u32_at(&bytes, 56), 4320);
}

#[test]
fn both_timestamps_hold_the_one_moment_the_program_records() {
    let bytes = written(&program(7));

    assert_eq!(u32_at(&bytes, 20), CREATED_SEC as u32);
    assert_eq!(u32_at(&bytes, 24), CREATED_SEC as u32);
}

#[test]
fn the_print_reaches_the_top_of_its_last_layer() {
    let bytes = written(&program(7));

    assert_eq!(f32_at(&bytes, 28), 0.35);
    assert_eq!(f32_at(&bytes, 32), 0.05);
}

#[test]
fn the_exposures_come_from_the_first_and_last_layers() {
    let bytes = written(&program(7));

    assert_eq!(f32_at(&bytes, 36), 2.7);
    assert_eq!(f32_at(&bytes, 40), 31.0);
    assert_eq!(u32_at(&bytes, 48), 3);
}

#[test]
fn the_light_off_delay_is_zero_because_the_waits_carry_the_pause() {
    assert_eq!(f32_at(&written(&program(7)), 44), 0.0);
}

#[test]
fn every_recorded_offset_is_the_one_the_layout_measured() {
    let bytes = written(&program(7));
    let layout = layout();

    assert_eq!(u32_at(&bytes, 60), layout.large_preview_record);
    assert_eq!(u32_at(&bytes, 64), layout.layer_table);
    assert_eq!(u32_at(&bytes, 72), layout.small_preview_record);
    assert_eq!(u32_at(&bytes, 84), layout.print_parameters);
    assert_eq!(u32_at(&bytes, 104), layout.slicer_info);
}

#[test]
fn each_recorded_offset_is_followed_by_the_size_of_what_it_points_at() {
    let bytes = written(&program(7));

    assert_eq!(u32_at(&bytes, 88), PRINT_PARAMETERS_SIZE);
    assert_eq!(u32_at(&bytes, 108), SLICER_INFO_SIZE);
}

#[test]
fn the_layer_count_is_the_programs_own() {
    assert_eq!(u32_at(&written(&program(7)), 68), 7);
}

#[test]
fn the_print_time_is_the_estimate_the_program_carries() {
    assert_eq!(u32_at(&written(&program(7)), 76), 5664);
}

#[test]
fn the_projector_is_mirrored_when_the_projection_flips_x() {
    let mut flat = program(7);
    flat.raster = raster(Mirror { x: false, y: false });

    assert_eq!(u32_at(&written(&program(7)), 80), 1);
    assert_eq!(u32_at(&written(&flat), 80), 0);
}

#[test]
fn one_level_set_because_grey_is_stored_per_pixel() {
    assert_eq!(u32_at(&written(&program(7)), 92), 1);
}

#[test]
fn the_duty_cycles_follow_the_last_and_first_layers() {
    let bytes = written(&program(7));

    assert_eq!(u16_at(&bytes, 96), 204);
    assert_eq!(u16_at(&bytes, 98), 255);
}

#[test]
fn the_layer_images_are_left_unobfuscated() {
    assert_eq!(u32_at(&written(&program(7)), 100), 0);
}

#[test]
fn an_empty_program_still_writes_a_whole_header() {
    let bytes = written(&program(0));

    assert_eq!(bytes.len(), HEADER_SIZE as usize);
    assert_eq!(u32_at(&bytes, 68), 0);
    assert_eq!(f32_at(&bytes, 28), 0.0);
    assert_eq!(f32_at(&bytes, 36), 0.0);
}
