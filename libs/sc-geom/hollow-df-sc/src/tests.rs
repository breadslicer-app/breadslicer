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
                ..GridRequest::default()
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

/// The hollow a model gets carries the cavity its preview showed, in the
/// leaf's own frame, vertex for vertex (#29), also under a non-uniform
/// scale and through the session the preview left (as the Hollow tab runs
/// them).
#[test]
fn the_applied_hollow_carries_the_previewed_cavity() {
    let world = affine(
        [[1.6, 0.0, 0.2], [0.0, 1.0, 0.0], [0.0, 0.0, 0.8]],
        [5.0, -3.0, 12.0],
    );
    let (scene, leaf) = scene_with(sphere_leaf_mesh(12.0), world);
    let preview = run(&scene, leaf, HollowRequest::default());
    let HollowResult::Preview { cavity, .. } = &preview.result else {
        panic!("preview");
    };
    let applied = run(
        &scene,
        leaf,
        HollowRequest {
            kind: HollowKind::Apply,
            prepared: Some(preview.prepared.clone()),
            ..HollowRequest::default()
        },
    );
    assert!(applied.reused_session);
    let HollowResult::Apply { mesh, .. } = &applied.result else {
        panic!("apply");
    };
    let corners = |m: &IndexedMesh, f: &IndexedTriangle| {
        f.vertices
            .map(|v| m.vertices()[usize::from(v)].as_array().map(f32::to_bits))
    };
    let hollow: std::collections::HashSet<[[u32; 3]; 3]> =
        mesh.faces().iter().map(|f| corners(mesh, f)).collect();
    assert!(!cavity.faces().is_empty());
    for f in cavity.faces() {
        let [a, b, c] = corners(cavity, f);
        assert!(
            [[a, b, c], [b, c, a], [c, a, b]]
                .iter()
                .any(|t| hollow.contains(t)),
            "a previewed cavity triangle is not in the hollow"
        );
    }
    assert_eq!(
        mesh.faces().len(),
        sphere_leaf_mesh(12.0).faces().len() + cavity.faces().len()
    );
}

#[test]
fn a_memory_limited_hollow_keeps_the_previewed_voxel() {
    let (scene, leaf) = scene_with(sphere_leaf_mesh(12.0), Transform3D::Identity);
    let grid = GridRequest {
        voxel_size_mm: 0.1,
        max_memory_bytes: 200_000 * hollow_df::engine::BYTES_PER_CELL,
        ..GridRequest::default()
    };
    let preview = run(
        &scene,
        leaf,
        HollowRequest {
            grid,
            ..HollowRequest::default()
        },
    );
    assert!(matches!(
        preview.stats.grid.bound,
        Some(hollow_df::ResolutionBound::MemoryLimited { .. })
    ));
    let applied = run(
        &scene,
        leaf,
        HollowRequest {
            kind: HollowKind::Apply,
            grid,
            prepared: Some(preview.prepared.clone()),
            ..HollowRequest::default()
        },
    );
    assert!(applied.reused_session);
    assert_eq!(applied.stats.grid, preview.stats.grid);
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

/// With infill, drains that open are cut (#321): the result is one the
/// slicer takes, and the drain from below is a hole in the floor.
#[test]
fn an_applied_hollow_with_infill_and_drains_slices() {
    use hollow_df::{HollowFill, InfillMode};
    let (scene, leaf) = scene_with(sphere_leaf_mesh(15.0), affine(IDENTITY, [50.0, 50.0, 15.0]));
    for pattern in [InfillMode::Lattice, InfillMode::Pillar] {
        let out = run(
            &scene,
            leaf,
            HollowRequest {
                kind: HollowKind::Apply,
                settings: HollowSettings {
                    fill: HollowFill::Infill {
                        pattern,
                        cell_mm: 6.0,
                        beam_radius_mm: 0.6,
                    },
                    ..HollowSettings::default()
                },
                drains: vec![
                    WorldDrain::new([50.0, 50.0, 0.0], [0.0, 0.0, 1.0], 1.5),
                    WorldDrain::new([35.0, 50.0, 15.0], [1.0, 0.0, 0.0], 1.0),
                    WorldDrain::new([50.0, 65.0, 15.0], [0.0, -1.0, 0.0], 1.0),
                ],
                ..HollowRequest::default()
            },
        );
        let HollowResult::Apply { mesh, drains } = &out.result else {
            panic!("apply");
        };
        assert!(drains.iter().all(|d| d.reaches_cavity), "{drains:?}");
        assert!(mesh.is_sliceable(), "{pattern:?}");
        let world = bake_mesh_to_world_of(&scene, leaf, mesh);
        let (polygons, holes) = holes_at_mid_height(world, 1.0);
        assert_eq!(
            (polygons, holes),
            (1, 1),
            "{pattern:?}: the drain in the floor"
        );
    }
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
                ..GridRequest::default()
            },
            ..HollowRequest::default()
        },
    )
    .unwrap();
    handle.cancel();
    assert!(matches!(handle.join(), Err(JobError::Cancelled)));
}

/// Every triangle of `part` is in `whole`, corner for corner.
fn has_every_triangle(whole: &IndexedMesh, part: &IndexedMesh) -> bool {
    let key = |mesh: &IndexedMesh, f: &IndexedTriangle| -> [u32; 9] {
        let corners = f.vertices.map(|v| {
            mesh.vertices()[u32::from(v) as usize]
                .as_array()
                .map(f32::to_bits)
        });
        let first = (0..3).min_by_key(|&k| corners[k]).unwrap();
        std::array::from_fn(|i| corners[(first + i / 3) % 3][i % 3])
    };
    let all: std::collections::HashSet<[u32; 9]> =
        whole.faces().iter().map(|f| key(whole, f)).collect();
    part.faces().iter().all(|f| all.contains(&key(part, f)))
}

/// #339 through the adapter: a preview with the model's drains shows the
/// infill with their channels cleared, exactly as the apply carries it.
#[test]
fn a_preview_with_drains_shows_the_infill_the_hollow_gets() {
    use hollow_df::{HollowFill, InfillMode};
    let (scene, leaf) = scene_with(sphere_leaf_mesh(15.0), affine(IDENTITY, [50.0, 50.0, 15.0]));
    for pattern in [InfillMode::Lattice, InfillMode::Pillar] {
        let settings = HollowSettings {
            fill: HollowFill::Infill {
                pattern,
                cell_mm: 6.0,
                beam_radius_mm: 0.6,
            },
            ..HollowSettings::default()
        };
        let plain = run(
            &scene,
            leaf,
            HollowRequest {
                settings: settings.clone(),
                ..HollowRequest::default()
            },
        );
        let HollowResult::Preview { infill, .. } = &plain.result else {
            panic!("preview");
        };
        // Straight down onto the highest beam end (local frame: the sphere
        // is centred on the origin).
        let top = infill
            .as_ref()
            .unwrap()
            .vertices()
            .iter()
            .map(|v| *v.as_array())
            .max_by(|a, b| a[2].total_cmp(&b[2]))
            .unwrap();
        let surface = (225.0 - top[0] * top[0] - top[1] * top[1]).sqrt();
        let request = HollowRequest {
            settings,
            drains: vec![
                WorldDrain::new(
                    [50.0 + top[0], 50.0 + top[1], 15.0 + surface],
                    [0.0, 0.0, -1.0],
                    1.5,
                ),
                WorldDrain::new([35.0, 50.0, 15.0], [1.0, 0.0, 0.0], 1.0),
            ],
            ..HollowRequest::default()
        };
        let shown = run(&scene, leaf, request.clone());
        let applied = run(
            &scene,
            leaf,
            HollowRequest {
                kind: HollowKind::Apply,
                ..request
            },
        );
        let infill = |out: &HollowOutcome| match &out.result {
            HollowResult::Preview { infill, .. } => infill.clone().unwrap(),
            HollowResult::Apply { .. } => panic!("preview"),
        };
        let (whole, cleared) = (infill(&plain), infill(&shown));
        assert!(
            cleared != whole,
            "{pattern:?}: the drains clear some infill"
        );
        let HollowResult::Apply { mesh, .. } = &applied.result else {
            panic!("apply");
        };
        assert!(mesh.is_sliceable(), "{pattern:?}");
        assert!(has_every_triangle(mesh, &cleared), "{pattern:?}");
    }
}

/// #330 through the adapter: on a fin 3 mm wide, drains whose cutter
/// corner lies on the diagonal of the fin's top give a mesh the slicer
/// takes, and the top of the fin is open at both.
#[test]
fn drains_on_a_face_diagonal_slice() {
    let outline: [(f32, f32); 8] = [
        (0.0, 0.0),
        (40.0, 0.0),
        (40.0, 30.0),
        (21.5, 30.0),
        (21.5, 50.0),
        (18.5, 50.0),
        (18.5, 30.0),
        (0.0, 30.0),
    ];
    let n = outline.len() as u32;
    let mut vertices = Vec::new();
    for y in [0.0, 30.0] {
        vertices.extend(outline.iter().map(|&(x, z)| Vertex::new([x, y, z])));
    }
    let mut faces = Vec::new();
    for [a, b, c] in [
        [0, 1, 2],
        [0, 2, 3],
        [6, 3, 4],
        [6, 4, 5],
        [0, 3, 6],
        [0, 6, 7],
    ] {
        faces.push(IndexedTriangle::from_raw_indices([a, b, c]));
        faces.push(IndexedTriangle::from_raw_indices([a + n, c + n, b + n]));
    }
    for i in 0..n {
        let j = (i + 1) % n;
        faces.push(IndexedTriangle::from_raw_indices([i, j + n, j]));
        faces.push(IndexedTriangle::from_raw_indices([i, i + n, j + n]));
    }
    let block = IndexedMesh::new(None, vertices, faces);
    assert!(block.is_sliceable());
    let (scene, leaf) = scene_with(block, Transform3D::Identity);
    for wall in [1.0, 2.0] {
        let out = run(
            &scene,
            leaf,
            HollowRequest {
                kind: HollowKind::Apply,
                settings: HollowSettings {
                    wall_thickness_mm: wall,
                    ..HollowSettings::default()
                },
                drains: vec![
                    WorldDrain::new([20.0, 10.0, 50.0], [0.0, 0.0, -1.0], 0.5),
                    WorldDrain::new([20.0, 20.0, 50.0], [0.0, 0.0, -1.0], 0.5),
                ],
                ..HollowRequest::default()
            },
        );
        let HollowResult::Apply { mesh, drains } = &out.result else {
            panic!("apply");
        };
        assert!(drains.iter().all(|d| d.reaches_cavity), "{drains:?}");
        assert!(mesh.is_sliceable(), "wall {wall}");
        let (polygons, holes) = holes_at_mid_height(mesh.clone(), 49.9);
        assert_eq!(
            (polygons, holes),
            (1, 2),
            "wall {wall}: two drains in the tip"
        );
    }
}
