/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_print_primitives::{AaLevel, PositiveFiniteMm};
use sc_print_profiles::{EffectiveMaterial, MaterialProfile, PrinterProfile};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::CancelToken;
use sc_sl1cer::error::SliceSceneError;

use super::prepare::UnsliceableSelection;
use super::slice_plate::{
    SlicePlateError, SliceRun, SlicedPlate, SlicingParams, schedule_for_selection, slice_plate,
};

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

const MATERIAL_TOML: &str = r#"
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

[per_printer]
"#;

/// A material carrying no layer-height source at all, so the
/// schedule cannot resolve without a per-print request.
const HEIGHTLESS_MATERIAL_TOML: &str = r#"
[identity]
id = "no-height"
brand = "Default"
family = "standard"
name = "No Height"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer]
"#;

fn printer() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_TOML).expect("printer fixture parses")
}

fn material() -> EffectiveMaterial {
    let profile: MaterialProfile =
        toml_edit::de::from_str(MATERIAL_TOML).expect("material fixture parses");
    EffectiveMaterial::from(profile)
}

fn heightless_material() -> EffectiveMaterial {
    let profile: MaterialProfile =
        toml_edit::de::from_str(HEIGHTLESS_MATERIAL_TOML).expect("material fixture parses");
    EffectiveMaterial::from(profile)
}

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).expect("test lengths are positive finite")
}

/// 0.25 mm layers; the cube (top at z = 0.5) covers in 2, the
/// prisms (top at z = 2) in 8.
fn params() -> SlicingParams {
    SlicingParams {
        layer_height_request: Some(mm(0.25)),
        first_layer_height: None,
        aa_level: AaLevel::Off,
    }
}

fn scene_with(meshes: impl IntoIterator<Item = IndexedMesh>) -> Scene {
    let mut scene = Scene::new();
    for mesh in meshes {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
    }
    scene
}

fn completed(scene: &Scene) -> SlicedPlate {
    match slice_plate(
        scene,
        &printer(),
        &material(),
        &params(),
        &CancelToken::new(),
        |_| {},
    )
    .expect("the fixture scene slices")
    {
        SliceRun::Completed(plate) => plate,
        SliceRun::Cancelled => panic!("an uncancelled run cannot report Cancelled"),
    }
}

#[test]
fn the_cube_slices_to_the_requested_layer_grid() {
    let plate = completed(&scene_with([unit_cube_mesh()]));
    assert_eq!(plate.schedule().total_layers(), 2);
    assert_eq!(plate.leaves().len(), 1);
    let stack = plate.leaves()[0].stack();
    assert_eq!(stack.layers().len(), 2);
    for layer in stack.layers() {
        assert_eq!(layer.polygons().len(), 1, "one solid square per layer");
        assert!(layer.polygons()[0].holes().is_empty());
    }
}

#[test]
fn the_z_tunnel_prism_carries_one_polygon_with_one_hole_in_every_layer() {
    let plate = completed(&scene_with([cuboid_with_z_tunnel()]));
    let stack = plate.leaves()[0].stack();
    assert_eq!(stack.layers().len(), 8);
    for layer in stack.layers() {
        assert_eq!(layer.polygons().len(), 1);
        assert_eq!(layer.polygons()[0].holes().len(), 1);
    }
}

#[test]
fn the_y_tunnel_prism_transitions_between_one_and_two_polygons() {
    let plate = completed(&scene_with([cuboid_with_y_tunnel()]));
    let stack = plate.leaves()[0].stack();
    assert_eq!(stack.layers().len(), 8);
    let counts: Vec<usize> = stack
        .layers()
        .iter()
        .map(|layer| layer.polygons().len())
        .collect();
    assert_eq!(counts[0], 1, "below the tunnel: solid");
    assert_eq!(counts[3], 2, "through the tunnel: split in two");
    assert_eq!(counts[4], 2, "through the tunnel: split in two");
    assert_eq!(counts[7], 1, "above the tunnel: solid");
    for layer in [stack.layers().first(), stack.layers().last()]
        .into_iter()
        .flatten()
    {
        assert!(layer.polygons()[0].holes().is_empty());
    }
}

#[test]
fn the_trio_merges_into_one_plate_stack() {
    let plate = completed(&scene_with([
        unit_cube_mesh(),
        cuboid_with_z_tunnel(),
        cuboid_with_y_tunnel(),
    ]));
    assert_eq!(plate.leaves().len(), 3);
    let merged = plate.merge().expect("stacks share one schedule");
    assert_eq!(merged.layers().len(), 8);
    // Layer 0 carries all 3 leaves; the cube (top at 0.5) is gone
    // well before the last layer, where the Y-tunnel prism is solid
    // again and the Z-tunnel prism still has its hole.
    assert_eq!(merged.layers()[0].polygons().len(), 3);
    assert_eq!(merged.layers()[7].polygons().len(), 2);
}

#[test]
fn a_pre_cancelled_token_yields_cancelled() {
    let cancel = CancelToken::new();
    cancel.cancel();
    let run = slice_plate(
        &scene_with([unit_cube_mesh()]),
        &printer(),
        &material(),
        &params(),
        &cancel,
        |_| {},
    )
    .expect("cancellation is an outcome, not an error");
    assert!(matches!(run, SliceRun::Cancelled));
}

#[test]
fn progress_ends_complete() {
    let last_completed = AtomicU32::new(0);
    let last_total = AtomicU32::new(0);
    let run = slice_plate(
        &scene_with([unit_cube_mesh()]),
        &printer(),
        &material(),
        &params(),
        &CancelToken::new(),
        |progress| {
            last_completed.fetch_max(progress.completed as u32, Ordering::Relaxed);
            last_total.store(progress.total as u32, Ordering::Relaxed);
        },
    )
    .expect("the cube slices");
    assert!(matches!(run, SliceRun::Completed(_)));
    assert_eq!(last_total.load(Ordering::Relaxed), 2);
    assert_eq!(last_completed.load(Ordering::Relaxed), 2);
}

#[test]
fn an_unresolvable_layer_height_is_a_prepare_error() {
    let no_request = SlicingParams {
        layer_height_request: None,
        first_layer_height: None,
        aa_level: AaLevel::Off,
    };
    let err = slice_plate(
        &scene_with([unit_cube_mesh()]),
        &printer(),
        &heightless_material(),
        &no_request,
        &CancelToken::new(),
        |_| {},
    )
    .expect_err("no layer height resolves");
    assert!(matches!(
        err,
        SlicePlateError::Prepare(UnsliceableSelection::LayerHeight(_)),
    ));
}

#[test]
fn an_empty_scene_is_a_prepare_error() {
    let err = slice_plate(
        &Scene::new(),
        &printer(),
        &material(),
        &params(),
        &CancelToken::new(),
        |_| {},
    )
    .expect_err("no geometry to slice");
    assert!(matches!(
        err,
        SlicePlateError::Prepare(UnsliceableSelection::Scene(_)),
    ));
}

#[test]
fn an_invalid_leaf_is_an_engine_error() {
    let broken = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 1.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 9],
        )],
    };
    let err = slice_plate(
        &scene_with([broken]),
        &printer(),
        &material(),
        &params(),
        &CancelToken::new(),
        |_| {},
    )
    .expect_err("the leaf fails validation");
    assert!(matches!(
        err,
        SlicePlateError::Engine(SliceSceneError::LeavesValidationFailed(_)),
    ));
}

#[test]
fn stacks_from_different_schedules_refuse_to_merge() {
    let cube = completed(&scene_with([unit_cube_mesh()]));
    let prism = completed(&scene_with([cuboid_with_z_tunnel()]));
    // Splice leaves sliced under 2-layer and 8-layer grids into one
    // plate; only reachable by hand, and merge must refuse, typed.
    let mut leaves = cube.leaves;
    leaves.extend(prism.leaves);
    let mixed = SlicedPlate {
        leaves,
        profile: cube.profile,
    };
    assert!(mixed.merge().is_err());
}

#[test]
fn schedule_for_selection_matches_the_sliced_schedule() {
    let scene = scene_with([unit_cube_mesh()]);
    let schedule = schedule_for_selection(&scene, &printer(), &material(), &params())
        .expect("the schedule resolves");
    let plate = completed(&scene);
    assert_eq!(&schedule, plate.schedule());
}
