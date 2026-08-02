/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pre-slice validation must reject meshes whose baked vertices carry
//! NaN or infinite coordinates. Covers direct mesh input (a poisoned
//! coordinate on the unit cube and on both tunnel prisms) and the
//! transform-overflow path (a finite scale whose f64-to-f32 downcast
//! saturates the baked vertices to infinity), plus the scene-level
//! aggregation that must name only the offending leaf.

use std::sync::Arc;

use sc_mesh_core::scene::node::{LeafPayload, NodeId, SceneMetadata};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, Scene, Vertex};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::error::{InvalidLeafGeometry, SliceError, SliceSceneError};
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::{slice_leaf, slice_scene};

/// Eight uniform 0.25 mm slabs, the same schedule the multi-leaf
/// suite uses; the exact planes are irrelevant here because
/// validation must reject the leaf before any plane is visited.
fn profile() -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(20.0, 20.0, 20.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        LayerSchedule::uniform(0.25, 0.25, 8).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

/// Replace one coordinate of vertex `vertex_idx` with `value`,
/// keeping the other 2 coordinates intact. The mutation only touches
/// coordinate data, so the mesh stays edge-manifold by construction.
fn poison_coordinate(mesh: &mut IndexedMesh, vertex_idx: usize, axis: usize, value: f32) {
    let v = mesh.vertices[vertex_idx];
    let mut coords = [v[0], v[1], v[2]];
    coords[axis] = value;
    mesh.vertices[vertex_idx] = Vertex::new(coords);
}

fn single_leaf_scene(mesh: IndexedMesh, transform: Transform3D) -> (Scene, NodeId) {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
        .expect("adding a mesh leaf succeeds");
    scene.add_root(leaf).expect("adding the root succeeds");
    (scene, leaf)
}

fn named(name: &str) -> Option<SceneMetadata> {
    Some(SceneMetadata {
        name: Some(name.to_string()),
        ..Default::default()
    })
}

/// Assert `result` is the vertex-validation rejection: the dedicated
/// [`SliceError::VerticesValidationFailed`] variant carrying at least
/// one error, with the expected per-variant summary line in its
/// rendering.
fn assert_vertex_rejection(result: Result<ContourStack, SliceError>, expected_variant: &str) {
    let err = result.expect_err("non-finite vertices must fail validation");
    assert!(
        matches!(err, SliceError::VerticesValidationFailed(ref errors) if !errors.is_empty()),
        "expected VerticesValidationFailed, got: {err:?}",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("vertex validation failed"),
        "error must come from the vertex stage; got: {msg}",
    );
    assert!(
        msg.contains(expected_variant),
        "summary must name {expected_variant}; got: {msg}",
    );
}

#[test]
fn nan_x_on_the_cube_is_rejected() {
    let mut mesh = unit_cube_mesh();
    poison_coordinate(&mut mesh, 0, 0, f32::NAN);
    let (scene, leaf) = single_leaf_scene(mesh, Transform3D::Identity);
    assert_vertex_rejection(slice_leaf(&scene, leaf, &profile()), "NaNCoordinate");
}

#[test]
fn nan_y_on_the_z_tunnel_prism_is_rejected() {
    let mut mesh = cuboid_with_z_tunnel();
    poison_coordinate(&mut mesh, 0, 1, f32::NAN);
    let (scene, leaf) = single_leaf_scene(mesh, Transform3D::Identity);
    assert_vertex_rejection(slice_leaf(&scene, leaf, &profile()), "NaNCoordinate");
}

#[test]
fn nan_z_on_the_y_tunnel_prism_is_rejected() {
    let mut mesh = cuboid_with_y_tunnel();
    poison_coordinate(&mut mesh, 0, 2, f32::NAN);
    let (scene, leaf) = single_leaf_scene(mesh, Transform3D::Identity);
    assert_vertex_rejection(slice_leaf(&scene, leaf, &profile()), "NaNCoordinate");
}

#[test]
fn infinite_z_on_the_cube_is_rejected() {
    let mut mesh = unit_cube_mesh();
    poison_coordinate(&mut mesh, 0, 2, f32::INFINITY);
    let (scene, leaf) = single_leaf_scene(mesh, Transform3D::Identity);
    assert_vertex_rejection(slice_leaf(&scene, leaf, &profile()), "InfiniteCoordinate");
}

/// A finite uniform scale whose product with the cube's coordinates
/// exceeds f32 range: the f64 ancestor-chain composition stays
/// finite, the per-vertex downcast saturates to infinity. The gate
/// runs on the BAKED mesh, so this must be rejected exactly like a
/// poisoned source mesh. Before the gate existed this sliced "fine"
/// into all-empty layers.
#[test]
fn transform_overflow_is_rejected_after_baking() {
    let transform = Transform3D::Affine(AffineTransform3D::from_uniform_scale(1.0e39));
    let (scene, leaf) = single_leaf_scene(unit_cube_mesh(), transform);
    assert_vertex_rejection(slice_leaf(&scene, leaf, &profile()), "InfiniteCoordinate");
}

/// Scene-level aggregation: the clean leaf must not be reported, the
/// poisoned leaf must be named with the vertex-stage summary.
#[test]
fn slice_scene_reports_only_the_poisoned_leaf() {
    let mut poisoned = cuboid_with_z_tunnel();
    poison_coordinate(&mut poisoned, 0, 0, f32::NAN);

    let mut scene = Scene::new();
    let clean_leaf = scene
        .add_leaf(
            named("clean-cube"),
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("adding the clean leaf succeeds");
    let bad_leaf = scene
        .add_leaf(
            named("poisoned-prism"),
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(poisoned)),
        )
        .expect("adding the poisoned leaf succeeds");
    scene.add_root(clean_leaf).expect("clean root");
    scene.add_root(bad_leaf).expect("poisoned root");

    let err = slice_scene(&scene, &profile())
        .expect_err("a scene with a non-finite leaf must fail validation");
    match &err {
        SliceSceneError::LeavesValidationFailed(failures) => {
            assert_eq!(failures.len(), 1, "exactly the poisoned leaf must fail");
            assert!(
                matches!(failures[0].geometry, InvalidLeafGeometry::Vertices(_)),
                "the failure must carry the vertex stage's errors; got: {:?}",
                failures[0].geometry,
            );
            assert_eq!(failures[0].leaf_name.as_deref(), Some("poisoned-prism"));
        }
        other => panic!("expected LeavesValidationFailed, got: {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("poisoned-prism"),
        "the failing leaf must be named; got: {msg}",
    );
    assert!(
        msg.contains("vertex validation failed"),
        "the failure must come from the vertex stage; got: {msg}",
    );
    assert!(
        !msg.contains("clean-cube"),
        "the clean leaf must not be listed; got: {msg}",
    );
}
