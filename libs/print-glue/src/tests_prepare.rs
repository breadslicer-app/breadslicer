/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::node::LeafPayload;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::{PrinterHardware, PrinterProfile};
use sc_sl1cer::profile::{AaLevel, LayerSchedule, PackingMode, SliceProfile};

use super::prepare::{UnsliceableScene, slice_profile_for_scene};
use super::projection::UnschedulableHeight;

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
"#;

fn hardware() -> PrinterHardware {
    let printer: PrinterProfile =
        toml_edit::de::from_str(PRINTER_TOML).expect("printer fixture parses");
    printer.hardware
}

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).expect("test lengths are positive finite")
}

/// A cuboid spanning `min`..`max`; enough faces for the AABB walk.
fn cube_mesh(min: [f32; 3], max: [f32; 3]) -> IndexedMesh {
    let v = |x, y, z| Vertex::new([x, y, z]);
    let vertices = vec![
        v(min[0], min[1], min[2]),
        v(max[0], min[1], min[2]),
        v(max[0], max[1], min[2]),
        v(min[0], max[1], min[2]),
        v(min[0], min[1], max[2]),
        v(max[0], min[1], max[2]),
        v(max[0], max[1], max[2]),
        v(min[0], max[1], max[2]),
    ];
    let n = Normal::new([0.0, 0.0, 1.0]);
    let faces = vec![
        IndexedTriangle::from_raw_indices(n, [0, 1, 2]),
        IndexedTriangle::from_raw_indices(n, [0, 2, 3]),
    ];
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// Single-leaf scene holding `mesh` under an identity transform.
fn leaf_scene(mesh: IndexedMesh) -> Scene {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
    scene
}

fn uniform_schedule(profile: &SliceProfile) -> (f64, f64, u32) {
    match profile.layer_schedule() {
        LayerSchedule::Uniform(uniform) => (
            uniform.layer_height().value(),
            uniform.first_layer_height().value(),
            uniform.total_layers().get(),
        ),
    }
}

#[test]
fn an_empty_scene_has_no_geometry() {
    let result = slice_profile_for_scene(
        &Scene::new(),
        &hardware(),
        mm(0.05),
        None,
        AaLevel::Off,
        PackingMode::None,
    );
    assert!(matches!(result, Err(UnsliceableScene::NoGeometry)));
}

#[test]
fn a_scene_below_the_plate_is_rejected() {
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, -2.0], [1.0, 1.0, -1.0]));
    let result = slice_profile_for_scene(
        &scene,
        &hardware(),
        mm(0.05),
        None,
        AaLevel::Off,
        PackingMode::None,
    );
    assert!(matches!(
        result,
        Err(UnsliceableScene::TopAtOrBelowPlate { z_max_mm }) if z_max_mm == -1.0
    ));
}

#[test]
fn a_scene_topping_exactly_at_the_plate_is_rejected() {
    // z_max == 0.0 sits on the boundary; nothing is above the plate,
    // so the positivity rule refuses it.
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, -1.0], [1.0, 1.0, 0.0]));
    let result = slice_profile_for_scene(
        &scene,
        &hardware(),
        mm(0.05),
        None,
        AaLevel::Off,
        PackingMode::None,
    );
    assert!(matches!(
        result,
        Err(UnsliceableScene::TopAtOrBelowPlate { z_max_mm }) if z_max_mm == 0.0
    ));
}

#[test]
fn absent_first_layer_height_falls_back_to_the_layer_height() {
    // z_max 0.97, layer height 0.05, first layer 0.05 (fallback):
    // beyond = 0.92, ceil(0.92 / 0.05) = 19, total = 20.
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]));
    let profile = slice_profile_for_scene(
        &scene,
        &hardware(),
        mm(0.05),
        None,
        AaLevel::Off,
        PackingMode::None,
    )
    .expect("valid derivation");
    assert_eq!(uniform_schedule(&profile), (0.05, 0.05, 20));
}

#[test]
fn an_explicit_first_layer_height_overrides_the_fallback() {
    // z_max 0.97, layer height 0.05, first layer 0.15:
    // beyond = 0.82, ceil(0.82 / 0.05) = 17, total = 18.
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]));
    let profile = slice_profile_for_scene(
        &scene,
        &hardware(),
        mm(0.05),
        Some(mm(0.15)),
        AaLevel::Off,
        PackingMode::None,
    )
    .expect("valid derivation");
    assert_eq!(uniform_schedule(&profile), (0.05, 0.15, 18));
}

#[test]
fn the_printer_hardware_is_carried_verbatim() {
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]));
    let profile = slice_profile_for_scene(
        &scene,
        &hardware(),
        mm(0.05),
        None,
        AaLevel::X4,
        PackingMode::None,
    )
    .expect("valid derivation");
    assert_eq!(profile.build_volume().x_mm(), 153.36);
    assert_eq!(profile.build_volume().y_mm(), 77.76);
    assert_eq!(profile.build_volume().z_mm(), 165.0);
    assert_eq!(profile.pixel_resolution().x(), 8520);
    assert_eq!(profile.pixel_resolution().y(), 4320);
    assert!(profile.mirror().x);
    assert!(!profile.mirror().y);
    assert_eq!(profile.aa_level(), AaLevel::X4);
}

#[test]
fn an_absurdly_tall_scene_is_unschedulable() {
    // f32::MAX as the top needs more layers than a u32 can hold.
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, f32::MAX]));
    let result = slice_profile_for_scene(
        &scene,
        &hardware(),
        mm(0.05),
        None,
        AaLevel::Off,
        PackingMode::None,
    );
    assert!(matches!(
        result,
        Err(UnsliceableScene::Unschedulable(
            UnschedulableHeight::LayerCountOverflow { .. }
        ))
    ));
}

#[test]
fn a_non_finite_scene_top_is_unschedulable() {
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, f32::INFINITY]));
    let result = slice_profile_for_scene(
        &scene,
        &hardware(),
        mm(0.05),
        None,
        AaLevel::Off,
        PackingMode::None,
    );
    assert!(matches!(
        result,
        Err(UnsliceableScene::Unschedulable(
            UnschedulableHeight::NonFiniteHeight { .. }
        ))
    ));
}

#[test]
fn the_fixture_trio_derives_the_expected_schedules() {
    // unit cube: z in [-0.5, 0.5], z_max 0.5: beyond = 0.45,
    // ceil(0.45 / 0.05) = 9, total = 10. Both tunnel prisms: z in
    // [0, 2], z_max 2.0: beyond = 1.95, ceil(1.95 / 0.05) = 39,
    // total = 40.
    let cases: [(IndexedMesh, u32); 3] = [
        (unit_cube_mesh(), 10),
        (cuboid_with_z_tunnel(), 40),
        (cuboid_with_y_tunnel(), 40),
    ];
    for (mesh, expected_layers) in cases {
        let scene = leaf_scene(mesh);
        let profile = slice_profile_for_scene(
            &scene,
            &hardware(),
            mm(0.05),
            None,
            AaLevel::Off,
            PackingMode::None,
        )
        .expect("the fixture trio derives");
        let (layer_height, first_layer_height, total) = uniform_schedule(&profile);
        assert_eq!(layer_height, 0.05);
        assert_eq!(first_layer_height, 0.05);
        assert_eq!(total, expected_layers);
    }
}

#[test]
fn display_texts_name_the_failure() {
    let no_geometry = UnsliceableScene::NoGeometry;
    assert_eq!(
        no_geometry.to_string(),
        "the scene contributes no geometry to slice",
    );
    let below = UnsliceableScene::TopAtOrBelowPlate { z_max_mm: -1.5 };
    assert_eq!(
        below.to_string(),
        "the scene's top is at z = -1.5 mm, at or below the build plate at z = 0",
    );
    let unschedulable = UnsliceableScene::Unschedulable(UnschedulableHeight::NonFiniteHeight {
        z_max_mm: f64::NAN,
    });
    assert_eq!(
        unschedulable.to_string(),
        "the scene height yields no valid layer count: the scene height z = NaN mm is not a finite value",
    );
}
