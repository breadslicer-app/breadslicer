/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::layout::{Layout, VariableSections};
use super::resin_parameters::{byte_length, write_resin_parameters};
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

const MATERIAL: &str = "Magna Durable";
const MACHINE: &str = "ELEGOO Jupiter SE";
const FIXED_SIZE: usize = 40;

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn raster() -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(153.36, 77.76, 165.0).unwrap(),
        PixelResolution::new(8520, 4320).unwrap(),
        LayerSchedule::uniform(0.05, 0.05, 2).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

fn layer() -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(0.05),
        exposure_sec: PositiveFiniteSec::new(2.5).unwrap(),
        wait_before_cure_sec: NonNegativeFiniteSec::ZERO,
        wait_after_cure_sec: NonNegativeFiniteSec::ZERO,
        wait_after_lift_sec: NonNegativeFiniteSec::ZERO,
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
        light_pwm: LightPwm::FULL,
    }
}

/// 20 ml of material weighing 22 g, so the density is exactly 1.1.
fn program() -> PrintProgram {
    PrintProgram {
        raster: raster(),
        machine_name: NonEmptyString::new(MACHINE).unwrap(),
        material_name: NonEmptyString::new(MATERIAL).unwrap(),
        layers: vec![layer()],
        layer_height_mm: mm(0.05),
        bottom_layer_count: 3,
        transition_layer_count: 5,
        volume_mm3: VolumeMm3::new(20_000.0).unwrap(),
        mass_g: MassG::new(22.0).unwrap(),
        print_time_sec: NonNegativeFiniteSec::new(5664.0).unwrap(),
        created_at: CreationTime {
            rendered: NonEmptyString::new("2026-09-04 21:07:00").unwrap(),
            unix_sec: 1_788_721_620,
        },
        slicer_version: "0.0.12",
    }
}

fn layout(program: &PrintProgram) -> Layout {
    Layout::plan(VariableSections {
        large_preview_bytes: 5_000,
        small_preview_bytes: 900,
        machine_name_bytes: u32::try_from(MACHINE.len()).unwrap(),
        resin_parameters_bytes: byte_length(program),
        layer_count: 1,
    })
}

fn written(program: &PrintProgram) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_resin_parameters(program, &layout(program), &mut bytes).unwrap();
    bytes
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn the_block_is_as_long_as_it_reported_it_would_be() {
    let program = program();

    assert_eq!(written(&program).len(), byte_length(&program) as usize);
}

#[test]
fn the_length_is_the_fixed_part_plus_the_three_strings() {
    assert_eq!(
        byte_length(&program()) as usize,
        FIXED_SIZE + MATERIAL.len() * 2 + MACHINE.len(),
    );
}

#[test]
fn each_address_is_followed_by_its_own_length() {
    let program = program();
    let bytes = written(&program);
    let base = layout(&program).resin_parameters;

    assert_eq!(u32_at(&bytes, 8), base + FIXED_SIZE as u32);
    assert_eq!(u32_at(&bytes, 12), MATERIAL.len() as u32);
    assert_eq!(u32_at(&bytes, 16), base + FIXED_SIZE as u32 + 13);
    assert_eq!(u32_at(&bytes, 20), MATERIAL.len() as u32);
    assert_eq!(u32_at(&bytes, 24), base + FIXED_SIZE as u32 + 26);
    assert_eq!(u32_at(&bytes, 28), MACHINE.len() as u32);
}

#[test]
fn the_addresses_are_spaced_by_the_lengths_they_declare() {
    let program = program();
    let bytes = written(&program);

    assert_eq!(u32_at(&bytes, 16) - u32_at(&bytes, 8), u32_at(&bytes, 12));
    assert_eq!(u32_at(&bytes, 24) - u32_at(&bytes, 16), u32_at(&bytes, 20));
}

#[test]
fn the_strings_are_packed_with_no_terminator_between_them() {
    let program = program();
    let bytes = written(&program);
    let expected = format!("{MATERIAL}{MATERIAL}{MACHINE}");

    assert_eq!(&bytes[FIXED_SIZE..], expected.as_bytes());
}

#[test]
fn the_density_is_the_mass_the_enclosed_volume_carries() {
    assert_eq!(f32_at(&written(&program()), 32), 1.1);
}

#[test]
fn a_plate_that_encloses_nothing_reports_no_density() {
    let mut empty = program();
    empty.volume_mm3 = VolumeMm3::ZERO;

    assert_eq!(f32_at(&written(&empty), 32), 0.0);
}

#[test]
fn the_colour_and_both_pads_are_left_unset() {
    let bytes = written(&program());

    assert_eq!(u32_at(&bytes, 0), 0);
    assert_eq!(&bytes[4..8], &[0, 0, 0, 0]);
    assert_eq!(u32_at(&bytes, 36), 0);
}
