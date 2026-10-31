/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for the slicer's gate: solids that touch along an edge (2+2 edges)
//! are separated on the slicer's copy and pass; every other bad edge is
//! rejected with the errors of the mesh as given, which stays unchanged.

use super::mesh::IndexedMesh;
use super::sliceable::SliceableError;
use super::tests_watertight::{Soup, edge_manifold, volume};
use super::validated::MeshEdgeManifoldError;
use crate::primitives::Vertex;

/// 2 unit boxes that share the vertical edge at (1, 1), welded by position as
/// the STL reader does.
fn boxes_sharing_an_edge() -> IndexedMesh {
    Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .add_box([1.0, 1.0, 0.0], [2.0, 2.0, 1.0], &[], false)
        .mesh()
}

fn manifold_errors(mesh: &IndexedMesh) -> Vec<MeshEdgeManifoldError> {
    mesh.validated_faces()
        .expect("valid faces")
        .validate_edge_manifold()
        .expect_err("not edge-manifold")
}

#[test]
fn solids_that_touch_along_an_edge_are_sliceable() {
    let mesh = boxes_sharing_an_edge();
    assert!(!edge_manifold(&mesh));
    assert!(mesh.is_sliceable());
}

#[test]
fn the_slicer_copy_gets_1_vertex_per_solid_and_keeps_every_face() {
    let mut mesh = boxes_sharing_an_edge();
    let before = mesh.clone();
    let faces = mesh.make_sliceable().expect("sliceable").faces().len();
    assert_eq!(faces, 24);
    // The 2 ends of the shared edge got a copy each, at the same place.
    assert_eq!(mesh.vertices.len(), before.vertices.len() + 2);
    assert_eq!(
        &mesh.vertices[..before.vertices.len()],
        &before.vertices[..]
    );
    for added in &mesh.vertices[before.vertices.len()..] {
        assert!(before.vertices.contains(added));
    }
    // Same faces in the same order and winding, only repointed to the copies.
    for (after, was) in mesh.faces.iter().zip(&before.faces) {
        let at = |m: &IndexedMesh, f: &crate::primitives::IndexedTriangle| {
            f.vertices.map(|v| m.vertices[usize::from(v)])
        };
        assert_eq!(at(&mesh, after), at(&before, was));
    }
    assert!(edge_manifold(&mesh));
    assert!((volume(&mesh) - 2.0).abs() < 1e-9, "{}", volume(&mesh));
}

#[test]
fn a_mesh_that_passes_as_is_does_not_change() {
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .mesh();
    let before = mesh.clone();
    mesh.make_sliceable().expect("sliceable");
    assert_eq!(mesh, before);
}

#[test]
fn solids_that_touch_along_a_face_are_sliceable() {
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .add_box([1.0, 0.0, 0.0], [2.0, 1.0, 1.0], &[], false)
        .mesh();
    assert!(!edge_manifold(&mesh));
    assert!(mesh.is_sliceable());
    mesh.make_sliceable().expect("sliceable");
    assert!(edge_manifold(&mesh));
    assert!((volume(&mesh) - 2.0).abs() < 1e-9, "{}", volume(&mesh));
}

#[test]
fn an_edge_with_3_faces_is_rejected_as_before() {
    // A single face glued to an edge of a box.
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .mesh();
    mesh.vertices.push(Vertex::new([0.5, -1.0, 0.0]));
    let fin = crate::primitives::IndexedTriangle::from_raw_indices([0, 1, 8]);
    mesh.faces.push(fin);
    assert!(!mesh.is_sliceable());
    let before = mesh.clone();
    let errors = manifold_errors(&mesh);
    assert_eq!(
        mesh.make_sliceable().map(|_| ()),
        Err(SliceableError::Manifold(errors))
    );
    assert_eq!(mesh, before);
}

#[test]
fn a_2_plus_2_edge_next_to_a_hole_is_rejected_with_every_error() {
    // The 2nd box has no top: its hole keeps the mesh open after separating.
    let mut mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .add_box([1.0, 1.0, 0.0], [2.0, 2.0, 1.0], &[2, 3], false)
        .mesh();
    assert!(!mesh.is_sliceable());
    let before = mesh.clone();
    let errors = manifold_errors(&mesh);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, MeshEdgeManifoldError::NonManifoldEdge { .. }))
    );
    assert_eq!(
        mesh.make_sliceable().map(|_| ()),
        Err(SliceableError::Manifold(errors))
    );
    assert_eq!(mesh, before);
}

#[test]
fn vertex_and_face_errors_stop_the_gate_first() {
    let mut nan = boxes_sharing_an_edge();
    nan.vertices[0] = Vertex::new([f32::NAN, 0.0, 0.0]);
    assert!(!nan.is_sliceable());
    assert!(matches!(
        nan.make_sliceable(),
        Err(SliceableError::Vertices(_))
    ));

    let mut out_of_range = boxes_sharing_an_edge();
    out_of_range.faces[0] = crate::primitives::IndexedTriangle::from_raw_indices([0, 1, 99]);
    assert!(!out_of_range.is_sliceable());
    assert!(matches!(
        out_of_range.make_sliceable(),
        Err(SliceableError::Faces(_))
    ));
}

#[test]
fn an_empty_mesh_is_rejected() {
    let mut empty = IndexedMesh {
        meta: None,
        vertices: vec![],
        faces: vec![],
    };
    assert!(!empty.is_sliceable());
    assert_eq!(
        empty.make_sliceable().map(|_| ()),
        Err(SliceableError::Manifold(vec![
            MeshEdgeManifoldError::EmptyMesh
        ]))
    );
}
