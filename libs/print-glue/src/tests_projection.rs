/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::num::NonZeroU32;

use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::{MaterialProfile, PrinterProfile, ResolvedPrintConfig, SceneOverrides};
use sc_sl1cer::profile::{AaLevel, LayerSchedule, PackingMode};

use super::projection::{project_hardware_to_slice_profile, project_to_slice_profile};

fn layers(count: u32) -> NonZeroU32 {
    NonZeroU32::new(count).expect("test counts are non-zero")
}

const PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-saturn-4-ultra"
manufacturer = "ELEGOO"
family = "Saturn"
name = "Saturn 4 Ultra"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
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
scale_compensation_pct = { x = 0.5, y = 0.5, z = 0.0 }

[per_printer."elegoo-saturn-4-ultra"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

fn resolved_config() -> ResolvedPrintConfig {
    let printer: PrinterProfile = toml_edit::de::from_str(PRINTER_TOML).unwrap();
    let material: MaterialProfile = toml_edit::de::from_str(MATERIAL_TOML).unwrap();
    sc_print_profiles::resolve(&printer, &material, &SceneOverrides::default()).unwrap()
}

#[test]
fn projection_copies_hardware_geometry_verbatim() {
    let config = resolved_config();
    let profile = project_to_slice_profile(
        &config,
        AaLevel::Off,
        PackingMode::None,
        PositiveFiniteMm::new(0.1).unwrap(),
        layers(20),
    );

    assert_eq!(profile.build_volume().x_mm(), 153.36);
    assert_eq!(profile.build_volume().y_mm(), 77.76);
    assert_eq!(profile.build_volume().z_mm(), 165.0);
    assert_eq!(profile.pixel_resolution().x(), 8520);
    assert_eq!(profile.pixel_resolution().y(), 4320);
    assert!(profile.mirror().x);
    assert!(!profile.mirror().y);
}

#[test]
fn projection_uses_resolved_layer_height_and_passed_scene_params() {
    let config = resolved_config();
    let profile = project_to_slice_profile(
        &config,
        AaLevel::X4,
        PackingMode::None,
        PositiveFiniteMm::new(0.15).unwrap(),
        layers(20),
    );

    assert_eq!(profile.aa_level(), AaLevel::X4);
    assert_eq!(profile.packing_mode(), PackingMode::None);
    match profile.layer_schedule() {
        LayerSchedule::Uniform(uniform) => {
            assert_eq!(uniform.layer_height().value(), 0.05);
            assert_eq!(uniform.first_layer_height().value(), 0.15);
            assert_eq!(uniform.total_layers().get(), 20);
        }
    }
}

#[test]
fn projection_carries_printer_machine_z_not_a_mesh_height() {
    // The build volume Z is the printer's machine maximum, taken
    // verbatim; it is not derived from any mesh extent.
    let config = resolved_config();
    let profile = project_to_slice_profile(
        &config,
        AaLevel::Off,
        PackingMode::None,
        PositiveFiniteMm::new(0.05).unwrap(),
        layers(5),
    );
    assert_eq!(profile.build_volume().z_mm(), 165.0);
}

#[test]
fn hardware_projection_builds_from_hardware_and_layer_height_alone() {
    // The geometry-only path: no ResolvedPrintConfig, just the hardware
    // and an explicit layer height.
    let config = resolved_config();
    let profile = project_hardware_to_slice_profile(
        &config.printer.hardware,
        PositiveFiniteMm::new(0.05).unwrap(),
        AaLevel::Off,
        PackingMode::None,
        PositiveFiniteMm::new(0.1).unwrap(),
        layers(10),
    );

    assert_eq!(profile.build_volume().x_mm(), 153.36);
    assert_eq!(profile.pixel_resolution().x(), 8520);
    assert!(profile.mirror().x);
    match profile.layer_schedule() {
        LayerSchedule::Uniform(uniform) => {
            assert_eq!(uniform.layer_height().value(), 0.05);
            assert_eq!(uniform.first_layer_height().value(), 0.1);
            assert_eq!(uniform.total_layers().get(), 10);
        }
    }
}
