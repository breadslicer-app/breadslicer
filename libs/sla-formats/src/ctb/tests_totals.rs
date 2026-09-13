/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::totals::{overall_height_mm, still_time_sec};
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn sec(value: f64) -> NonNegativeFiniteSec {
    NonNegativeFiniteSec::new(value).unwrap()
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

/// A layer whose exposure and 3 waits are all distinct, so a total
/// that drops one of them is visible.
fn layer(slab_top_mm: f64, exposure_sec: f64) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(slab_top_mm),
        exposure_sec: PositiveFiniteSec::new(exposure_sec).unwrap(),
        wait_before_cure_sec: sec(0.5),
        wait_after_cure_sec: sec(0.25),
        wait_after_lift_sec: sec(0.125),
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

fn program(layers: Vec<LayerProgram>) -> PrintProgram {
    PrintProgram {
        raster: raster(),
        machine_name: NonEmptyString::new("ELEGOO Mars 5 Ultra").unwrap(),
        material_name: NonEmptyString::new("Magna Durable").unwrap(),
        layers,
        layer_height_mm: mm(0.05),
        bottom_layer_count: 1,
        transition_layer_count: 0,
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

#[test]
fn the_overall_height_is_where_the_last_layer_ends() {
    let program = program(vec![layer(0.05, 32.0), layer(0.10, 2.5), layer(0.15, 2.5)]);

    assert_eq!(overall_height_mm(&program), 0.15);
}

#[test]
fn a_program_with_no_layers_reaches_no_height_and_spends_no_time() {
    let program = program(Vec::new());

    assert_eq!(overall_height_mm(&program), 0.0);
    assert_eq!(still_time_sec(&program), 0.0);
}

#[test]
fn the_still_time_counts_every_exposure_and_every_wait() {
    let program = program(vec![layer(0.05, 32.0), layer(0.10, 2.5)]);

    assert_eq!(still_time_sec(&program), 32.0 + 2.5 + 2.0 * 0.875);
}

#[test]
fn the_still_time_leaves_the_travel_out() {
    let single = program(vec![layer(0.05, 2.5)]);
    let mut faster = single.clone();
    faster.layers[0].lift.speed_mm_min = PositiveFiniteMmPerMin::new(1.0).unwrap();

    assert_eq!(still_time_sec(&single), still_time_sec(&faster));
}
