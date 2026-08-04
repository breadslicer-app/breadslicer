/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Phase-ordering and completion coverage for
//! [`write_scene_with_progress`] across the 4 write formats, pinned on
//! the shared manifold fixtures.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, LeafPayload, Normal, Scene, Vertex};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};

use crate::filetypes::WriteFormat;
use crate::ports::WriteSceneProgress;
use crate::scene_writer::{
    FlattenWarning, InstanceFreeScene, write_scene, write_scene_with_progress,
};

fn witness(scene: &Scene) -> InstanceFreeScene<'_> {
    InstanceFreeScene::try_new(scene).expect("test scenes carry no instances")
}

/// The 3 standing fixtures every format test writes: the cube plus
/// the Z-tunnel and Y-tunnel prisms.
fn fixture_meshes() -> Vec<IndexedMesh> {
    vec![
        unit_cube_mesh(),
        cuboid_with_z_tunnel(),
        cuboid_with_y_tunnel(),
    ]
}

/// Loader-shape graph: every mesh becomes a translated leaf under one
/// anonymous identity-Group root.
fn scene_with_leaves(meshes: Vec<IndexedMesh>) -> Scene {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    for (i, mesh) in meshes.into_iter().enumerate() {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([
                    i as f64 * 30.0,
                    0.0,
                    0.0,
                ])),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
    }
    g
}

fn face_total(meshes: &[IndexedMesh]) -> u64 {
    meshes.iter().map(|m| m.faces().len() as u64).sum()
}

fn vertex_total(meshes: &[IndexedMesh]) -> u64 {
    meshes.iter().map(|m| m.vertices().len() as u64).sum()
}

/// Writes `graph` with progress reporting and returns the reports,
/// the flatten warnings, and the written bytes.
fn write_collecting(
    graph: &Scene,
    format: WriteFormat,
    file_name: &str,
) -> (Vec<WriteSceneProgress>, Vec<FlattenWarning>, Vec<u8>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join(file_name);
    let mut reports = Vec::new();
    let warnings = write_scene_with_progress(witness(graph), &target, format, |p| reports.push(p))
        .expect("write succeeds");
    let bytes = std::fs::read(&target).expect("target written");
    (reports, warnings, bytes)
}

/// Position of a phase in execution order.
fn phase_rank(report: &WriteSceneProgress) -> u8 {
    match report {
        WriteSceneProgress::Flatten { .. } => 0,
        WriteSceneProgress::Serialize { .. } => 1,
        WriteSceneProgress::Compress { .. } => 2,
        WriteSceneProgress::Write { .. } => 3,
    }
}

fn done_of(report: &WriteSceneProgress) -> u64 {
    match report {
        WriteSceneProgress::Flatten { done, .. }
        | WriteSceneProgress::Serialize { done, .. }
        | WriteSceneProgress::Compress { done, .. }
        | WriteSceneProgress::Write { done, .. } => *done,
    }
}

/// Phases must run in declaration order and `done` must never regress
/// inside a phase.
fn assert_phases_monotone(reports: &[WriteSceneProgress]) {
    for pair in reports.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert!(
            phase_rank(a) <= phase_rank(b),
            "phase order regressed: {a:?} then {b:?}"
        );
        if phase_rank(a) == phase_rank(b) {
            assert!(
                done_of(a) <= done_of(b),
                "done regressed within a phase: {a:?} then {b:?}"
            );
        }
    }
}

fn last_serialize(reports: &[WriteSceneProgress]) -> (u64, u64) {
    reports
        .iter()
        .rev()
        .find_map(|r| match r {
            WriteSceneProgress::Serialize { done, total } => Some((*done, *total)),
            _ => None,
        })
        .expect("at least one Serialize report")
}

fn has_phase(reports: &[WriteSceneProgress], rank: u8) -> bool {
    reports.iter().any(|r| phase_rank(r) == rank)
}

#[test]
fn stl_binary_multi_object_reports_flatten_serialize_write_in_order() {
    let meshes = fixture_meshes();
    let faces = face_total(&meshes);
    let graph = scene_with_leaves(meshes);

    let (reports, warnings, _) = write_collecting(&graph, WriteFormat::StlBin, "multi.stl");

    assert_phases_monotone(&reports);
    assert_eq!(
        reports.first(),
        Some(&WriteSceneProgress::Flatten { done: 0, total: 1 })
    );
    assert!(reports.contains(&WriteSceneProgress::Flatten { done: 1, total: 1 }));
    assert_eq!(last_serialize(&reports), (faces, faces));
    assert!(!has_phase(&reports, 2), "STL never compresses");
    assert_eq!(
        reports.last(),
        Some(&WriteSceneProgress::Write { done: 1, total: 1 })
    );
    assert_eq!(
        warnings,
        vec![FlattenWarning::MultiObjectMerge { object_count: 3 }]
    );
}

#[test]
fn stl_ascii_single_object_reports_every_facet() {
    let cube = unit_cube_mesh();
    let faces = cube.faces().len() as u64;
    let graph = Scene::from(cube);

    let (reports, warnings, _) = write_collecting(&graph, WriteFormat::StlAscii, "single.stla");

    assert_phases_monotone(&reports);
    assert!(reports.contains(&WriteSceneProgress::Flatten { done: 1, total: 1 }));
    assert_eq!(last_serialize(&reports), (faces, faces));
    // Fewer facets than the throttle slots, so every facet reports:
    // the begin report plus one per facet.
    let serialize_reports = reports.iter().filter(|r| phase_rank(r) == 1).count() as u64;
    assert_eq!(serialize_reports, faces + 1);
    assert_eq!(
        reports.last(),
        Some(&WriteSceneProgress::Write { done: 1, total: 1 })
    );
    assert!(warnings.is_empty());
}

#[test]
fn obj_reports_serialize_across_leaves_without_flatten() {
    let meshes = fixture_meshes();
    let elements = vertex_total(&meshes) + face_total(&meshes);
    let graph = scene_with_leaves(meshes);

    let (reports, warnings, _) = write_collecting(&graph, WriteFormat::Obj, "multi.obj");

    assert_phases_monotone(&reports);
    assert!(!has_phase(&reports, 0), "OBJ never flattens");
    assert!(!has_phase(&reports, 2), "OBJ never compresses");
    assert_eq!(last_serialize(&reports), (elements, elements));
    assert_eq!(
        reports.last(),
        Some(&WriteSceneProgress::Write { done: 1, total: 1 })
    );
    assert!(warnings.is_empty());
}

#[test]
fn obj_total_skips_faceless_meshes() {
    // A faceless mesh emits no lines (the section writer skips it),
    // so its vertices must not count toward the serialise total or
    // `done` could never reach it.
    let faceless = IndexedMesh {
        meta: None,
        vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
        faces: vec![],
    };
    let triangle = IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([0.0, 0.0, 1.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([1.0, 0.0, 0.0]),
            [0, 1, 2],
        )],
    };
    let graph = scene_with_leaves(vec![faceless, triangle]);

    let (reports, _, _) = write_collecting(&graph, WriteFormat::Obj, "sparse.obj");

    // 3 vertex lines plus 1 face line from the triangle leaf alone.
    assert_eq!(last_serialize(&reports), (4, 4));
}

#[test]
fn threemf_reports_serialize_compress_write_in_order() {
    let graph = scene_with_leaves(fixture_meshes());

    let (reports, warnings, _) = write_collecting(&graph, WriteFormat::ThreeMf, "multi.3mf");

    assert_phases_monotone(&reports);
    assert!(!has_phase(&reports, 0), "3MF never flattens");
    match reports.first() {
        Some(WriteSceneProgress::Serialize { done: 0, total }) => {
            assert!(*total > 0, "estimated XML size must be non-zero");
        }
        other => panic!("expected an initial Serialize report, got {other:?}"),
    }
    let (done, total) = reports
        .iter()
        .rev()
        .find_map(|r| match r {
            WriteSceneProgress::Compress { done, total } => Some((*done, *total)),
            _ => None,
        })
        .expect("at least one Compress report");
    assert_eq!(done, total, "compression ends at its exact total");
    assert_eq!(
        reports.last(),
        Some(&WriteSceneProgress::Write { done: 1, total: 1 })
    );
    assert!(warnings.is_empty());
}

#[test]
fn progress_reporting_keeps_output_bytes_identical() {
    // The 3MF container embeds zip metadata that is not pinned here;
    // its output equivalence is covered by the roundtrip suites.
    for (format, file_name) in [
        (WriteFormat::Obj, "plain.obj"),
        (WriteFormat::StlBin, "plain.stl"),
        (WriteFormat::StlAscii, "plain.stla"),
    ] {
        let graph = scene_with_leaves(fixture_meshes());
        let dir = tempfile::tempdir().expect("temp dir");
        let plain_target = dir.path().join(file_name);
        write_scene(witness(&graph), &plain_target, format).expect("plain write succeeds");
        let plain_bytes = std::fs::read(&plain_target).expect("plain target written");

        let (_, _, progress_bytes) = write_collecting(&graph, format, file_name);
        assert_eq!(
            plain_bytes, progress_bytes,
            "progress reporting changed the {format:?} output"
        );
    }
}
