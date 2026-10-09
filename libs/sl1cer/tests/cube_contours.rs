/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end test of [`slice_leaf`] against a unit cube spanning `[0, 1]^3`
//! millimetres. 8 uniform layers cover the interior of the cube; every layer
//! must carry 1 CCW unit-area polygon with no holes, and the output must be
//! deterministic across runs.

use std::sync::Arc;

use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, NodeId};
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::slice_leaf;

/// Tolerance on the per-layer cross-section area. The analytic cross-section of
/// the cube at any Z in (0, 1) is exactly 1.0 mm^2; segment interpolation runs
/// in f64, so the FP noise here stays at machine epsilon.
const AREA_TOLERANCE_MM2: f64 = 1e-9;

/// Returns a closed unit cube spanning `[0, 1]^3`: 8 vertices, 12 triangles,
/// wound outward. Manifold by construction.
fn unit_cube_zero_to_one() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([0.0, 0.0, 0.0]),
        Vertex::new([1.0, 0.0, 0.0]),
        Vertex::new([1.0, 1.0, 0.0]),
        Vertex::new([0.0, 1.0, 0.0]),
        Vertex::new([0.0, 0.0, 1.0]),
        Vertex::new([1.0, 0.0, 1.0]),
        Vertex::new([1.0, 1.0, 1.0]),
        Vertex::new([0.0, 1.0, 1.0]),
    ];
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            f([0, 3, 2]),
            f([0, 2, 1]),
            f([4, 5, 6]),
            f([4, 6, 7]),
            f([3, 7, 6]),
            f([3, 6, 2]),
            f([0, 1, 5]),
            f([0, 5, 4]),
            f([1, 2, 6]),
            f([1, 6, 5]),
            f([0, 4, 7]),
            f([0, 7, 3]),
        ],
    }
}

/// Returns a profile with 8 uniform layers spanning `[0, 1]` mm in Z: layer
/// thickness 0.125 mm, first-layer thickness also 0.125 mm. Centred Z values:
/// 0.0625, 0.1875, ..., 0.9375. Each Z sits strictly inside the cube.
fn eight_uniform_layers() -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(10.0, 10.0, 10.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        LayerSchedule::uniform(0.125, 0.125, 8).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

fn scene_with_cube_root() -> (Scene, NodeId) {
    let mut scene = Scene::new();
    let leaf_id = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_cube_zero_to_one())),
        )
        .expect("add_leaf accepts an inline mesh");
    scene
        .add_root(leaf_id)
        .expect("add_root accepts a freshly added leaf");
    (scene, leaf_id)
}

#[test]
fn every_layer_carries_exactly_one_ccw_polygon_with_unit_area() {
    let (scene, leaf_id) = scene_with_cube_root();
    let profile = eight_uniform_layers();
    let stack = slice_leaf(&scene, leaf_id, &profile).expect("cube slices cleanly");

    assert_eq!(stack.layers().len(), 8);
    for (i, layer) in stack.layers().iter().enumerate() {
        let z = layer.z_mm();
        assert_eq!(
            layer.polygons().len(),
            1,
            "layer {i} (z = {z} mm) should hold one polygon",
        );
        let polygon = &layer.polygons()[0];
        assert!(
            polygon.holes().is_empty(),
            "layer {i} polygon must have no holes",
        );
        // Each side face is triangulated into 2 triangles, so the Z plane
        // crosses 8 edges per layer: 4 vertical corner edges and 4
        // face-diagonal edges (1 per side face). The stitched outer ring traces
        // the square boundary through 4 corner crossings plus 4 face-diagonal
        // midpoints.
        assert_eq!(
            polygon.outer().vertices().len(),
            8,
            "layer {i} outer ring must have eight vertices",
        );
        let area = polygon.outer().signed_area();
        assert!(
            area > 0.0,
            "layer {i} outer ring must be CCW (area > 0), got {area}",
        );
        assert!(
            (area - 1.0).abs() <= AREA_TOLERANCE_MM2,
            "layer {i} polygon area = {area}, expected 1.0 +- {AREA_TOLERANCE_MM2}",
        );
    }
}

#[test]
fn slice_leaf_output_is_deterministic_across_invocations() {
    let (scene, leaf_id) = scene_with_cube_root();
    let profile = eight_uniform_layers();
    let first = slice_leaf(&scene, leaf_id, &profile).expect("cube slices cleanly");
    let second = slice_leaf(&scene, leaf_id, &profile).expect("cube slices cleanly");
    assert_eq!(first, second);
}
