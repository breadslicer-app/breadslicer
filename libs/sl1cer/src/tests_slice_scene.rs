/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-level tests for [`slice_scene`]: leaf discovery, transform
//! composition, deferred node kinds, and aggregated validation
//! reporting. Geometric multi-leaf assertions live in the
//! `multi_leaf_contours` integration test.

use std::sync::Arc;

use sc_mesh_core::scene::node::{CsgOp, LeafPayload, NodeId, SceneMetadata};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Scene, Vertex};
use sc_mesh_fixtures::unit_cube_mesh;

use crate::error::{InvalidLeafGeometry, SliceSceneError};
use crate::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use crate::{
    CancelToken, SliceOutcome, SliceProgress, slice_leaf, slice_scene, slice_scene_with_progress,
    sliceable_leaves,
};

/// Four uniform 0.25 mm slabs: planes at 0.125, 0.375, 0.625, 0.875.
/// All four fall inside a unit cube lifted to `z in [0, 1]`.
fn four_layer_profile() -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(20.0, 20.0, 20.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        LayerSchedule::uniform(0.25, 0.25, 4).unwrap(),
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

/// Translation lifting the origin-centred unit cube onto the plate
/// (`z in [0, 1]`) at the given XY offset.
fn lift(x: f64, y: f64) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation([x, y, 0.5]))
}

fn add_root_cube(scene: &mut Scene, name: &str, x: f64) -> NodeId {
    let leaf = scene
        .add_leaf(
            named(name),
            lift(x, 0.0),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
    leaf
}

/// Single triangle referencing vertex 0 twice: rejected by the
/// face-validation stage with `ZeroLengthEdge`.
fn zero_length_edge_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 0],
        )],
    }
}

/// Single well-formed triangle: passes face validation, fails the
/// manifold stage with boundary edges.
fn open_triangle_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 2],
        )],
    }
}

fn add_root_mesh(scene: &mut Scene, name: &str, mesh: IndexedMesh) -> NodeId {
    let leaf = scene
        .add_leaf(
            named(name),
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
    leaf
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    #[test]
    fn root_leaves_slice_in_root_order_with_names() {
        let mut scene = Scene::new();
        let a = add_root_cube(&mut scene, "alpha", -3.0);
        let b = add_root_cube(&mut scene, "beta", 3.0);

        let slices = slice_scene(&scene, &four_layer_profile()).expect("both cubes slice");
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].leaf(), a);
        assert_eq!(slices[0].leaf_name(), Some("alpha"));
        assert_eq!(slices[1].leaf(), b);
        assert_eq!(slices[1].leaf_name(), Some("beta"));
        for record in &slices {
            assert_eq!(record.stack().layers().len(), 4);
            assert!(
                record
                    .stack()
                    .layers()
                    .iter()
                    .all(|layer| layer.polygons().len() == 1),
                "every plane crosses the cube interior",
            );
        }
    }

    #[test]
    fn leaf_under_a_translated_group_slices_at_the_composed_position() {
        let mut scene = Scene::new();
        let group = scene.add_group(None, lift(5.0, 0.0));
        let leaf = scene
            .add_leaf(
                named("nested"),
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        scene.add_child(group, leaf).expect("attach leaf to group");
        scene.add_root(group).expect("group becomes the root");

        let slices = slice_scene(&scene, &four_layer_profile()).expect("nested cube slices");
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].leaf(), leaf);
        let ring = slices[0].stack().layers()[0].polygons()[0].outer().clone();
        let xs: Vec<f64> = ring.vertices().iter().map(|p| p.x).collect();
        let min_x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!((min_x - 4.5).abs() < 1e-6, "min_x: {min_x}");
        assert!((max_x - 5.5).abs() < 1e-6, "max_x: {max_x}");
    }

    #[test]
    fn leaf_name_falls_back_to_the_mesh_metadata_name() {
        // OBJ-shaped topology: nameless group root, nameless leaf,
        // object name carried on the mesh itself.
        let mut scene = Scene::new();
        let group = scene.add_group(None, Transform3D::Identity);
        let mut mesh = unit_cube_mesh();
        mesh.meta = Some(sc_mesh_core::indexed_mesh::MeshMetadata {
            name: Some("from-the-mesh".to_string()),
            ..Default::default()
        });
        let leaf = scene
            .add_leaf(None, lift(0.0, 0.0), LeafPayload::Mesh(Arc::new(mesh)))
            .expect("inline-mesh leaf");
        scene.add_child(group, leaf).expect("attach leaf to group");
        scene.add_root(group).expect("group becomes the root");

        let slices = slice_scene(&scene, &four_layer_profile()).expect("cube slices");
        assert_eq!(slices[0].leaf_name(), Some("from-the-mesh"));
    }

    #[test]
    fn empty_scene_is_rejected() {
        let scene = Scene::new();
        let err = slice_scene(&scene, &four_layer_profile()).unwrap_err();
        assert!(matches!(err, SliceSceneError::EmptyScene), "err: {err:?}");
    }

    #[test]
    fn scene_with_only_an_empty_group_is_rejected() {
        let mut scene = Scene::new();
        let group = scene.add_group(None, Transform3D::Identity);
        scene.add_root(group).expect("group becomes the root");
        let err = slice_scene(&scene, &four_layer_profile()).unwrap_err();
        assert!(matches!(err, SliceSceneError::EmptyScene), "err: {err:?}");
    }

    #[test]
    fn csg_node_anywhere_rejects_the_scene() {
        let mut scene = Scene::new();
        add_root_cube(&mut scene, "plain", -3.0);
        let operand = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        let csg = scene
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![operand])
            .expect("csg over one operand");
        scene.add_root(csg).expect("csg becomes a root");

        let err = slice_scene(&scene, &four_layer_profile()).unwrap_err();
        assert!(
            matches!(err, SliceSceneError::CsgNotYetSupported { node_id } if node_id == csg),
            "err: {err:?}",
        );
    }

    #[test]
    fn instance_node_rejects_the_scene() {
        let mut scene = Scene::new();
        let template = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        let resource = scene
            .register_subtree_resource(template)
            .expect("parentless non-root leaf registers");
        let instance = scene
            .add_instance(None, Transform3D::Identity, resource)
            .expect("instance of a live resource");
        scene.add_root(instance).expect("instance becomes a root");

        let err = slice_scene(&scene, &four_layer_profile()).unwrap_err();
        assert!(
            matches!(
                err,
                SliceSceneError::InstanceNotYetSupported { node_id } if node_id == instance,
            ),
            "err: {err:?}",
        );
    }
}

#[cfg(test)]
mod validation_aggregation_tests {
    use super::*;

    #[test]
    fn every_invalid_leaf_is_reported_in_discovery_order() {
        let mut scene = Scene::new();
        add_root_cube(&mut scene, "good", -3.0);
        let bad_faces = add_root_mesh(&mut scene, "bad-faces", zero_length_edge_mesh());
        let bad_manifold = add_root_mesh(&mut scene, "bad-manifold", open_triangle_mesh());

        let err = slice_scene(&scene, &four_layer_profile()).unwrap_err();
        let SliceSceneError::LeavesValidationFailed(failures) = err else {
            panic!("expected LeavesValidationFailed, got {err:?}");
        };
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].leaf, bad_faces);
        assert_eq!(failures[0].leaf_name.as_deref(), Some("bad-faces"));
        assert!(matches!(
            failures[0].geometry,
            InvalidLeafGeometry::Faces(_),
        ));
        assert_eq!(failures[1].leaf, bad_manifold);
        assert_eq!(failures[1].leaf_name.as_deref(), Some("bad-manifold"));
        assert!(matches!(
            failures[1].geometry,
            InvalidLeafGeometry::Manifold(_),
        ));
    }

    #[test]
    fn leaves_after_the_first_failure_are_still_validated() {
        // The invalid leaf comes FIRST; the last leaf is also invalid.
        // Aggregation must reach it even though slicing stopped.
        let mut scene = Scene::new();
        let first_bad = add_root_mesh(&mut scene, "first-bad", open_triangle_mesh());
        add_root_cube(&mut scene, "good", -3.0);
        let last_bad = add_root_mesh(&mut scene, "last-bad", zero_length_edge_mesh());

        let err = slice_scene(&scene, &four_layer_profile()).unwrap_err();
        let SliceSceneError::LeavesValidationFailed(failures) = err else {
            panic!("expected LeavesValidationFailed, got {err:?}");
        };
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].leaf, first_bad);
        assert_eq!(failures[1].leaf, last_bad);
    }
}

#[cfg(test)]
mod equivalence_tests {
    use super::*;

    #[test]
    fn single_leaf_scene_matches_slice_leaf_output() {
        let mut scene = Scene::new();
        let leaf = add_root_cube(&mut scene, "solo", 0.0);
        let profile = four_layer_profile();

        let via_scene = slice_scene(&scene, &profile).expect("cube slices");
        let via_leaf = slice_leaf(&scene, leaf, &profile).expect("cube slices");
        assert_eq!(via_scene.len(), 1);
        assert_eq!(via_scene[0].stack(), &via_leaf);
    }

    #[test]
    fn slice_scene_is_deterministic_across_runs() {
        let mut scene = Scene::new();
        add_root_cube(&mut scene, "alpha", -3.0);
        add_root_cube(&mut scene, "beta", 3.0);
        let profile = four_layer_profile();

        let a = slice_scene(&scene, &profile).unwrap();
        let b = slice_scene(&scene, &profile).unwrap();
        assert_eq!(a, b);
    }
}

#[cfg(test)]
mod progress_and_cancel_tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Two cubes under a four-layer schedule: eight layer-slices total.
    fn two_cube_scene() -> Scene {
        let mut scene = Scene::new();
        add_root_cube(&mut scene, "alpha", -3.0);
        add_root_cube(&mut scene, "beta", 3.0);
        scene
    }

    #[test]
    fn progress_reports_each_layer_up_to_the_total() {
        let scene = two_cube_scene();
        let reports = Mutex::new(Vec::new());
        let outcome = slice_scene_with_progress(
            &scene,
            &four_layer_profile(),
            &CancelToken::new(),
            |progress| {
                reports.lock().unwrap().push(progress);
            },
        )
        .expect("both cubes slice");

        assert!(matches!(outcome, SliceOutcome::Completed(_)));
        let reports = reports.into_inner().unwrap();
        // 2 leaves * 4 layers.
        assert_eq!(reports.len(), 8);
        assert!(reports.iter().all(|p| p.total == 8));
        // Layers slice in parallel, so reports can arrive out of
        // order; the counts over a completed run are a permutation
        // of 1..=total.
        let mut counts: Vec<usize> = reports.iter().map(|p| p.completed).collect();
        counts.sort_unstable();
        assert_eq!(counts, (1..=8).collect::<Vec<_>>());
    }

    #[test]
    fn a_completed_run_matches_slice_scene() {
        let scene = two_cube_scene();
        let profile = four_layer_profile();
        let expected = slice_scene(&scene, &profile).expect("cubes slice");

        let outcome =
            slice_scene_with_progress(&scene, &profile, &CancelToken::new(), |_| {}).unwrap();
        assert_eq!(outcome, SliceOutcome::Completed(expected));
    }

    #[test]
    fn cancelling_after_the_first_layer_stops_early() {
        let scene = two_cube_scene();
        let cancel = CancelToken::new();
        let calls = AtomicUsize::new(0);

        let outcome = slice_scene_with_progress(&scene, &four_layer_profile(), &cancel, |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            cancel.cancel();
        })
        .expect("cancellation is a clean stop, not an error");

        assert_eq!(outcome, SliceOutcome::Cancelled);
        // The first report cancels the token, but the first leaf's
        // layer tasks run in parallel: any of its 4 layers may already
        // be past the cancellation poll and still report. The second
        // leaf never starts (the poll between leaves sees the trip),
        // so between 1 and 4 reports arrive.
        let calls = calls.into_inner();
        assert!((1..=4).contains(&calls), "reports: {calls}");
    }

    #[test]
    fn a_pre_cancelled_token_slices_nothing() {
        let scene = two_cube_scene();
        let cancel = CancelToken::new();
        cancel.cancel();
        let calls = AtomicUsize::new(0);

        let outcome = slice_scene_with_progress(&scene, &four_layer_profile(), &cancel, |_| {
            calls.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();

        assert_eq!(outcome, SliceOutcome::Cancelled);
        assert_eq!(calls.into_inner(), 0);
    }

    #[test]
    fn a_validation_failure_still_surfaces_through_the_progress_api() {
        let mut scene = Scene::new();
        add_root_cube(&mut scene, "good", -3.0);
        add_root_mesh(&mut scene, "bad", open_triangle_mesh());

        let err =
            slice_scene_with_progress(&scene, &four_layer_profile(), &CancelToken::new(), |_| {})
                .unwrap_err();
        assert!(
            matches!(err, SliceSceneError::LeavesValidationFailed(_)),
            "err: {err:?}",
        );
    }

    #[test]
    fn fraction_is_zero_when_there_is_no_work() {
        assert_eq!(
            SliceProgress {
                completed: 0,
                total: 0,
            }
            .fraction(),
            0.0,
        );
    }

    #[test]
    fn fraction_reports_the_completed_ratio() {
        assert_eq!(
            SliceProgress {
                completed: 2,
                total: 8,
            }
            .fraction(),
            0.25,
        );
    }
}

#[cfg(test)]
mod parallel_equivalence_tests {
    use sc_mesh_core::indexed_mesh::MeshTopology;
    use sc_mesh_core::scene::bake::bake_leaf_to_world;
    use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel};

    use crate::api::slice_layer;
    use crate::contour::{ContourLayer, ContourStack};

    use super::*;

    /// 64 uniform 0.03125 mm slabs: plane centres from 0.015625 up to
    /// 1.984375, covering `z in [0, 2]`. Enough tasks to occupy
    /// several worker threads.
    fn sixty_four_layer_profile() -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(20.0, 20.0, 20.0).unwrap(),
            PixelResolution::new(100, 100).unwrap(),
            LayerSchedule::uniform(0.03125, 0.03125, 64).unwrap(),
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        )
    }

    /// The reference result: the leaf sliced by a plain sequential
    /// loop over [`slice_layer`], one plane after another on this
    /// thread, with every face as a candidate (a full scan, no
    /// bucketing). The parallel bucketed pipeline must reproduce it
    /// exactly.
    fn expected_sequential_stack(
        scene: &Scene,
        leaf: NodeId,
        profile: &SliceProfile,
    ) -> ContourStack {
        let baked = bake_leaf_to_world(scene, leaf).expect("leaf bakes");
        let faces = baked.validated_faces().expect("faces validate");
        let mesh = faces.validated_manifold().expect("mesh is manifold");
        let topology = MeshTopology::from_manifold(&mesh);
        let all_faces: Vec<u32> = (0..mesh.faces().len() as u32).collect();
        let layers: Vec<ContourLayer> = profile
            .layer_schedule()
            .iter_z()
            .map(|z_mm| slice_layer(&mesh, z_mm, &topology, &all_faces).expect("layer slices"))
            .collect();
        ContourStack::new_unchecked(layers)
    }

    #[test]
    fn parallel_scene_matches_a_sequential_layer_pass() {
        let mut scene = Scene::new();
        let cube = add_root_cube(&mut scene, "cube", -3.0);
        let z_tunnel = add_root_mesh(&mut scene, "z-tunnel", cuboid_with_z_tunnel());
        let y_tunnel = add_root_mesh(&mut scene, "y-tunnel", cuboid_with_y_tunnel());
        let profile = sixty_four_layer_profile();

        let slices = slice_scene(&scene, &profile).expect("all three leaves slice");

        assert_eq!(slices.len(), 3);
        for (record, leaf) in slices.iter().zip([cube, z_tunnel, y_tunnel]) {
            assert_eq!(record.leaf(), leaf);
            let expected = expected_sequential_stack(&scene, leaf, &profile);
            assert_eq!(record.stack(), &expected);
        }

        // Guard against a vacuously-empty comparison: the z-tunnel
        // slices to an annulus (outer ring plus one hole) on every
        // layer, and the y-tunnel splits into two solid polygons
        // inside its tunnel band.
        let z_first = &slices[1].stack().layers()[0];
        assert_eq!(z_first.polygons().len(), 1);
        assert_eq!(z_first.polygons()[0].holes().len(), 1);
        let y_mid = &slices[2].stack().layers()[32];
        assert_eq!(y_mid.polygons().len(), 2);
    }

    #[test]
    fn slice_leaf_matches_a_sequential_layer_pass() {
        let mut scene = Scene::new();
        let leaf = add_root_mesh(&mut scene, "y-tunnel", cuboid_with_y_tunnel());
        let profile = sixty_four_layer_profile();

        let stack = slice_leaf(&scene, leaf, &profile).expect("leaf slices");

        assert_eq!(stack, expected_sequential_stack(&scene, leaf, &profile));
    }
}

#[cfg(test)]
mod sliceable_leaves_tests {
    use super::*;

    #[test]
    fn returns_root_leaves_in_root_order() {
        let mut scene = Scene::new();
        let a = add_root_cube(&mut scene, "alpha", -3.0);
        let b = add_root_cube(&mut scene, "beta", 3.0);
        assert_eq!(sliceable_leaves(&scene).expect("two leaves"), vec![a, b]);
    }

    #[test]
    fn enters_groups_to_reach_nested_leaves() {
        let mut scene = Scene::new();
        let group = scene.add_group(None, lift(5.0, 0.0));
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        scene.add_child(group, leaf).expect("attach leaf to group");
        scene.add_root(group).expect("group becomes the root");
        assert_eq!(
            sliceable_leaves(&scene).expect("one nested leaf"),
            vec![leaf]
        );
    }

    #[test]
    fn order_matches_slice_scene_discovery() {
        let mut scene = Scene::new();
        add_root_cube(&mut scene, "alpha", -3.0);
        add_root_cube(&mut scene, "beta", 3.0);
        let sliced: Vec<NodeId> = slice_scene(&scene, &four_layer_profile())
            .expect("both cubes slice")
            .iter()
            .map(|record| record.leaf())
            .collect();
        assert_eq!(sliceable_leaves(&scene).expect("two leaves"), sliced);
    }

    #[test]
    fn empty_scene_is_rejected() {
        let scene = Scene::new();
        assert!(matches!(
            sliceable_leaves(&scene).unwrap_err(),
            SliceSceneError::EmptyScene,
        ));
    }

    #[test]
    fn csg_node_aborts_the_walk() {
        let mut scene = Scene::new();
        add_root_cube(&mut scene, "plain", -3.0);
        let operand = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        let csg = scene
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![operand])
            .expect("csg over one operand");
        scene.add_root(csg).expect("csg becomes a root");
        assert!(matches!(
            sliceable_leaves(&scene).unwrap_err(),
            SliceSceneError::CsgNotYetSupported { node_id } if node_id == csg,
        ));
    }

    #[test]
    fn instance_node_aborts_the_walk() {
        let mut scene = Scene::new();
        let template = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        let resource = scene
            .register_subtree_resource(template)
            .expect("parentless non-root leaf registers");
        let instance = scene
            .add_instance(None, Transform3D::Identity, resource)
            .expect("instance of a live resource");
        scene.add_root(instance).expect("instance becomes a root");
        assert!(matches!(
            sliceable_leaves(&scene).unwrap_err(),
            SliceSceneError::InstanceNotYetSupported { node_id } if node_id == instance,
        ));
    }
}

#[cfg(test)]
mod pinched_leaf_tests {
    use super::*;

    use sc_mesh_fixtures::{bowtie_tetrahedra, cuboid_with_pinched_z_tunnel};

    /// One 2 mm slab covering the whole fixture: its plane sits at
    /// exactly z = 1, the pinch height of both pinch fixtures.
    fn through_pinch_profile() -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(20.0, 20.0, 20.0).unwrap(),
            PixelResolution::new(100, 100).unwrap(),
            LayerSchedule::uniform(2.0, 2.0, 1).unwrap(),
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        )
    }

    /// Eight 0.25 mm slabs: planes at 0.125 .. 1.875, straddling the
    /// pinch at z = 1 without ever landing on it.
    fn eight_layer_profile() -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(20.0, 20.0, 20.0).unwrap(),
            PixelResolution::new(100, 100).unwrap(),
            LayerSchedule::uniform(0.25, 0.25, 8).unwrap(),
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        )
    }

    #[test]
    fn bowtie_leaf_slices_end_to_end_through_the_pinch() {
        // Regression: this exact call used to panic inside the
        // topology build before pinch vertices were accepted.
        let mut scene = Scene::new();
        let leaf = add_root_mesh(&mut scene, "bowtie", bowtie_tetrahedra());
        let stack =
            slice_leaf(&scene, leaf, &through_pinch_profile()).expect("pinched leaf slices");
        assert_eq!(stack.layers().len(), 1);
        assert!(
            stack.layers()[0].polygons().is_empty(),
            "the through-pinch plane meets the bowtie in a single point"
        );
    }

    #[test]
    fn bowtie_leaf_slices_one_lobe_polygon_per_off_pinch_layer() {
        let mut scene = Scene::new();
        let leaf = add_root_mesh(&mut scene, "bowtie", bowtie_tetrahedra());
        let stack = slice_leaf(&scene, leaf, &eight_layer_profile()).expect("pinched leaf slices");
        assert_eq!(stack.layers().len(), 8);
        for layer in stack.layers() {
            assert_eq!(
                layer.polygons().len(),
                1,
                "layer {} expected exactly one lobe cross-section",
                layer.z_mm()
            );
            assert!(layer.polygons()[0].holes().is_empty());
        }
    }

    #[test]
    fn pinched_tunnel_leaf_slices_end_to_end_through_the_pinch() {
        let mut scene = Scene::new();
        let leaf = add_root_mesh(&mut scene, "pinched tunnel", cuboid_with_pinched_z_tunnel());
        let stack =
            slice_leaf(&scene, leaf, &through_pinch_profile()).expect("pinched leaf slices");
        assert_eq!(stack.layers().len(), 1);
        let polygons = stack.layers()[0].polygons();
        assert_eq!(polygons.len(), 1);
        assert!(
            polygons[0].holes().is_empty(),
            "the tunnel hole degenerates to the pinch point on this plane"
        );
    }

    #[test]
    fn pinched_tunnel_leaf_keeps_its_hole_on_every_off_pinch_layer() {
        let mut scene = Scene::new();
        let leaf = add_root_mesh(&mut scene, "pinched tunnel", cuboid_with_pinched_z_tunnel());
        let stack = slice_leaf(&scene, leaf, &eight_layer_profile()).expect("pinched leaf slices");
        assert_eq!(stack.layers().len(), 8);
        for layer in stack.layers() {
            assert_eq!(layer.polygons().len(), 1);
            assert_eq!(
                layer.polygons()[0].holes().len(),
                1,
                "layer {} expected the shrunk tunnel hole",
                layer.z_mm()
            );
        }
    }
}
