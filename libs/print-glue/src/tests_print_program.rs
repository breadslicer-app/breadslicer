/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::num::NonZeroU32;
use std::sync::Arc;

use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Normal};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_print_primitives::{LightPwm, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm};
use sc_print_profiles::{
    EffectiveMaterial, MaterialProfile, PrinterProfile, ResolvedPrintConfig, SceneOverrides,
};
use sc_scene::node::LeafPayload;
use sc_scene::{Scene, SceneError};
use sc_sl1cer::profile::{AaLevel, LayerSchedule, PackingMode, SliceProfile};
use sc_sla_formats::PrintProgram;

use super::print_program::{
    SlabTopOutOfRange, UnprojectablePrint, creation_time_now, project_to_print_program,
};
use super::projection::project_to_slice_profile;

const BOTTOM_LAYER_COUNT: u32 = 8;
const TRANSITION_COUNT: u32 = 5;
const BOTTOM_EXPOSURE_SEC: f64 = 30.0;
const NORMAL_EXPOSURE_SEC: f64 = 2.5;
const LAYER_HEIGHT_MM: f64 = 0.05;

const PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-mars-5"
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
two_stage_lift = true
transition_layers = 5
has_camera = false

[constraints]
z_step_mm = 0.01
light_intensity_mw_per_cm2 = 5.0
"#;

const PRINTER_WITHOUT_TRANSITION_LAYERS_TOML: &str = r#"
[identity]
id = "elegoo-mars-5"
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
two_stage_lift = true
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

[per_printer."elegoo-mars-5"]
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

[per_printer."elegoo-mars-5"]
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

/// The same resin again, with the bottom layers' waits calibrated
/// apart from the normal ones.
const MATERIAL_WITH_BOTTOM_WAITS_TOML: &str = r#"
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

[per_printer."elegoo-mars-5"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
wait_before_cure_sec = 0.4
wait_after_cure_sec = 1.2
wait_after_lift_sec = 2.5
bottom_wait_before_cure_sec = 6.0
bottom_wait_after_cure_sec = 7.0
bottom_wait_after_lift_sec = 8.0
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

/// The same resin again, with the bottom layers' travel calibrated
/// apart from the normal one.
const MATERIAL_WITH_BOTTOM_MOTION_TOML: &str = r#"
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

[per_printer."elegoo-mars-5"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
bottom_lift_distance_mm = 8.0
bottom_lift_speed_mm_min = 40.0
bottom_retract_speed_mm_min = 90.0
"#;

/// A resin whose travel splits in two, carrying the numbers ELEGOO's
/// own slicer wrote for the normal layers of a Mars 5 and Lychee
/// wrote for its bottom layers.
const MATERIAL_WITH_SPLIT_TRAVEL_TOML: &str = r#"
[identity]
id = "elegoo-standard-grey"
brand = "ELEGOO"
family = "standard"
name = "Standard Grey"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }

[per_printer."elegoo-mars-5"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 32.0
bottom_layer_count = 8
lift_distance_mm = 2.0
lift_speed_mm_min = 75.0
retract_speed_mm_min = 75.0
second_travel = { lift_distance_mm = 3.0, lift_speed_mm_min = 230.0, retract_speed_mm_min = 230.0 }
bottom_lift_distance_mm = 3.0
bottom_lift_speed_mm_min = 60.0
bottom_retract_speed_mm_min = 60.0
bottom_second_travel = { lift_distance_mm = 3.0, lift_speed_mm_min = 120.0, retract_speed_mm_min = 120.0 }
"#;

/// The same split travel with round numbers, so a 2 layer schedule
/// adds up by hand.
const MATERIAL_WITH_SPLIT_TRAVEL_TIMING_TOML: &str = r#"
[identity]
id = "elegoo-standard-grey"
brand = "ELEGOO"
family = "standard"
name = "Standard Grey"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }

[per_printer."elegoo-mars-5"]
normal_exposure_sec = 2.0
bottom_exposure_sec = 30.0
bottom_layer_count = 1
lift_distance_mm = 2.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 120.0
second_travel = { lift_distance_mm = 3.0, lift_speed_mm_min = 180.0, retract_speed_mm_min = 180.0 }
bottom_lift_distance_mm = 4.0
bottom_lift_speed_mm_min = 120.0
bottom_retract_speed_mm_min = 120.0
bottom_second_travel = { lift_distance_mm = 2.0, lift_speed_mm_min = 120.0, retract_speed_mm_min = 120.0 }
"#;

/// The same resin again, driving the light at two different duties.
const MATERIAL_WITH_LIGHT_PWM_TOML: &str = r#"
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

[per_printer."elegoo-mars-5"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
light_pwm = 80.0
bottom_light_pwm = 60.0
"#;

/// The same resin again, with a single bottom layer and every pause
/// and travel calibrated apart, so a 2 layer schedule adds up by
/// hand.
const MATERIAL_FOR_TIMING_TOML: &str = r#"
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

[per_printer."elegoo-mars-5"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 1
wait_before_cure_sec = 0.4
wait_after_cure_sec = 1.2
wait_after_lift_sec = 2.5
bottom_wait_before_cure_sec = 6.0
bottom_wait_after_cure_sec = 7.0
bottom_wait_after_lift_sec = 8.0
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
bottom_lift_distance_mm = 8.0
bottom_lift_speed_mm_min = 40.0
bottom_retract_speed_mm_min = 96.0
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

/// The 3 fixture solids together: a unit cube, plus 2 prisms of 8
/// mm3 each holding a 0.8 x 0.8 x 2 tunnel.
const SCENE_VOLUME_MM3: f64 = 1.0 + 2.0 * (8.0 - 1.28);

fn plate_of(meshes: impl IntoIterator<Item = IndexedMesh>) -> Scene {
    let mut scene = Scene::new();
    for mesh in meshes {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        scene.add_root(leaf).expect("leaf as root");
    }
    scene
}

fn scene() -> Scene {
    plate_of([
        unit_cube_mesh(),
        cuboid_with_z_tunnel(),
        cuboid_with_y_tunnel(),
    ])
}

const CREATED_AT: &str = "2026-09-04 17:31:02";

fn created_at() -> NonEmptyString {
    NonEmptyString::new(CREATED_AT).unwrap()
}

fn program(printer_toml: &str, total_layers: u32) -> PrintProgram {
    let config = resolved_config(printer_toml);
    let profile = slice_profile(&config, total_layers);
    project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap()
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
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();

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
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();

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
fn the_bottom_motion_stops_where_the_bottom_layers_do() {
    let config = resolved_config_for(PRINTER_TOML, MATERIAL_WITH_BOTTOM_MOTION_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();
    let bottom = BOTTOM_LAYER_COUNT as usize;

    for layer in &program.layers[..bottom] {
        assert_eq!(layer.lift.distance_mm.value(), 8.0);
        assert_eq!(layer.lift.speed_mm_min.value(), 40.0);
        assert_eq!(layer.retract.distance_mm.value(), 8.0);
        assert_eq!(layer.retract.speed_mm_min.value(), 90.0);
    }
    for layer in &program.layers[bottom..] {
        assert_eq!(layer.lift.distance_mm.value(), 5.0);
        assert_eq!(layer.lift.speed_mm_min.value(), 120.0);
        assert_eq!(layer.retract.distance_mm.value(), 5.0);
        assert_eq!(layer.retract.speed_mm_min.value(), 150.0);
    }
}

#[test]
fn a_split_travel_lands_as_the_vendor_files_record_it() {
    let config = resolved_config_for(PRINTER_TOML, MATERIAL_WITH_SPLIT_TRAVEL_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();

    for layer in &program.layers[BOTTOM_LAYER_COUNT as usize..] {
        assert_eq!(layer.lift.distance_mm.value(), 2.0);
        assert_eq!(layer.lift.speed_mm_min.value(), 75.0);
        let second_lift = layer.second_lift.expect("the travel splits in two");
        assert_eq!(second_lift.distance_mm.value(), 3.0);
        assert_eq!(second_lift.speed_mm_min.value(), 230.0);
        assert_eq!(layer.retract.distance_mm.value(), 3.0);
        assert_eq!(layer.retract.speed_mm_min.value(), 230.0);
        let second_retract = layer.second_retract.expect("the return splits in two");
        assert_eq!(second_retract.distance_mm.value(), 2.0);
        assert_eq!(second_retract.speed_mm_min.value(), 75.0);
    }
}

#[test]
fn a_split_travel_rises_and_returns_the_same_distance() {
    let config = resolved_config_for(PRINTER_TOML, MATERIAL_WITH_SPLIT_TRAVEL_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();

    for layer in &program.layers {
        let up = layer.lift.distance_mm.value()
            + layer.second_lift.expect("a split lift").distance_mm.value();
        let down = layer.retract.distance_mm.value()
            + layer
                .second_retract
                .expect("a split return")
                .distance_mm
                .value();
        assert_eq!(up, down, "the platform would drift");
    }
}

#[test]
fn the_bottom_layers_take_their_own_split_travel() {
    let config = resolved_config_for(PRINTER_TOML, MATERIAL_WITH_SPLIT_TRAVEL_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();

    for layer in &program.layers[..BOTTOM_LAYER_COUNT as usize] {
        assert_eq!(layer.lift.distance_mm.value(), 3.0);
        assert_eq!(layer.lift.speed_mm_min.value(), 60.0);
        let second_lift = layer.second_lift.expect("the travel splits in two");
        assert_eq!(second_lift.distance_mm.value(), 3.0);
        assert_eq!(second_lift.speed_mm_min.value(), 120.0);
        assert_eq!(layer.retract.speed_mm_min.value(), 120.0);
        assert_eq!(
            layer
                .second_retract
                .expect("the return splits in two")
                .speed_mm_min
                .value(),
            60.0,
        );
    }
}

#[test]
fn a_resin_that_moves_in_one_stage_leaves_the_split_travel_empty() {
    let program = program(PRINTER_TOML, 20);
    for layer in &program.layers {
        assert!(layer.second_lift.is_none());
        assert!(layer.second_retract.is_none());
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
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();

    for layer in &program.layers {
        assert_eq!(layer.wait_before_cure_sec.value(), 0.4);
        assert_eq!(layer.wait_after_cure_sec.value(), 1.2);
        assert_eq!(layer.wait_after_lift_sec.value(), 2.5);
    }
}

#[test]
fn the_bottom_waits_stop_where_the_bottom_layers_do() {
    let config = resolved_config_for(PRINTER_TOML, MATERIAL_WITH_BOTTOM_WAITS_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();
    let bottom = BOTTOM_LAYER_COUNT as usize;

    for layer in &program.layers[..bottom] {
        assert_eq!(layer.wait_before_cure_sec.value(), 6.0);
        assert_eq!(layer.wait_after_cure_sec.value(), 7.0);
        assert_eq!(layer.wait_after_lift_sec.value(), 8.0);
    }
    for layer in &program.layers[bottom..] {
        assert_eq!(layer.wait_before_cure_sec.value(), 0.4);
        assert_eq!(layer.wait_after_cure_sec.value(), 1.2);
        assert_eq!(layer.wait_after_lift_sec.value(), 2.5);
    }
}

#[test]
fn the_transition_band_holds_the_normal_waits_while_its_exposure_still_ramps() {
    let config = resolved_config_for(PRINTER_TOML, MATERIAL_WITH_BOTTOM_WAITS_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();
    let band = BOTTOM_LAYER_COUNT as usize..(BOTTOM_LAYER_COUNT + TRANSITION_COUNT) as usize;

    for layer in &program.layers[band] {
        assert!(layer.exposure_sec.value() > NORMAL_EXPOSURE_SEC);
        assert_eq!(layer.wait_before_cure_sec.value(), 0.4);
        assert_eq!(layer.wait_after_cure_sec.value(), 1.2);
        assert_eq!(layer.wait_after_lift_sec.value(), 2.5);
    }
}

#[test]
fn a_light_nobody_dialled_down_runs_at_full_power() {
    let program = program(PRINTER_TOML, 20);
    for layer in &program.layers {
        assert_eq!(layer.light_pwm, LightPwm::FULL);
    }
}

#[test]
fn the_bottom_light_pwm_stops_where_the_bottom_layers_do() {
    let config = resolved_config_for(PRINTER_TOML, MATERIAL_WITH_LIGHT_PWM_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();
    let bottom = BOTTOM_LAYER_COUNT as usize;

    for layer in &program.layers[..bottom] {
        assert_eq!(layer.light_pwm.percent(), 60.0);
    }
    for layer in &program.layers[bottom..] {
        assert_eq!(layer.light_pwm.percent(), 80.0);
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

    let err =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap_err();
    match err {
        UnprojectablePrint::SlabTop(err) => {
            assert_eq!(err.layer_index, 1);
            assert!(!err.z_mm.is_finite(), "z_mm: {}", err.z_mm);
        }
        other => panic!("expected a slab top failure, got {other:?}"),
    }
}

#[test]
fn the_program_reports_the_material_the_plate_encloses() {
    let program = program(PRINTER_TOML, 20);
    let volume = program.volume_mm3.value();
    assert!((volume - SCENE_VOLUME_MM3).abs() < 1e-6, "volume: {volume}",);
}

#[test]
fn the_program_weighs_that_material_at_the_resins_cured_density() {
    // The fixture resin states none, so the resolver's typical
    // photopolymer stands in: 14.44 mm3 at 1.17 g/cm3.
    let program = program(PRINTER_TOML, 20);
    let expected = SCENE_VOLUME_MM3 / 1_000.0 * 1.17;
    let mass = program.mass_g.value();
    assert!((mass - expected).abs() < 1e-9, "mass: {mass}");
}

#[test]
fn a_denser_resin_weighs_more_for_the_same_plate() {
    let dense = MATERIAL_TOML.replace(
        "[intent]",
        "[physical]\ncured_density_g_per_cm3 = 2.0\n\n[intent]",
    );
    let config = resolved_config_for(PRINTER_TOML, &dense);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();

    let expected = SCENE_VOLUME_MM3 / 1_000.0 * 2.0;
    let mass = program.mass_g.value();
    assert!((mass - expected).abs() < 1e-9, "mass: {mass}");
}

#[test]
fn an_empty_plate_weighs_nothing() {
    let config = resolved_config(PRINTER_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&plate_of([]), &config, &profile, created_at(), "0.0.12").unwrap();
    assert_eq!(program.mass_g.value(), 0.0);
}

/// What [`MATERIAL_FOR_TIMING_TOML`] asks of a 2 layer schedule:
/// 30 + 21 + 12 + 5 seconds for the bottom layer, 2.5 + 4.1 + 2.5 + 2
/// for the normal one.
const SCHEDULE_SEC: f64 = 68.0 + 11.1;

fn timed_program(printer_toml: &str) -> PrintProgram {
    let config = resolved_config_for(printer_toml, MATERIAL_FOR_TIMING_TOML);
    let profile = slice_profile(&config, 2);
    project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap()
}

#[test]
fn an_uncalibrated_printer_reports_what_the_schedule_itself_asks_for() {
    let reported = timed_program(PRINTER_WITHOUT_TRANSITION_LAYERS_TOML)
        .print_time_sec
        .value();
    assert!(
        (reported - SCHEDULE_SEC).abs() < 1e-9,
        "print time: {reported}",
    );
}

/// What [`MATERIAL_WITH_SPLIT_TRAVEL_TIMING_TOML`] asks of a 2 layer
/// schedule: 30 seconds of light and 6 of travel for the bottom
/// layer, 2 and 4 for the normal one.
const SPLIT_TRAVEL_SCHEDULE_SEC: f64 = 36.0 + 6.0;

#[test]
fn the_reported_time_counts_both_halves_of_a_split_travel() {
    let config = resolved_config_for(
        PRINTER_WITHOUT_TRANSITION_LAYERS_TOML,
        MATERIAL_WITH_SPLIT_TRAVEL_TIMING_TOML,
    );
    let profile = slice_profile(&config, 2);
    let program =
        project_to_print_program(&scene(), &config, &profile, created_at(), "0.0.12").unwrap();

    let reported = program.print_time_sec.value();
    assert!(
        (reported - SPLIT_TRAVEL_SCHEDULE_SEC).abs() < 1e-9,
        "print time: {reported}",
    );
}

#[test]
fn a_calibrated_printer_reports_the_corrected_schedule() {
    let calibrated = format!(
        "{PRINTER_WITHOUT_TRANSITION_LAYERS_TOML}\n[time_calibration]\nprint_time_multiplier = 1.5\nprint_time_offset_sec = 30.0\n",
    );
    let reported = timed_program(&calibrated).print_time_sec.value();
    assert!((reported - 148.65).abs() < 1e-9, "print time: {reported}");
}

#[test]
fn a_calibration_that_subtracts_more_than_the_print_takes_reports_no_time() {
    let calibrated = format!(
        "{PRINTER_WITHOUT_TRANSITION_LAYERS_TOML}\n[time_calibration]\nprint_time_offset_sec = -1000.0\n",
    );
    assert_eq!(timed_program(&calibrated).print_time_sec.value(), 0.0);
}

#[test]
fn an_empty_plate_encloses_nothing() {
    let config = resolved_config(PRINTER_TOML);
    let profile = slice_profile(&config, 20);
    let program =
        project_to_print_program(&plate_of([]), &config, &profile, created_at(), "0.0.12").unwrap();
    assert_eq!(program.volume_mm3.value(), 0.0);
}

#[test]
fn a_scene_that_cannot_be_measured_stops_the_projection() {
    let mut mesh = unit_cube_mesh();
    mesh.faces[0] = IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 99]);
    let scene = plate_of([mesh]);
    let config = resolved_config(PRINTER_TOML);
    let profile = slice_profile(&config, 20);

    let err =
        project_to_print_program(&scene, &config, &profile, created_at(), "0.0.12").unwrap_err();
    assert!(
        matches!(
            err,
            UnprojectablePrint::Scene(SceneError::InvalidMeshFaces { .. })
        ),
        "err: {err:?}",
    );
}

#[test]
fn the_creation_time_renders_as_a_date_and_a_wall_clock_time() {
    const SEPARATORS: [(usize, char); 5] = [(4, '-'), (7, '-'), (10, ' '), (13, ':'), (16, ':')];

    let rendered = creation_time_now();
    let text = rendered.as_str();

    assert_eq!(text.len(), 19, "text: {text}");
    for (index, separator) in SEPARATORS {
        assert_eq!(
            text.chars().nth(index),
            Some(separator),
            "position {index} of {text}",
        );
    }
    for (index, character) in text.char_indices() {
        if SEPARATORS.iter().any(|(at, _)| *at == index) {
            continue;
        }
        assert!(
            character.is_ascii_digit(),
            "position {index} of {text} should be a digit",
        );
    }
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
