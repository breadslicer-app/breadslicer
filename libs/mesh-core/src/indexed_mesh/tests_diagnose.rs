/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the per-mesh diagnosis policy: validator ordering, the
//! face gate making the witness-gated results structurally absent,
//! and `has_problems` over every finding class. Each list is pinned
//! equal to the underlying validator's own output, so the policy can
//! never drift from the validators it reports on.

use crate::geometry::compute_normal_from_triangle_vertices;
use crate::indexed_mesh::IndexedMesh;
use crate::indexed_mesh::diagnose::{FaceDiagnosis, MeshDiagnosis};
use crate::indexed_mesh::validated::MeshEdgeManifoldError;
use crate::primitives::{IndexedTriangle, Normal, Vertex};

/// A closed, consistently wound tetrahedron with correct normals.
fn tetrahedron() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([1.0, 1.0, 1.0]),
        Vertex::new([1.0, -1.0, -1.0]),
        Vertex::new([-1.0, 1.0, -1.0]),
        Vertex::new([-1.0, -1.0, 1.0]),
    ];
    let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
        .into_iter()
        .map(|indices: [u32; 3]| {
            let tri = [
                vertices[indices[0] as usize],
                vertices[indices[1] as usize],
                vertices[indices[2] as usize],
            ];
            let normal = compute_normal_from_triangle_vertices(&tri).expect("valid triangle");
            IndexedTriangle::from_raw_indices(normal, indices)
        })
        .collect();
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// An open strip of 2 consistently wound triangles sharing one edge:
/// the 4 outer edges are boundary edges (a hole).
fn open_strip() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([0.0, 0.0, 0.0]),
        Vertex::new([1.0, 0.0, 0.0]),
        Vertex::new([0.0, 1.0, 0.0]),
        Vertex::new([1.0, 1.0, 0.0]),
    ];
    let faces = [[0, 1, 2], [2, 1, 3]]
        .into_iter()
        .map(|indices: [u32; 3]| {
            let tri = [
                vertices[indices[0] as usize],
                vertices[indices[1] as usize],
                vertices[indices[2] as usize],
            ];
            let normal = compute_normal_from_triangle_vertices(&tri).expect("valid triangle");
            IndexedTriangle::from_raw_indices(normal, indices)
        })
        .collect();
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// The diagnosis the validators themselves dictate for `mesh`,
/// assembled from direct calls: the equivalence oracle every test
/// below compares against.
fn expected_diagnosis(mesh: &IndexedMesh) -> MeshDiagnosis {
    let vertex_errors = mesh.validate_vertices().err().unwrap_or_default();
    let faces = match mesh.validated_faces() {
        Err(face_errors) => FaceDiagnosis::Invalid(face_errors),
        Ok(validated) => FaceDiagnosis::Valid {
            normal_vector_errors: validated
                .validate_normal_vectors()
                .err()
                .unwrap_or_default(),
            edge_manifold_errors: validated.validate_edge_manifold().err().unwrap_or_default(),
        },
    };
    MeshDiagnosis {
        vertex_errors,
        faces,
    }
}

#[test]
fn clean_mesh_diagnoses_clean() {
    let diagnosis = tetrahedron().diagnose();
    assert!(diagnosis.vertex_errors.is_empty());
    assert_eq!(
        diagnosis.faces,
        FaceDiagnosis::Valid {
            normal_vector_errors: vec![],
            edge_manifold_errors: vec![],
        }
    );
    assert!(!diagnosis.has_problems());
}

#[test]
fn invalid_vertices_do_not_gate_the_face_results() {
    let mut mesh = tetrahedron();
    mesh.vertices[0] = Vertex::new([f32::NAN, 1.0, 1.0]);
    let diagnosis = mesh.diagnose();
    assert_eq!(
        diagnosis.vertex_errors,
        mesh.validate_vertices().unwrap_err()
    );
    // A NaN coordinate breaks no face-index rule: the gate passes
    // and the witness-gated validators still run and report.
    assert!(matches!(diagnosis.faces, FaceDiagnosis::Valid { .. }));
    assert!(diagnosis.has_problems());
}

#[test]
fn a_failed_face_gate_leaves_the_gated_results_absent() {
    let mut mesh = tetrahedron();
    mesh.faces[0] = IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 99]);
    let diagnosis = mesh.diagnose();
    assert_eq!(
        diagnosis.faces,
        FaceDiagnosis::Invalid(mesh.validate_faces().unwrap_err())
    );
    assert!(diagnosis.has_problems());
}

#[test]
fn an_open_strip_reports_its_boundary_edges() {
    let mesh = open_strip();
    let diagnosis = mesh.diagnose();
    let FaceDiagnosis::Valid {
        normal_vector_errors,
        edge_manifold_errors,
    } = &diagnosis.faces
    else {
        panic!("open strip has valid faces; got {:?}", diagnosis.faces);
    };
    assert!(normal_vector_errors.is_empty());
    assert_eq!(
        *edge_manifold_errors,
        mesh.validated_faces()
            .expect("valid faces")
            .validate_edge_manifold()
            .unwrap_err()
    );
    assert!(diagnosis.has_problems());
}

#[test]
fn a_non_unit_normal_reports_normal_vector_errors() {
    let mut mesh = tetrahedron();
    let doubled = mesh.faces[0].normal.coords().map(|c| c * 2.0);
    mesh.faces[0].normal = Normal::new(doubled);
    let diagnosis = mesh.diagnose();
    let FaceDiagnosis::Valid {
        normal_vector_errors,
        edge_manifold_errors,
    } = &diagnosis.faces
    else {
        panic!("faces stay valid; got {:?}", diagnosis.faces);
    };
    assert_eq!(
        *normal_vector_errors,
        mesh.validated_faces()
            .expect("valid faces")
            .validate_normal_vectors()
            .unwrap_err()
    );
    assert!(edge_manifold_errors.is_empty());
    assert!(diagnosis.has_problems());
}

#[test]
fn an_empty_mesh_reports_the_empty_manifold_error() {
    let mesh = IndexedMesh {
        meta: None,
        vertices: vec![],
        faces: vec![],
    };
    let diagnosis = mesh.diagnose();
    assert_eq!(
        diagnosis.faces,
        FaceDiagnosis::Valid {
            normal_vector_errors: vec![],
            edge_manifold_errors: vec![MeshEdgeManifoldError::EmptyMesh],
        }
    );
    assert!(diagnosis.has_problems());
}

#[test]
fn diagnosis_equals_the_validators_for_every_fixture_class() {
    let nan = {
        let mut mesh = tetrahedron();
        mesh.vertices[0] = Vertex::new([f32::NAN, 1.0, 1.0]);
        mesh
    };
    let out_of_range = {
        let mut mesh = tetrahedron();
        mesh.faces[0] = IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 99]);
        mesh
    };
    let bad_normal = {
        let mut mesh = tetrahedron();
        let doubled = mesh.faces[0].normal.coords().map(|c| c * 2.0);
        mesh.faces[0].normal = Normal::new(doubled);
        mesh
    };
    let empty = IndexedMesh {
        meta: None,
        vertices: vec![],
        faces: vec![],
    };
    let fixtures = [
        ("clean", tetrahedron()),
        ("nan", nan),
        ("out_of_range", out_of_range),
        ("open_strip", open_strip()),
        ("bad_normal", bad_normal),
        ("empty", empty),
    ];
    for (name, mesh) in fixtures {
        assert_eq!(
            mesh.diagnose(),
            expected_diagnosis(&mesh),
            "diagnosis drifted from the validators for fixture {name}"
        );
    }
}
