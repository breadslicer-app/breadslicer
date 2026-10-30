/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::{
    EffectiveMaterial, MaterialProfile, PrinterHardware, PrinterProfile, ResolveError,
    SceneOverrides, resolve,
};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::profile::{AaLevel, LayerSchedule, PackingMode, SliceProfile};

use super::prepare::{
    UnsliceableScene, UnsliceableSelection, slice_profile_for_scene, slice_profile_for_selection,
};
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

fn printer() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_TOML).expect("printer fixture parses")
}

fn hardware() -> PrinterHardware {
    printer().hardware
}

// A generic material: an intent layer height and no per-printer calibration, so
// a full `resolve` rejects any pairing with it while the layer height alone
// resolves.
const GENERIC_MATERIAL_TOML: &str = r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[defaults.scale_compensation_pct]
x = 0.0
y = 0.0
z = 0.0

[per_printer]
"#;

fn generic_material() -> EffectiveMaterial {
    let profile: MaterialProfile =
        toml_edit::de::from_str(GENERIC_MATERIAL_TOML).expect("material fixture parses");
    EffectiveMaterial::from(profile)
}

// Same hardware, but with a 0.05 mm z step and a declared layer-height range,
// for the first-layer quantisation pins.
const CONSTRAINED_PRINTER_TOML: &str = r#"
[identity]
id = "constrained-printer"
manufacturer = "Example"
family = "Constrained"
name = "Constrained"
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
z_step_mm = 0.05
layer_height_range_mm = { min = 0.03, max = 0.1 }
"#;

fn constrained_printer() -> PrinterProfile {
    toml_edit::de::from_str(CONSTRAINED_PRINTER_TOML).expect("printer fixture parses")
}

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).expect("test lengths are positive finite")
}

/// Returns the 8 corners of the cuboid that spans `min`..`max`, with 2 faces:
/// enough for the AABB walk.
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
    let faces = vec![
        IndexedTriangle::from_raw_indices([0, 1, 2]),
        IndexedTriangle::from_raw_indices([0, 2, 3]),
    ];
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// Builds a single-leaf scene that holds `mesh` under an identity transform.
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
    // z_max == 0.0 sits on the boundary; nothing is above the plate, so the
    // positivity rule refuses it.
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
    // z_max 0.97, layer height 0.05, first layer 0.05 (fallback): 19 layers
    // are cut below the top, the last at 0.925; a 20th would be cut at 0.975,
    // above it.
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
    assert_eq!(uniform_schedule(&profile), (0.05, 0.05, 19));
}

#[test]
fn an_explicit_first_layer_height_overrides_the_fallback() {
    // z_max 0.97, layer height 0.05, first layer 0.15: the first layer and 16
    // more are cut below the top, the last at 0.925.
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
    assert_eq!(uniform_schedule(&profile), (0.05, 0.15, 17));
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
    // Unit cube: z in [-0.5, 0.5], z_max 0.5: beyond 0.45, ceil(0.45/0.05) = 9,
    // total 10. Both tunnel prisms: z in [0, 2], z_max 2.0: beyond 1.95,
    // ceil(1.95/0.05) = 39, total 40.
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
fn a_selection_without_print_calibration_still_slices() {
    // Geometry slicing consumes the layer height only; the exposure and motion
    // calibration belongs to the printer-file export step. A full `resolve`
    // rejects the fixture pairing, yet the selection entry produces a profile.
    let (printer, material) = (printer(), generic_material());
    assert!(
        resolve(&printer, &material, &SceneOverrides::default()).is_err(),
        "the fixture pairing must be one a full resolve rejects",
    );

    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]));
    let profile = slice_profile_for_selection(
        &scene,
        &printer,
        &material,
        None,
        None,
        AaLevel::Off,
        PackingMode::None,
    )
    .expect("a selection without print calibration must still slice");
    assert_eq!(uniform_schedule(&profile), (0.05, 0.05, 19));
    assert_eq!(profile.build_volume().x_mm(), 153.36);
}

#[test]
fn a_layer_height_request_reaches_the_schedule() {
    // Request 0.1 over an intent of 0.05: the request wins and the layer count
    // follows it (z_max 0.97, first layer 0.1 fallback: beyond 0.87,
    // ceil(0.87/0.1) = 9, total 10).
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]));
    let profile = slice_profile_for_selection(
        &scene,
        &printer(),
        &generic_material(),
        Some(mm(0.1)),
        None,
        AaLevel::Off,
        PackingMode::None,
    )
    .expect("a valid request derives");
    assert_eq!(uniform_schedule(&profile), (0.1, 0.1, 10));
}

#[test]
fn a_first_layer_off_the_z_step_grid_is_quantised() {
    // First layer 0.08 on a 0.05 z step snaps to 0.1; the layer height itself
    // (0.05 from intent) is already on the grid. z_max 0.97, first 0.1: the
    // first layer and 17 more are cut below the top, the last at 0.925.
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]));
    let profile = slice_profile_for_selection(
        &scene,
        &constrained_printer(),
        &generic_material(),
        None,
        Some(mm(0.08)),
        AaLevel::Off,
        PackingMode::None,
    )
    .expect("valid derivation");
    assert_eq!(uniform_schedule(&profile), (0.05, 0.1, 18));
}

#[test]
fn the_layer_height_range_does_not_bind_the_first_layer() {
    // 0.2 sits outside the declared [0.03, 0.1] range but on the 0.05 grid: it
    // passes unchanged. First layers can run thicker than the curing range of
    // normal layers. z_max 0.97, first 0.2: the first layer and 15 more are
    // cut below the top, the last at 0.925.
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]));
    let profile = slice_profile_for_selection(
        &scene,
        &constrained_printer(),
        &generic_material(),
        None,
        Some(mm(0.2)),
        AaLevel::Off,
        PackingMode::None,
    )
    .expect("valid derivation");
    assert_eq!(uniform_schedule(&profile), (0.05, 0.2, 16));
}

#[test]
fn an_unresolvable_layer_height_is_a_selection_failure() {
    // Strip the intent so that nothing supplies a layer height and no request
    // is made.
    let material_toml = GENERIC_MATERIAL_TOML.replace("target_layer_height_mm = 0.05", "");
    let profile: MaterialProfile =
        toml_edit::de::from_str(&material_toml).expect("material fixture parses");
    let scene = leaf_scene(cube_mesh([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]));
    let result = slice_profile_for_selection(
        &scene,
        &printer(),
        &EffectiveMaterial::from(profile),
        None,
        None,
        AaLevel::Off,
        PackingMode::None,
    );
    assert!(matches!(
        result,
        Err(UnsliceableSelection::LayerHeight(
            ResolveError::MissingRequiredField { .. }
        ))
    ));
}

#[test]
fn a_scene_failure_passes_through_the_selection_entry() {
    let result = slice_profile_for_selection(
        &Scene::new(),
        &printer(),
        &generic_material(),
        None,
        None,
        AaLevel::Off,
        PackingMode::None,
    );
    assert!(matches!(
        result,
        Err(UnsliceableSelection::Scene(UnsliceableScene::NoGeometry))
    ));
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

#[test]
fn a_rotated_model_gets_layers_up_to_its_highest_vertex() {
    use sc_mesh_core::transform::AffineTransform3D;
    // An octahedron (vertices on the axes at distance 1) turned 45 degrees
    // about X and raised by 1: its top vertex is at 1 + sin 45 = 1.707, so
    // 17 layers of 0.1 mm, the last cut at 1.65. Its transformed local box
    // reaches 1 + sqrt 2, which would schedule 24.
    let v = |p: [f32; 3]| Vertex::new(p);
    let octahedron = IndexedMesh {
        meta: None,
        vertices: vec![
            v([1.0, 0.0, 0.0]),
            v([-1.0, 0.0, 0.0]),
            v([0.0, 1.0, 0.0]),
            v([0.0, -1.0, 0.0]),
            v([0.0, 0.0, 1.0]),
            v([0.0, 0.0, -1.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices([0, 2, 4])],
    };
    let placed = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 1.0])).compose(
        &Transform3D::Affine(AffineTransform3D::from_rotation_x(
            std::f64::consts::FRAC_PI_4,
        )),
    );
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(None, placed, LeafPayload::Mesh(Arc::new(octahedron)))
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
    let profile = slice_profile_for_scene(
        &scene,
        &hardware(),
        mm(0.1),
        None,
        AaLevel::Off,
        PackingMode::None,
    )
    .expect("valid derivation");
    assert_eq!(uniform_schedule(&profile), (0.1, 0.1, 17));
}
