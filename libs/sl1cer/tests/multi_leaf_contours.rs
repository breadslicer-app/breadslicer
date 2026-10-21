/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end test of `slice_scene` on a three-model plate: a unit
//! cube next to the Z-tunnel and Y-tunnel prisms, at distinct XY
//! offsets and different heights. Checks per-leaf stack alignment,
//! empty layers above the short model, hole preservation, world
//! placement, and the layer-wise merge.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, SceneMetadata};
use sc_sl1cer::contour::{ContourStack, Polygon, merge_stacks};
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::{LeafSlices, slice_scene};

/// Tolerance for cross-section areas involving the tunnel prisms:
/// their 0.4 mm tunnel coordinates are not exactly representable in
/// the meshes' f32 vertices.
const PRISM_AREA_TOLERANCE_MM2: f64 = 1e-6;

/// Tolerance for the cube's cross-sections: every coordinate is
/// f32-exact, so only f64 interpolation noise remains.
const CUBE_AREA_TOLERANCE_MM2: f64 = 1e-9;

/// Eight uniform 0.25 mm slabs: planes at 0.125, 0.375, 0.625,
/// 0.875, 1.125, 1.375, 1.625, 1.875. The prisms span `z in [0, 2]`
/// (all planes inside); the lifted cube spans `z in [0, 1]` (first
/// four planes inside). The Y tunnel spans `z in [0.6, 1.4]`, so
/// planes 0.625 through 1.375 (indices 2..=5) cross it; no plane
/// lands exactly on a horizontal face.
const LAYER_COUNT: u32 = 8;

fn profile() -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(20.0, 20.0, 20.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        LayerSchedule::uniform(0.25, 0.25, LAYER_COUNT).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

fn named(name: &str) -> Option<SceneMetadata> {
    Some(SceneMetadata {
        name: Some(name.to_string()),
        ..Default::default()
    })
}

/// Cube at `x = -3` lifted onto the plate; Z-tunnel prism at the
/// origin; Y-tunnel prism at `x = +3`. All three are scene roots, in
/// this order.
fn three_model_plate() -> Scene {
    let mut scene = Scene::new();
    let cube = scene
        .add_leaf(
            named("cube"),
            Transform3D::Affine(AffineTransform3D::from_translation([-3.0, 0.0, 0.5])),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(cube).expect("cube becomes a root");
    let z_tunnel = scene
        .add_leaf(
            named("z-tunnel"),
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(cuboid_with_z_tunnel())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(z_tunnel).expect("prism becomes a root");
    let y_tunnel = scene
        .add_leaf(
            named("y-tunnel"),
            Transform3D::Affine(AffineTransform3D::from_translation([3.0, 0.0, 0.0])),
            LeafPayload::Mesh(Arc::new(cuboid_with_y_tunnel())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(y_tunnel).expect("prism becomes a root");
    scene
}

/// Enclosed area of a polygon: outer area plus (negative) hole areas.
fn net_area(polygon: &Polygon) -> f64 {
    let holes: f64 = polygon.holes().iter().map(|h| h.signed_area()).sum();
    polygon.outer().signed_area() + holes
}

fn slice_plate() -> Vec<LeafSlices> {
    slice_scene(&three_model_plate(), &profile()).expect("plate slices cleanly")
}

#[test]
fn per_leaf_stacks_share_one_layer_grid() {
    let slices = slice_plate();
    assert_eq!(slices.len(), 3);
    let names: Vec<Option<&str>> = slices.iter().map(|s| s.leaf_name()).collect();
    assert_eq!(
        names,
        vec![Some("cube"), Some("z-tunnel"), Some("y-tunnel")],
        "records preserve root discovery order",
    );
    for record in &slices {
        assert_eq!(record.stack().layers().len(), LAYER_COUNT as usize);
    }
    let reference: Vec<f64> = slices[0]
        .stack()
        .layers()
        .iter()
        .map(|layer| layer.z_mm())
        .collect();
    for record in &slices[1..] {
        let zs: Vec<f64> = record
            .stack()
            .layers()
            .iter()
            .map(|layer| layer.z_mm())
            .collect();
        assert_eq!(zs, reference, "shared schedule implies identical Z values");
    }
}

#[test]
fn short_cube_carries_empty_layers_above_its_top() {
    let slices = slice_plate();
    let cube = slices[0].stack();
    for (idx, layer) in cube.layers().iter().enumerate() {
        if idx < 4 {
            assert_eq!(layer.polygons().len(), 1, "layer {idx} is inside the cube");
            let polygon = &layer.polygons()[0];
            assert!(polygon.holes().is_empty());
            assert!(
                (net_area(polygon) - 1.0).abs() < CUBE_AREA_TOLERANCE_MM2,
                "layer {idx} area: {}",
                net_area(polygon),
            );
        } else {
            assert!(
                layer.polygons().is_empty(),
                "layer {idx} sits above the cube top",
            );
        }
    }
}

#[test]
fn cube_slices_at_its_world_offset() {
    let slices = slice_plate();
    let ring = slices[0].stack().layers()[0].polygons()[0].outer();
    let xs: Vec<f64> = ring.vertices().iter().map(|p| p.x).collect();
    let min_x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!((min_x - (-3.5)).abs() < 1e-6, "min_x: {min_x}");
    assert!((max_x - (-2.5)).abs() < 1e-6, "max_x: {max_x}");
}

#[test]
fn z_tunnel_prism_keeps_its_hole_on_every_layer() {
    let slices = slice_plate();
    let stack = slices[1].stack();
    for (idx, layer) in stack.layers().iter().enumerate() {
        assert_eq!(layer.polygons().len(), 1, "layer {idx}");
        let polygon = &layer.polygons()[0];
        assert_eq!(polygon.holes().len(), 1, "layer {idx} must keep the tunnel");
        assert!(
            (polygon.outer().signed_area() - 4.0).abs() < PRISM_AREA_TOLERANCE_MM2,
            "layer {idx} outer area: {}",
            polygon.outer().signed_area(),
        );
        assert!(
            (polygon.holes()[0].signed_area() - (-0.64)).abs() < PRISM_AREA_TOLERANCE_MM2,
            "layer {idx} hole area: {}",
            polygon.holes()[0].signed_area(),
        );
    }
}

#[test]
fn y_tunnel_prism_splits_in_two_across_the_tunnel_span() {
    let slices = slice_plate();
    let stack = slices[2].stack();
    for (idx, layer) in stack.layers().iter().enumerate() {
        let total: f64 = layer.polygons().iter().map(net_area).sum();
        if (2..=5).contains(&idx) {
            assert_eq!(
                layer.polygons().len(),
                2,
                "layer {idx} crosses the Y tunnel",
            );
            assert!(
                layer.polygons().iter().all(|p| p.holes().is_empty()),
                "the split halves are solid",
            );
            assert!(
                (total - 2.4).abs() < PRISM_AREA_TOLERANCE_MM2,
                "layer {idx} total area: {total}",
            );
        } else {
            assert_eq!(layer.polygons().len(), 1, "layer {idx} is solid");
            assert!(
                (total - 4.0).abs() < PRISM_AREA_TOLERANCE_MM2,
                "layer {idx} total area: {total}",
            );
        }
    }
}

#[test]
fn merged_plate_stack_concatenates_aligned_layers() {
    let slices = slice_plate();
    let stacks: Vec<ContourStack> = slices.iter().map(|s| s.stack().clone()).collect();
    let per_leaf_counts: Vec<Vec<usize>> = stacks
        .iter()
        .map(|stack| {
            stack
                .layers()
                .iter()
                .map(|layer| layer.polygons().len())
                .collect()
        })
        .collect();
    let merged = merge_stacks(stacks).expect("stacks share one schedule");

    assert_eq!(merged.layers().len(), LAYER_COUNT as usize);
    for (idx, layer) in merged.layers().iter().enumerate() {
        let expected: usize = per_leaf_counts.iter().map(|counts| counts[idx]).sum();
        assert_eq!(layer.polygons().len(), expected, "layer {idx}");
    }
    // Spot-check the composition: layers 2 and 3 carry all three
    // models (cube + holed prism + split prism = 1 + 1 + 2).
    assert_eq!(merged.layers()[2].polygons().len(), 4);
    // Above the cube, inside the tunnel span: 1 + 2.
    assert_eq!(merged.layers()[4].polygons().len(), 3);
    // Top layers: prisms only, tunnel closed again: 1 + 1.
    assert_eq!(merged.layers()[7].polygons().len(), 2);
}

#[test]
fn slice_scene_and_merge_are_deterministic_across_runs() {
    let a = slice_plate();
    let b = slice_plate();
    assert_eq!(a, b);
    let merged_a = merge_stacks(a.into_iter().map(LeafSlices::into_stack).collect()).unwrap();
    let merged_b = merge_stacks(b.into_iter().map(LeafSlices::into_stack).collect()).unwrap();
    assert_eq!(merged_a, merged_b);
}
