/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::fix::collinear_faces::test_meshes::{
    SPLIT_TETRAHEDRON, flat_tetrahedron, split_tetrahedron,
};
use crate::indexed_mesh::mesh::IndexedMesh;
use crate::indexed_mesh::validate::MeshFaceError;
use crate::indexed_mesh::validated::{MeshEdgeManifoldError, ZeroAreaFace};
use crate::primitives::{IndexedTriangle, TriVertexSlot, Vertex, VertexIndex};

fn face(v0: u32, v1: u32, v2: u32) -> IndexedTriangle {
    IndexedTriangle::from_raw_indices([v0, v1, v2])
}

fn make_mesh(num_vertices: usize, faces: Vec<IndexedTriangle>) -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![Vertex::new([0.0, 0.0, 0.0]); num_vertices],
        faces,
    }
}

#[test]
fn validated_faces_succeeds_on_clean_mesh() {
    let mesh = make_mesh(3, vec![face(0, 1, 2)]);
    let validated = mesh.validated_faces().expect("clean mesh");
    assert_eq!(validated.vertices().len(), 3);
    assert_eq!(validated.faces().len(), 1);
}

#[test]
fn validated_faces_returns_face_errors_on_out_of_range_indices() {
    let mesh = make_mesh(3, vec![face(0, 1, 99)]);
    let errors = mesh.validated_faces().expect_err("out-of-range index");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, MeshFaceError::OutOfRangeVertexIndex { .. }))
    );
}

#[test]
fn validated_faces_returns_face_errors_on_zero_length_edge() {
    let mesh = make_mesh(3, vec![face(0, 1, 1)]);
    let errors = mesh.validated_faces().expect_err("zero-length edge");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, MeshFaceError::ZeroLengthEdge { .. }))
    );
}

#[test]
fn validated_faces_mut_succeeds_on_clean_mesh() {
    let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
    let _ = mesh.validated_faces_mut().expect("clean mesh");
}

#[test]
fn validated_faces_mut_gives_the_vertices_and_accepts_the_face_areas() {
    let mut mesh = closed_tetrahedron();
    let vertices = mesh.vertices.clone();
    let witness = mesh.validated_faces_mut().expect("in-range indices");
    assert_eq!(witness.vertices(), vertices.as_slice());
    assert_eq!(witness.validate_face_areas(), Ok(()));
}

#[test]
fn validated_faces_mut_reports_a_zero_area_face() {
    // All vertices of `make_mesh` are at the origin.
    let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
    let witness = mesh.validated_faces_mut().expect("in-range indices");
    assert_eq!(
        witness.validate_face_areas(),
        Err(vec![ZeroAreaFace {
            face_idx: 0_usize.into()
        }])
    );
}

/// Pins the shortcut: when every input error is a `ZeroLengthEdge`, removing
/// the affected faces preserves the in-range invariant for survivors and
/// `validated_faces_mut` is *not* re-run. We cannot easily observe "didn't
/// re-run", but we can observe the post-condition: the returned witness must be
/// `Ok` for a mesh whose only flaw was zero-length edges.
#[test]
fn fix_zero_length_edges_into_validated_returns_ok_witness_when_only_zle() {
    let mut mesh = make_mesh(4, vec![face(0, 1, 1), face(0, 1, 2)]);
    let face_errors = mesh.validate_faces().expect_err("zle");
    let (fixes, witness) = mesh.fix_zero_length_edges_into_validated(&face_errors);
    assert_eq!(fixes.len(), face_errors.len());
    let witness = witness.expect("post-removal mesh validates");
    assert_eq!(witness.faces().len(), 1);
    assert_eq!(witness.faces()[0].vertices.map(u32::from), [0, 1, 2]);
}

/// Pins the slow path: when the input errors include a non-ZLE variant, the
/// routine re-runs `validate_faces` to confirm the post-removal mesh actually
/// validates. Here we hand-craft an error vector that includes an
/// `OutOfRangeVertexIndex` for an otherwise-clean mesh: the post-removal
/// validation still returns `Ok`, the witness is `Ok`, but the path that
/// produced it is the re-validate one.
#[test]
fn fix_zero_length_edges_into_validated_revalidates_when_mixed_errors() {
    let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
    let synthetic_errors = vec![MeshFaceError::OutOfRangeVertexIndex {
        face_idx: 0_usize.into(),
        vertex_pos: TriVertexSlot::V0,
        vertex_idx: VertexIndex::from(99_u32),
    }];
    let (fixes, witness) = mesh.fix_zero_length_edges_into_validated(&synthetic_errors);
    assert!(fixes.is_empty());
    let witness = witness.expect("post-removal mesh validates");
    assert_eq!(witness.faces().len(), 1);
}

/// Pins that a real out-of-range face combined with the slow path (any non-ZLE
/// error in the input) makes the witness `Err`.
#[test]
fn fix_zero_length_edges_into_validated_returns_err_when_post_removal_invalid() {
    let mut mesh = make_mesh(3, vec![face(0, 1, 99)]);
    let face_errors = mesh.validate_faces().expect_err("oor");
    let (fixes, witness) = mesh.fix_zero_length_edges_into_validated(&face_errors);
    assert!(fixes.is_empty());
    let _ = witness.expect_err("post-removal mesh still has out-of-range index");
}

/// Pins that a mesh with both a zero-length edge and an out-of-range index
/// gives `Err`: the removal of the zero-length-edge face moves the out-of-range
/// face to index 0.
#[test]
fn fix_zero_length_edges_into_validated_returns_err_for_both_error_kinds() {
    let mut mesh = make_mesh(3, vec![face(0, 1, 1), face(0, 1, 99), face(0, 1, 2)]);
    let face_errors = mesh.validate_faces().expect_err("zle and oor");
    let (fixes, witness) = mesh.fix_zero_length_edges_into_validated(&face_errors);
    assert_eq!(fixes.len(), 1);
    assert_eq!(
        witness.expect_err("the out-of-range face remains"),
        vec![MeshFaceError::OutOfRangeVertexIndex {
            face_idx: 0_usize.into(),
            vertex_pos: TriVertexSlot::V2,
            vertex_idx: VertexIndex::from(99_u32),
        }]
    );
}

/// Builds a closed tetrahedron with consistent outward winding, mirroring the
/// helper in `indexed_mesh_validate_edge_manifold_tests`.
fn closed_tetrahedron() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
            Vertex::new([-1.0, 1.0, -1.0]),
            Vertex::new([-1.0, -1.0, 1.0]),
        ],
        faces: vec![face(0, 1, 2), face(0, 3, 1), face(0, 2, 3), face(1, 3, 2)],
    }
}

#[test]
fn validated_edge_manifold_succeeds_on_closed_mesh() {
    let mesh = closed_tetrahedron();
    let witness = mesh
        .validated_faces()
        .expect("in-range indices")
        .validated_edge_manifold()
        .expect("manifold tetrahedron");
    assert_eq!(witness.vertices().len(), 4);
    assert_eq!(witness.faces().len(), 4);
}

#[test]
fn validated_edge_manifold_round_trips_buffers() {
    let mesh = closed_tetrahedron();
    let witness = mesh
        .validated_faces()
        .unwrap()
        .validated_edge_manifold()
        .unwrap();
    assert_eq!(witness.vertices(), mesh.vertices.as_slice());
    assert_eq!(witness.faces(), mesh.faces.as_slice());
    assert!(std::ptr::eq(witness.as_indexed_mesh(), &mesh));
}

#[test]
fn validated_edge_manifold_reports_the_zero_area_faces() {
    let clean = closed_tetrahedron();
    let witness = clean
        .validated_faces()
        .unwrap()
        .validated_edge_manifold()
        .unwrap();
    assert_eq!(witness.validate_face_areas(), Ok(()));
    for mesh in [flat_tetrahedron(), split_tetrahedron(SPLIT_TETRAHEDRON)] {
        let faces = mesh.validated_faces().unwrap();
        let in_range = faces.validate_face_areas();
        let face_0 = ZeroAreaFace {
            face_idx: 0_usize.into(),
        };
        assert_eq!(in_range, Err(vec![face_0]));
        let witness = faces.validated_edge_manifold().unwrap();
        assert_eq!(witness.validate_face_areas(), in_range);
    }
}

#[test]
fn validated_edge_manifold_fails_on_boundary_edge() {
    // 2 triangles glued along 1 edge: a non-closed strip whose remaining edges
    // are boundary edges, which the manifold check rejects.
    let mesh = make_mesh(4, vec![face(0, 1, 2), face(1, 0, 3)]);
    let errors = mesh
        .validated_faces()
        .unwrap()
        .validated_edge_manifold()
        .expect_err("open strip");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, MeshEdgeManifoldError::BoundaryEdge { .. }))
    );
}

#[test]
fn validated_edge_manifold_mut_succeeds_on_closed_mesh() {
    let mut mesh = closed_tetrahedron();
    let mut faces = mesh.validated_faces_mut().expect("in-range indices");
    assert!(faces.validated_edge_manifold_mut().is_ok());
}

#[test]
fn validated_edge_manifold_mut_gives_the_errors_of_validated_edge_manifold() {
    let mut mesh = make_mesh(4, vec![face(0, 1, 2), face(1, 0, 3)]);
    let expected = mesh
        .validated_faces()
        .unwrap()
        .validated_edge_manifold()
        .expect_err("open strip");
    let mut faces = mesh.validated_faces_mut().unwrap();
    assert_eq!(faces.validated_edge_manifold_mut().err(), Some(expected));
}

#[test]
fn fix_collinear_faces_leaves_the_in_range_witness_usable() {
    let mut mesh = split_tetrahedron(SPLIT_TETRAHEDRON);
    let mut faces = mesh.validated_faces_mut().unwrap();
    let fixes = faces
        .validated_edge_manifold_mut()
        .unwrap()
        .fix_collinear_faces();
    assert_eq!(fixes.len(), 1);
    assert_eq!(faces.validate_face_areas(), Ok(()));
}
