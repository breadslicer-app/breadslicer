/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::layout::{DISCLAIMER_SIZE, Layout, PRINT_PARAMETERS_V4_SIZE, VariableSections};
use super::print_parameters_v4::write_print_parameters_v4;
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
/// tell which layer and which stage a byte came from. The waits are
/// whole halves, so their sum is exact in an f32.
fn layer(bottom: bool) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(0.05),
        exposure_sec: PositiveFiniteSec::new(if bottom { 31.0 } else { 2.5 }).unwrap(),
        wait_before_cure_sec: NonNegativeFiniteSec::new(0.5).unwrap(),
        wait_after_cure_sec: NonNegativeFiniteSec::new(0.25).unwrap(),
        wait_after_lift_sec: NonNegativeFiniteSec::new(1.25).unwrap(),
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

fn layout() -> Layout {
    Layout::plan(VariableSections {
        large_preview_bytes: 5_000,
        small_preview_bytes: 900,
        machine_name_bytes: 18,
        resin_parameters_bytes: 64,
        layer_count: 2,
    })
}

fn written(program: &PrintProgram) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_print_parameters_v4(program, &layout(), &mut bytes).unwrap();
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
    assert_eq!(
        written(&two_stage()).len(),
        PRINT_PARAMETERS_V4_SIZE as usize,
    );
}

#[test]
fn the_bottom_retract_is_the_one_the_earlier_blocks_leave_out() {
    let bytes = written(&two_stage());

    assert_eq!(f32_at(&bytes, 0), 151.0);
    assert_eq!(f32_at(&bytes, 4), 171.0);
    assert_eq!(f32_at(&bytes, 36), 3.75);
}

#[test]
fn the_two_unidentified_slots_carry_what_the_vendor_slicer_writes() {
    let bytes = written(&two_stage());

    assert_eq!(u32_at(&bytes, 8), 0);
    assert_eq!(f32_at(&bytes, 12), 4.0);
    assert_eq!(u32_at(&bytes, 16), 0);
    assert_eq!(f32_at(&bytes, 20), 4.0);
}

#[test]
fn all_three_waits_are_repeated_here() {
    let bytes = written(&two_stage());

    assert_eq!(f32_at(&bytes, 24), 0.5, "once the platform is back");
    assert_eq!(f32_at(&bytes, 28), 1.25, "at the top of the lift");
    assert_eq!(f32_at(&bytes, 32), 0.25, "once the light goes off");
}

#[test]
fn the_still_time_totals_the_exposures_and_the_waits() {
    let bytes = written(&two_stage());

    assert_eq!(f32_at(&bytes, 40), 31.0 + 2.5 + 2.0 * 2.0);
}

#[test]
fn the_still_time_grows_with_the_layer_count() {
    let four = program(vec![layer(true), layer(false), layer(false), layer(false)]);
    let bytes = written(&four);

    assert_eq!(f32_at(&bytes, 40), 31.0 + 3.0 * 2.5 + 4.0 * 2.0);
}

#[test]
fn the_layer_count_closes_the_motion_words() {
    let bytes = written(&two_stage());

    assert_eq!(u32_at(&bytes, 44), 0);
    assert_eq!(u32_at(&bytes, 48), 0);
    assert_eq!(u32_at(&bytes, 52), 2);
    assert!(bytes[56..72].iter().all(|byte| *byte == 0));
}

#[test]
fn the_sections_this_block_locates_are_the_ones_the_layout_placed() {
    let bytes = written(&two_stage());
    let layout = layout();

    assert_eq!(u32_at(&bytes, 72), layout.disclaimer);
    assert_eq!(u32_at(&bytes, 76), DISCLAIMER_SIZE);
    assert_eq!(u32_at(&bytes, 80), layout.resin_parameters);
}

#[test]
fn the_block_ends_in_reserved_bytes() {
    assert!(written(&two_stage())[84..].iter().all(|byte| *byte == 0));
}

#[test]
fn an_empty_program_still_writes_a_whole_block() {
    let bytes = written(&program(Vec::new()));

    assert_eq!(bytes.len(), PRINT_PARAMETERS_V4_SIZE as usize);
    assert_eq!(f32_at(&bytes, 40), 0.0);
    assert_eq!(u32_at(&bytes, 52), 0);
    assert_eq!(u32_at(&bytes, 72), layout().disclaimer);
}
