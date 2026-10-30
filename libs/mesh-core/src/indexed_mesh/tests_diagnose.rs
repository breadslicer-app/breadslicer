/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the per-mesh diagnosis policy: validator ordering, the face gate
//! making the witness-gated results structurally absent, and `has_problems`
//! over every finding class. Each list is pinned equal to the underlying
//! validator's own output, so the policy can never drift from the validators it
//! reports on.

use crate::indexed_mesh::diagnose::{FaceDiagnosis, MeshDiagnosis};
use crate::indexed_mesh::mesh::IndexedMesh;
use crate::indexed_mesh::validated::MeshEdgeManifoldError;
use crate::primitives::{IndexedTriangle, Vertex};

/// Returns a closed, consistently wound tetrahedron.
fn tetrahedron() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([1.0, 1.0, 1.0]),
        Vertex::new([1.0, -1.0, -1.0]),
        Vertex::new([-1.0, 1.0, -1.0]),
        Vertex::new([-1.0, -1.0, 1.0]),
    ];
    let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
        .into_iter()
        .map(IndexedTriangle::from_raw_indices)
        .collect();
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// Returns the tetrahedron with vertex 3 moved to (1, 0, 0), the midpoint of
/// vertices 0 and 1: face 1 has zero area and the mesh stays edge-manifold.
fn tetrahedron_with_a_zero_area_face() -> IndexedMesh {
    let mut mesh = tetrahedron();
    mesh.vertices[3] = Vertex::new([1.0, 0.0, 0.0]);
    mesh
}

/// Returns an open strip of 2 consistently wound triangles sharing 1 edge: the
/// 4 outer edges are boundary edges (a hole).
fn open_strip() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([0.0, 0.0, 0.0]),
        Vertex::new([1.0, 0.0, 0.0]),
        Vertex::new([0.0, 1.0, 0.0]),
        Vertex::new([1.0, 1.0, 0.0]),
    ];
    let faces = [[0, 1, 2], [2, 1, 3]]
        .into_iter()
        .map(IndexedTriangle::from_raw_indices)
        .collect();
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

/// Returns the diagnosis that the validators give for `mesh`, assembled from
/// direct calls: the oracle for the fixture sweep at the end of this file.
fn expected_diagnosis(mesh: &IndexedMesh) -> MeshDiagnosis {
    let vertex_errors = mesh.validate_vertices().err().unwrap_or_default();
    let faces = match mesh.validated_faces() {
        Err(face_errors) => FaceDiagnosis::Invalid(face_errors),
        Ok(validated) => FaceDiagnosis::Valid {
            zero_area_faces: validated.validate_face_areas().err().unwrap_or_default(),
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
            zero_area_faces: vec![],
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
    // A NaN coordinate breaks no face-index rule: the gate passes and the
    // witness-gated validators still run and report.
    assert!(matches!(diagnosis.faces, FaceDiagnosis::Valid { .. }));
    assert!(diagnosis.has_problems());
}

#[test]
fn vertex_errors_alone_are_problems() {
    // The NaN vertex is in no face, so the face results stay clean.
    let mut mesh = tetrahedron();
    mesh.vertices.push(Vertex::new([f32::NAN, 0.0, 0.0]));
    let diagnosis = mesh.diagnose();
    assert_eq!(
        diagnosis.vertex_errors,
        mesh.validate_vertices().unwrap_err()
    );
    assert_eq!(
        diagnosis.faces,
        FaceDiagnosis::Valid {
            zero_area_faces: vec![],
            edge_manifold_errors: vec![],
        }
    );
    assert!(diagnosis.has_problems());
}

#[test]
fn a_failed_face_gate_leaves_the_gated_results_absent() {
    let mut mesh = tetrahedron();
    mesh.faces[0] = IndexedTriangle::from_raw_indices([0, 1, 99]);
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
        zero_area_faces,
        edge_manifold_errors,
    } = &diagnosis.faces
    else {
        panic!("open strip has valid faces; got {:?}", diagnosis.faces);
    };
    assert!(zero_area_faces.is_empty());
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
fn a_zero_area_face_is_reported() {
    let mesh = tetrahedron_with_a_zero_area_face();
    let diagnosis = mesh.diagnose();
    let FaceDiagnosis::Valid {
        zero_area_faces,
        edge_manifold_errors,
    } = &diagnosis.faces
    else {
        panic!("faces stay valid; got {:?}", diagnosis.faces);
    };
    assert_eq!(
        *zero_area_faces,
        mesh.validated_faces()
            .expect("valid faces")
            .validate_face_areas()
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
            zero_area_faces: vec![],
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
        mesh.faces[0] = IndexedTriangle::from_raw_indices([0, 1, 99]);
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
        ("zero_area", tetrahedron_with_a_zero_area_face()),
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

#[test]
fn only_a_closed_consistent_mesh_with_finite_vertices_is_sliceable() {
    assert!(tetrahedron().is_sliceable());
    // A zero-area face does not block slicing.
    assert!(tetrahedron_with_a_zero_area_face().is_sliceable());

    let mut nan_vertex = tetrahedron();
    nan_vertex.vertices[0] = Vertex::new([f32::NAN, 1.0, 1.0]);
    let mut out_of_range = tetrahedron();
    out_of_range.faces[0] = IndexedTriangle::from_raw_indices([0, 1, 99]);
    let mut flipped = tetrahedron();
    flipped.faces[0] = IndexedTriangle::from_raw_indices([0, 2, 1]);
    let empty = IndexedMesh {
        meta: None,
        vertices: vec![],
        faces: vec![],
    };
    for (name, mesh) in [
        ("nan vertex", nan_vertex),
        ("out-of-range face", out_of_range),
        ("open strip", open_strip()),
        ("flipped face", flipped),
        ("empty", empty),
    ] {
        assert!(!mesh.is_sliceable(), "{name} must not be sliceable");
    }
}
