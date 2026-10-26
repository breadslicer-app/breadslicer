use std::sync::Arc;

use sc_mesh_core::transform::AffineTransform3D;
use sc_scene::LeafPayload;
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};

use super::*;

fn sphere_leaf_mesh(radius: f32) -> IndexedMesh {
    let s = hollow_df::shapes::icosphere(radius, 3);
    IndexedMesh::new(
        Some(MeshMetadata {
            name: Some("sphere".into()),
            ..Default::default()
        }),
        s.positions
            .iter()
            .map(|p| Vertex::new(p.to_array()))
            .collect(),
        s.triangles
            .iter()
            .map(|&t| IndexedTriangle::from_raw_indices(t))
            .collect(),
    )
}

fn scene_with(mesh: IndexedMesh, transform: Transform3D) -> (Scene, NodeId) {
    let mut scene = Scene::new();
    let id = scene
        .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
        .unwrap();
    (scene, id)
}

fn affine(linear: [[f64; 3]; 3], translation: [f64; 3]) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
        linear,
        translation,
    ))
}

fn run(scene: &Scene, leaf: NodeId, request: HollowRequest) -> HollowOutcome {
    start_hollow(scene, leaf, request).unwrap().join().unwrap()
}

fn as_engine(mesh: &IndexedMesh) -> hollow_df::IndexedMesh {
    engine_mesh(mesh, &LeafFrame::of(&Transform3D::Identity).unwrap())
}

#[test]
fn a_rigid_transform_has_no_stretch() {
    let rot = AffineTransform3D::from_axis_angle([0.3, 0.5, 0.8], 1.1).unwrap();
    let world = affine(*rot.linear(), [10.0, -4.0, 2.0]);
    let frame = LeafFrame::of(&world).unwrap();
    assert_eq!(frame.stretch, IDENTITY);
    for i in 0..3 {
        for j in 0..3 {
            assert!((frame.rotation[i][j] - rot.linear()[i][j]).abs() < 1e-12);
        }
    }
}

#[test]
fn a_scaled_transform_splits_into_rotation_and_stretch() {
    let rot = AffineTransform3D::from_axis_angle([1.0, 0.0, 1.0], 0.7).unwrap();
    let scale = AffineTransform3D::from_non_uniform_scale([2.0, 1.0, 0.5]);
    let m = mul(rot.linear(), scale.linear());
    let frame = LeafFrame::of(&affine(m, [0.0; 3])).unwrap();
    let rebuilt = mul(&frame.rotation, &frame.stretch);
    let r_rt = mul(&frame.rotation, &transpose(&frame.rotation));
    for i in 0..3 {
        for j in 0..3 {
            assert!((rebuilt[i][j] - m[i][j]).abs() < 1e-12, "M = R·P");
            assert!((r_rt[i][j] - IDENTITY[i][j]).abs() < 1e-12, "R orthogonal");
            assert!(
                (frame.stretch[i][j] - frame.stretch[j][i]).abs() < 1e-12,
                "P symmetric"
            );
        }
    }
}

#[test]
fn moving_and_turning_the_model_reuses_the_session() {
    let (mut scene, leaf) = scene_with(sphere_leaf_mesh(15.0), Transform3D::Identity);
    let first = run(&scene, leaf, HollowRequest::default());
    assert!(!first.reused_session);
    let rot = AffineTransform3D::from_axis_angle([0.0, 1.0, 1.0], 0.9).unwrap();
    scene
        .set_local_transform(leaf, affine(*rot.linear(), [30.0, 20.0, 15.0]))
        .unwrap();
    let second = run(
        &scene,
        leaf,
        HollowRequest {
            prepared: Some(first.prepared.clone()),
            ..HollowRequest::default()
        },
    );
    assert!(second.reused_session);
    let (HollowResult::Preview { cavity: a, .. }, HollowResult::Preview { cavity: b, .. }) =
        (&first.result, &second.result)
    else {
        panic!("previews");
    };
    assert_eq!(a, b, "the local-frame cavity does not depend on the pose");

    // A scale is a different physical object: a new session.
    scene
        .set_local_transform(
            leaf,
            Transform3D::Affine(AffineTransform3D::from_uniform_scale(1.2)),
        )
        .unwrap();
    let third = run(
        &scene,
        leaf,
        HollowRequest {
            prepared: Some(second.prepared.clone()),
            ..HollowRequest::default()
        },
    );
    assert!(!third.reused_session);
}

#[test]
fn the_wall_is_physical_under_a_non_uniform_scale() {
    // A sphere of radius 10 stretched to 40 x 20 x 20 mm; a 2 mm wall must
    // stay 2 mm on every axis (DragonFruit's mean-scale rule would give 1.5 mm
    // across Y and Z and 3 mm across X).
    let world = affine(
        [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [0.0; 3],
    );
    let (scene, leaf) = scene_with(sphere_leaf_mesh(10.0), world);
    let out = run(
        &scene,
        leaf,
        HollowRequest {
            grid: GridRequest {
                voxel_size_mm: 0.25,
                max_resolution: 192,
            },
            ..HollowRequest::default()
        },
    );
    let HollowResult::Preview { cavity, .. } = &out.result else {
        panic!("preview");
    };
    // Physically the cavity spans 40 - 4 along X and 20 - 4 along Y.
    let frame = LeafFrame::of(&world).unwrap();
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in cavity.vertices() {
        let p = frame.local_to_frame(*v.as_array());
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let span = [0, 1, 2].map(|k| max[k] - min[k]);
    assert!((span[0] - 36.0).abs() < 0.6, "x span {}", span[0]);
    assert!((span[1] - 16.0).abs() < 0.6, "y span {}", span[1]);
    assert!((span[2] - 16.0).abs() < 0.6, "z span {}", span[2]);
}

fn profile(layers: u32) -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(100.0, 100.0, 100.0).unwrap(),
        PixelResolution::new(1000, 1000).unwrap(),
        LayerSchedule::uniform(0.5, 0.5, layers).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

/// Holes in the layers of `mesh` (world) sliced at mid height.
fn holes_at_mid_height(mesh: IndexedMesh, z_mid: f64) -> (usize, usize) {
    let (scene, leaf) = scene_with(mesh, Transform3D::Identity);
    let stack = sc_sl1cer::slice_leaf(&scene, leaf, &profile((2.0 * z_mid / 0.5) as u32)).unwrap();
    let layer = stack
        .layers()
        .iter()
        .min_by(|a, b| {
            (a.z_mm() - z_mid)
                .abs()
                .total_cmp(&(b.z_mm() - z_mid).abs())
        })
        .unwrap();
    let holes = layer.polygons().iter().map(|p| p.holes().len()).sum();
    (layer.polygons().len(), holes)
}

#[test]
fn an_applied_hollow_with_a_drain_slices_with_a_hole() {
    // Sphere of radius 15 sitting on the plate at (50, 50).
    let (scene, leaf) = scene_with(sphere_leaf_mesh(15.0), affine(IDENTITY, [50.0, 50.0, 15.0]));
    let out = run(
        &scene,
        leaf,
        HollowRequest {
            kind: HollowKind::Apply,
            drains: vec![WorldDrain::new([50.0, 50.0, 0.0], [0.0, 0.0, 1.0], 1.5)],
            ..HollowRequest::default()
        },
    );
    let HollowResult::Apply { mesh, drains } = &out.result else {
        panic!("apply");
    };
    assert!(drains[0].reaches_cavity, "{drains:?}");
    assert_eq!(
        mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
        Some("sphere")
    );
    assert!(hollow_df::topology::is_edge_manifold(&as_engine(mesh)));
    let removed = out.stats.removed_volume_mm3.unwrap();
    assert!(
        removed > 0.5 * out.stats.source_volume_mm3,
        "{:?}",
        out.stats
    );

    let world = bake_mesh_to_world_of(&scene, leaf, mesh);
    let (polygons, holes) = holes_at_mid_height(world, 15.0);
    assert_eq!((polygons, holes), (1, 1), "a ring: wall around the cavity");
}

/// `mesh` (a leaf's local mesh) under that leaf's world transform.
fn bake_mesh_to_world_of(scene: &Scene, leaf: NodeId, mesh: &IndexedMesh) -> IndexedMesh {
    let world = scene.world_transform_of(leaf).unwrap();
    sc_scene::bake_mesh_to_world(mesh, &world).unwrap()
}

#[test]
fn an_undrained_hollow_slices_with_a_hole() {
    // The cavity is its own (inward) shell when no drain joins it to the
    // outside. The slicer groups contours per shell, so this checks it
    // still reads the cavity as a hole rather than as a second solid.
    let (scene, leaf) = scene_with(sphere_leaf_mesh(15.0), affine(IDENTITY, [50.0, 50.0, 15.0]));
    let out = run(
        &scene,
        leaf,
        HollowRequest {
            kind: HollowKind::Apply,
            ..HollowRequest::default()
        },
    );
    let HollowResult::Apply { mesh, .. } = &out.result else {
        panic!("apply");
    };
    let world = bake_mesh_to_world_of(&scene, leaf, mesh);
    let (polygons, holes) = holes_at_mid_height(world, 15.0);
    assert_eq!(
        (polygons, holes),
        (1, 1),
        "the cavity must be a hole, not a second solid"
    );
}

#[test]
fn a_cancelled_job_reports_cancelled() {
    let (scene, leaf) = scene_with(sphere_leaf_mesh(15.0), Transform3D::Identity);
    let handle = start_hollow(
        &scene,
        leaf,
        HollowRequest {
            grid: GridRequest {
                voxel_size_mm: 0.1,
                max_resolution: 400,
            },
            ..HollowRequest::default()
        },
    )
    .unwrap();
    handle.cancel();
    assert!(matches!(handle.join(), Err(JobError::Cancelled)));
}
