/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::layout::PRINT_PARAMETERS_SIZE;
use super::print_parameters::write_print_parameters;
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

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

fn program(layers: Vec<LayerProgram>) -> PrintProgram {
    PrintProgram {
        raster: raster(),
        machine_name: NonEmptyString::new("ELEGOO Jupiter SE").unwrap(),
        material_name: NonEmptyString::new("Magna Durable").unwrap(),
        layers,
        layer_height_mm: mm(0.05),
        bottom_layer_count: 3,
        transition_layer_count: 5,
        volume_mm3: VolumeMm3::new(15_000.0).unwrap(),
        mass_g: MassG::new(17.5).unwrap(),
        print_time_sec: NonNegativeFiniteSec::new(5664.0).unwrap(),
        created_at: CreationTime {
            rendered: NonEmptyString::new("2026-09-04 21:07:00").unwrap(),
            unix_sec: 1_788_721_620,
        },
        slicer_version: "0.0.12",
    }
}

fn two_stage() -> PrintProgram {
    program(vec![layer(true), layer(false)])
}

fn written(program: &PrintProgram) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_print_parameters(program, &mut bytes).unwrap();
    bytes
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn the_block_is_the_size_the_layout_reserves_for_it() {
    assert_eq!(written(&two_stage()).len(), PRINT_PARAMETERS_SIZE as usize);
}

#[test]
fn each_lift_height_is_both_stages_added_together() {
    let bytes = written(&two_stage());

    assert_eq!(f32_at(&bytes, 0), 9.0);
    assert_eq!(f32_at(&bytes, 8), 8.25);
}

#[test]
fn a_single_stage_machine_writes_its_only_stage_as_the_travel() {
    let mut bottom = layer(true);
    let mut normal = layer(false);
    bottom.second_lift = None;
    normal.second_lift = None;
    let bytes = written(&program(vec![bottom, normal]));

    assert_eq!(f32_at(&bytes, 0), 7.25);
    assert_eq!(f32_at(&bytes, 8), 5.5);
}

#[test]
fn the_speeds_come_from_the_first_and_last_layers() {
    let bytes = written(&two_stage());

    assert_eq!(f32_at(&bytes, 4), 63.0);
    assert_eq!(f32_at(&bytes, 12), 91.0);
    assert_eq!(f32_at(&bytes, 16), 137.0);
}

#[test]
fn the_material_figures_are_the_ones_the_machine_displays() {
    let bytes = written(&two_stage());

    assert_eq!(f32_at(&bytes, 20), 15.0, "cubic millimetres become ml");
    assert_eq!(f32_at(&bytes, 24), 17.5);
    assert_eq!(f32_at(&bytes, 28), 0.0, "nothing here prices a print");
}

#[test]
fn both_light_off_delays_are_zero_because_the_waits_carry_the_pause() {
    let bytes = written(&two_stage());

    assert_eq!(f32_at(&bytes, 32), 0.0);
    assert_eq!(f32_at(&bytes, 36), 0.0);
}

#[test]
fn the_bottom_layer_count_is_the_programs_own() {
    assert_eq!(u32_at(&written(&two_stage()), 40), 3);
}

#[test]
fn the_block_ends_in_padding() {
    assert!(written(&two_stage())[44..].iter().all(|byte| *byte == 0));
}

#[test]
fn an_empty_program_writes_a_whole_block_of_zeros() {
    let bytes = written(&program(Vec::new()));

    assert_eq!(bytes.len(), PRINT_PARAMETERS_SIZE as usize);
    assert_eq!(f32_at(&bytes, 0), 0.0);
    assert_eq!(f32_at(&bytes, 8), 0.0);
    assert_eq!(f32_at(&bytes, 16), 0.0);
}
