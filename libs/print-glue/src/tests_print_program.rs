/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::num::NonZeroU32;

use sc_print_primitives::{LightPwm, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm};
use sc_print_profiles::{
    EffectiveMaterial, MaterialProfile, PrinterProfile, ResolvedPrintConfig, SceneOverrides,
};
use sc_sl1cer::profile::{AaLevel, LayerSchedule, PackingMode, SliceProfile};
use sc_sla_formats::PrintProgram;

use super::print_program::{SlabTopOutOfRange, project_to_print_program};
use super::projection::project_to_slice_profile;

const BOTTOM_LAYER_COUNT: u32 = 8;
const TRANSITION_COUNT: u32 = 5;
const BOTTOM_EXPOSURE_SEC: f64 = 30.0;
const NORMAL_EXPOSURE_SEC: f64 = 2.5;
const LAYER_HEIGHT_MM: f64 = 0.05;

const PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-mars-5-goo"
manufacturer = "ELEGOO"
family = "Mars"
name = "Mars 5"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 143.43, y_mm = 89.6, z_mm = 150.0 }
pixel_resolution = { x = 4098, y = 2560 }
pixel_size_microns = { x = 35.0, y = 35.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "goo", version = "v3.0" }]
supports_grayscale = true
two_stage_lift = false
transition_layers = 5
has_camera = false

[constraints]
z_step_mm = 0.01
light_intensity_mw_per_cm2 = 5.0
"#;

const PRINTER_WITHOUT_TRANSITION_LAYERS_TOML: &str = r#"
[identity]
id = "elegoo-mars-5-goo"
manufacturer = "ELEGOO"
family = "Mars"
name = "Mars 5"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 143.43, y_mm = 89.6, z_mm = 150.0 }
pixel_resolution = { x = 4098, y = 2560 }
pixel_size_microns = { x = 35.0, y = 35.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "goo", version = "v3.0" }]
supports_grayscale = true
two_stage_lift = false
has_camera = false

[constraints]
z_step_mm = 0.01
light_intensity_mw_per_cm2 = 5.0
"#;

const MATERIAL_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }

[per_printer."elegoo-mars-5-goo"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

/// The same resin, with every wait calibrated for the same printer.
const MATERIAL_WITH_WAITS_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }

[per_printer."elegoo-mars-5-goo"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
wait_before_cure_sec = 0.4
wait_after_cure_sec = 1.2
wait_after_lift_sec = 2.5
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

fn resolved_config(printer_toml: &str) -> ResolvedPrintConfig {
    resolved_config_for(printer_toml, MATERIAL_TOML)
}

fn resolved_config_for(printer_toml: &str, material_toml: &str) -> ResolvedPrintConfig {
    let printer: PrinterProfile = toml_edit::de::from_str(printer_toml).unwrap();
    let material: MaterialProfile = toml_edit::de::from_str(material_toml).unwrap();
    let material = EffectiveMaterial::from(material);
    sc_print_profiles::resolve(&printer, &material, &SceneOverrides::default()).unwrap()
}

fn slice_profile(config: &ResolvedPrintConfig, total_layers: u32) -> SliceProfile {
    project_to_slice_profile(
        config,
        AaLevel::Off,
        PackingMode::None,
        PositiveFiniteMm::new(LAYER_HEIGHT_MM).unwrap(),
        NonZeroU32::new(total_layers).expect("test counts are non-zero"),
    )
}

const CREATED_AT: &str = "2026-09-04 17:31:02";

fn created_at() -> NonEmptyString {
    NonEmptyString::new(CREATED_AT).unwrap()
}

fn program(printer_toml: &str, total_layers: u32) -> PrintProgram {
    let config = resolved_config(printer_toml);
    let profile = slice_profile(&config, total_layers);
    project_to_print_program(&config, &profile, created_at(), "0.0.12").unwrap()
}

fn exposures(program: &PrintProgram) -> Vec<f64> {
    program
        .layers
        .iter()
        .map(|layer| layer.exposure_sec.value())
        .collect()
}

#[test]
fn one_record_lands_per_scheduled_layer() {
    let program = program(PRINTER_TOML, 20);
    assert_eq!(program.layers.len(), 20);
}

#[test]
fn the_bottom_layers_carry_the_bottom_exposure() {
    let exposures = exposures(&program(PRINTER_TOML, 20));
    for (index, exposure) in exposures
        .iter()
        .enumerate()
        .take(BOTTOM_LAYER_COUNT as usize)
    {
        assert_eq!(*exposure, BOTTOM_EXPOSURE_SEC, "layer {index}");
    }
}

#[test]
fn the_transition_band_steps_towards_the_normal_exposure() {
    let exposures = exposures(&program(PRINTER_TOML, 20));
    let span = BOTTOM_EXPOSURE_SEC - NORMAL_EXPOSURE_SEC;
    let step = span / f64::from(TRANSITION_COUNT + 1);
    for offset in 0..TRANSITION_COUNT {
        let index = (BOTTOM_LAYER_COUNT + offset) as usize;
        let expected = f64::from(offset + 1).mul_add(-step, BOTTOM_EXPOSURE_SEC);
        assert!(
            (exposures[index] - expected).abs() < 1e-12,
            "layer {index}: {} against {expected}",
            exposures[index],
        );
    }
}

#[test]
fn the_transition_band_stays_above_the_normal_exposure() {
    let exposures = exposures(&program(PRINTER_TOML, 20));
    for offset in 0..TRANSITION_COUNT {
        let index = (BOTTOM_LAYER_COUNT + offset) as usize;
        assert!(
            exposures[index] > NORMAL_EXPOSURE_SEC,
            "layer {index}: {}",
            exposures[index],
        );
    }
}

#[test]
fn the_layer_after_the_band_is_the_first_at_the_normal_exposure() {
    let exposures = exposures(&program(PRINTER_TOML, 20));
    let first_normal = (BOTTOM_LAYER_COUNT + TRANSITION_COUNT) as usize;
    assert_ne!(exposures[first_normal - 1], NORMAL_EXPOSURE_SEC);
    for (index, exposure) in exposures.iter().enumerate().skip(first_normal) {
        assert_eq!(*exposure, NORMAL_EXPOSURE_SEC, "layer {index}");
    }
}

#[test]
fn without_transition_layers_the_normal_exposure_follows_the_bottom_layers() {
    let exposures = exposures(&program(PRINTER_WITHOUT_TRANSITION_LAYERS_TOML, 20));
    assert_eq!(
        exposures[(BOTTOM_LAYER_COUNT - 1) as usize],
        BOTTOM_EXPOSURE_SEC,
    );
    assert_eq!(exposures[BOTTOM_LAYER_COUNT as usize], NORMAL_EXPOSURE_SEC);
}

#[test]
fn a_schedule_shorter_than_the_bottom_layers_is_all_bottom_layers() {
    let exposures = exposures(&program(PRINTER_TOML, 3));
    assert_eq!(exposures, vec![BOTTOM_EXPOSURE_SEC; 3]);
}

#[test]
fn every_layer_records_its_slab_top() {
    let config = resolved_config(PRINTER_TOML);
    let profile = slice_profile(&config, 20);
    let program = project_to_print_program(&config, &profile, created_at(), "0.0.12").unwrap();

    let scheduled: Vec<f64> = profile.layer_schedule().iter_slab_tops().collect();
    let recorded: Vec<f64> = program
        .layers
        .iter()
        .map(|layer| layer.slab_top_mm.value())
        .collect();
    assert_eq!(recorded, scheduled);
}

#[test]
fn the_raster_setup_comes_from_the_slice_profile() {
    let config = resolved_config(PRINTER_TOML);
    let profile = slice_profile(&config, 20);
    let program = project_to_print_program(&config, &profile, created_at(), "0.0.12").unwrap();

    assert_eq!(program.raster.build_volume(), profile.build_volume());
    assert_eq!(
        program.raster.pixel_resolution(),
        profile.pixel_resolution()
    );
    assert_eq!(program.raster.mirror(), profile.mirror());
    assert_eq!(program.raster.aa(), profile.aa_level());
}

#[test]
fn the_machine_and_material_names_come_from_the_profiles() {
    let program = program(PRINTER_TOML, 20);
    assert_eq!(program.machine_name.as_str(), "Mars 5");
    assert_eq!(program.material_name.as_str(), "Blu");
    assert_eq!(program.slicer_version, "0.0.12");
}

#[test]
fn the_creation_time_is_the_callers() {
    assert_eq!(program(PRINTER_TOML, 20).created_at.as_str(), CREATED_AT);
}

#[test]
fn the_layer_height_is_the_resolved_one() {
    let program = program(PRINTER_TOML, 20);
    assert_eq!(program.layer_height_mm.value(), LAYER_HEIGHT_MM);
}

#[test]
fn the_bottom_and_transition_counts_come_from_the_profiles() {
    let program = program(PRINTER_TOML, 20);
    assert_eq!(program.bottom_layer_count, BOTTOM_LAYER_COUNT);
    assert_eq!(program.transition_layer_count, TRANSITION_COUNT);
}

#[test]
fn a_printer_declaring_no_transition_layers_carries_a_count_of_zero() {
    let program = program(PRINTER_WITHOUT_TRANSITION_LAYERS_TOML, 20);
    assert_eq!(program.transition_layer_count, 0);
}

#[test]
fn the_counts_describe_the_exposures_the_records_carry() {
    let program = program(PRINTER_TOML, 20);
    let bottom = program.bottom_layer_count as usize;
    let band_end = bottom + program.transition_layer_count as usize;

    assert_eq!(
        program.layers[bottom - 1].exposure_sec,
        program.layers[0].exposure_sec,
        "the layer before the count is still a bottom layer",
    );
    assert_ne!(
        program.layers[bottom].exposure_sec, program.layers[0].exposure_sec,
        "the layer at the count has left the bottom exposure",
    );
    assert_eq!(
        program.layers[band_end].exposure_sec,
        program.layers[program.layers.len() - 1].exposure_sec,
        "the layer after the band already holds the normal exposure",
    );
}

#[test]
fn the_retract_repeats_the_lift_distance_at_its_own_speed() {
    let program = program(PRINTER_TOML, 20);
    for layer in &program.layers {
        assert_eq!(layer.lift.distance_mm.value(), 5.0);
        assert_eq!(layer.lift.speed_mm_min.value(), 120.0);
        assert_eq!(layer.retract.distance_mm.value(), 5.0);
        assert_eq!(layer.retract.speed_mm_min.value(), 150.0);
    }
}

#[test]
fn a_resin_naming_no_pause_leaves_every_record_at_zero() {
    let program = program(PRINTER_TOML, 20);
    for layer in &program.layers {
        assert_eq!(layer.wait_before_cure_sec, NonNegativeFiniteSec::ZERO);
        assert_eq!(layer.wait_after_cure_sec, NonNegativeFiniteSec::ZERO);
        assert_eq!(layer.wait_after_lift_sec, NonNegativeFiniteSec::ZERO);
    }
}

#[test]
fn the_calibrated_waits_reach_every_record() {
    let config = resolved_config_for(PRINTER_TOML, MATERIAL_WITH_WAITS_TOML);
    let profile = slice_profile(&config, 20);
    let program = project_to_print_program(&config, &profile, created_at(), "0.0.12").unwrap();

    for layer in &program.layers {
        assert_eq!(layer.wait_before_cure_sec.value(), 0.4);
        assert_eq!(layer.wait_after_cure_sec.value(), 1.2);
        assert_eq!(layer.wait_after_lift_sec.value(), 2.5);
    }
}

#[test]
fn the_light_runs_at_full_power() {
    let program = program(PRINTER_TOML, 20);
    for layer in &program.layers {
        assert_eq!(layer.light_pwm, LightPwm::FULL);
    }
}

#[test]
fn no_layer_carries_a_second_motion_stage() {
    let program = program(PRINTER_TOML, 20);
    for layer in &program.layers {
        assert_eq!(layer.second_lift, None);
        assert_eq!(layer.second_retract, None);
    }
}

#[test]
fn a_layer_beyond_the_representable_height_names_itself() {
    let config = resolved_config(PRINTER_TOML);
    let profile = SliceProfile::new(
        config.printer.hardware.build_volume(),
        config.printer.hardware.pixel_resolution(),
        LayerSchedule::uniform(f64::MAX, f64::MAX, 3).unwrap(),
        config.printer.hardware.mirror(),
        AaLevel::Off,
        PackingMode::None,
    );

    let err = project_to_print_program(&config, &profile, created_at(), "0.0.12").unwrap_err();
    assert_eq!(err.layer_index, 1);
    assert!(!err.z_mm.is_finite(), "z_mm: {}", err.z_mm);
}

#[test]
fn slab_top_out_of_range_display_names_the_layer_and_the_height() {
    let err = SlabTopOutOfRange {
        layer_index: 7,
        z_mm: f64::INFINITY,
    };
    let msg = err.to_string();
    assert!(msg.contains("layer 7"), "msg: {msg}");
    assert!(msg.contains("inf"), "msg: {msg}");
}
