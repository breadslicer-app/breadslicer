/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::mesh::declared_facet_normals::DeclaredFacetNormals;
use crate::indexed_mesh::mesh::{IndexedMesh, MeshMetadata};
use crate::primitives::{FaceIndex, IndexedTriangle, Vertex};

const TETRA_FACES: [[u32; 3]; 4] = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];

fn tetra_vertices() -> Vec<Vertex> {
    [
        [1.0, 1.0, 1.0],
        [1.0, -1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
    ]
    .into_iter()
    .map(Vertex::new)
    .collect()
}

fn mesh(vertices: Vec<Vertex>, faces: &[[u32; 3]]) -> IndexedMesh {
    IndexedMesh::new(
        None,
        vertices,
        faces
            .iter()
            .copied()
            .map(IndexedTriangle::from_raw_indices)
            .collect(),
    )
}

fn tetrahedron() -> IndexedMesh {
    mesh(tetra_vertices(), &TETRA_FACES)
}

/// A tetrahedron stored twice over the same vertices, the way the
/// pre-supported files store a support twice.
fn tetrahedron_twice() -> IndexedMesh {
    let faces: Vec<[u32; 3]> = TETRA_FACES.iter().chain(&TETRA_FACES).copied().collect();
    mesh(tetra_vertices(), &faces)
}

fn faces(ids: &[usize]) -> Vec<FaceIndex> {
    ids.iter().copied().map(FaceIndex::from).collect()
}

#[test]
fn a_clean_mesh_has_no_duplicates_and_is_left_untouched() {
    let mut mesh = tetrahedron();
    let before = mesh.clone();
    assert!(mesh.duplicate_faces().is_empty());
    assert_eq!(mesh.drop_duplicate_faces(), 0);
    assert_eq!(mesh, before);
}

#[test]
fn a_body_stored_twice_loses_its_second_copy() {
    let mut mesh = tetrahedron_twice();
    assert_eq!(mesh.duplicate_faces(), faces(&[4, 5, 6, 7]));
    assert_eq!(mesh.drop_duplicate_faces(), 4);
    assert_eq!(mesh, tetrahedron());
    assert!(mesh.validated_faces().is_ok());
}

#[test]
fn a_rotated_copy_of_a_face_is_a_duplicate() {
    let mesh = mesh(tetra_vertices(), &[[0, 1, 2], [1, 2, 0], [2, 0, 1]]);
    assert_eq!(mesh.duplicate_faces(), faces(&[1, 2]));
}

#[test]
fn a_copy_with_the_opposite_winding_is_not_a_duplicate() {
    let mesh = mesh(tetra_vertices(), &[[0, 1, 2], [0, 2, 1]]);
    assert!(mesh.duplicate_faces().is_empty());
}

#[test]
fn faces_are_compared_by_position_not_by_index() {
    // The second copy goes through its own vertices, at the same positions;
    // -0.0 and 0.0 count as 1 position.
    let mut vertices = tetra_vertices();
    vertices.extend([[1.0, 1.0, 1.0], [1.0, -1.0, -1.0], [0.0, 0.0, 0.0]].map(Vertex::new));
    vertices.push(Vertex::new([-0.0, 0.0, -0.0]));
    let mesh = mesh(vertices, &[[0, 1, 6], [4, 5, 7]]);
    assert_eq!(mesh.duplicate_faces(), faces(&[1]));
}

#[test]
fn the_first_face_of_a_group_stays_and_the_order_is_kept() {
    let mut mesh = mesh(
        tetra_vertices(),
        &[
            [0, 3, 1],
            [0, 1, 2],
            [0, 3, 1],
            [1, 3, 2],
            [3, 1, 0],
            [0, 2, 3],
        ],
    );
    assert_eq!(mesh.duplicate_faces(), faces(&[2, 4]));
    assert_eq!(mesh.drop_duplicate_faces(), 2);
    let expected: Vec<IndexedTriangle> = [[0, 3, 1], [0, 1, 2], [1, 3, 2], [0, 2, 3]]
        .into_iter()
        .map(IndexedTriangle::from_raw_indices)
        .collect();
    assert_eq!(mesh.faces(), expected.as_slice());
}

#[test]
fn faces_without_a_valid_position_are_never_duplicates() {
    let mut vertices = tetra_vertices();
    vertices.push(Vertex::new([f32::NAN, 0.0, 0.0]));
    let mesh = mesh(vertices, &[[0, 1, 4], [0, 1, 4], [0, 1, 9], [0, 1, 9]]);
    assert!(mesh.duplicate_faces().is_empty());
}

#[test]
fn declared_facet_normals_follow_the_faces_that_stay() {
    let mut mesh = tetrahedron_twice();
    let normals: Vec<[u32; 3]> = (0..8).map(|i| [i, i, i]).collect();
    let declared = DeclaredFacetNormals::capture(&mesh, normals).expect("1 normal per face");
    mesh.set_meta(Some(MeshMetadata {
        name: Some("support".to_string()),
        declared_facet_normals: Some(declared),
        ..Default::default()
    }));
    assert_eq!(mesh.drop_duplicate_faces(), 4);
    let kept: Vec<[u32; 3]> = (0..4).map(|i| [i, i, i]).collect();
    assert_eq!(mesh.declared_facet_normals(), Some(kept.as_slice()));
    assert_eq!(
        mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
        Some("support")
    );
}

#[test]
fn stale_declared_facet_normals_are_dropped_with_the_faces() {
    let mut mesh = tetrahedron_twice();
    let declared =
        DeclaredFacetNormals::capture(&mesh, vec![[0; 3]; 8]).expect("1 normal per face");
    mesh.set_meta(Some(MeshMetadata {
        declared_facet_normals: Some(declared),
        ..Default::default()
    }));
    // Moving a vertex unbinds the normals from the geometry.
    mesh.vertices[0] = Vertex::new([2.0, 1.0, 1.0]);
    assert_eq!(mesh.drop_duplicate_faces(), 4);
    assert!(
        mesh.meta()
            .as_ref()
            .is_some_and(|m| m.declared_facet_normals.is_none())
    );
}

#[test]
fn a_face_and_its_reversed_copy_cancel_out() {
    let mut mesh = mesh(tetra_vertices(), &[[0, 1, 2], [0, 3, 1], [2, 1, 0]]);
    assert_eq!(mesh.opposite_face_pairs(), faces(&[0, 2]));
    assert_eq!(mesh.drop_opposite_face_pairs(), 2);
    assert_eq!(mesh.faces, [IndexedTriangle::from_raw_indices([0, 3, 1])]);
}

#[test]
fn only_as_many_faces_cancel_as_the_other_winding_has() {
    // 2 copies 1 way, 1 the other: 1 pair cancels, 1 face stays.
    let mesh = mesh(tetra_vertices(), &[[0, 1, 2], [1, 2, 0], [0, 2, 1]]);
    assert_eq!(mesh.opposite_face_pairs(), faces(&[0, 2]));
}

#[test]
fn opposite_copies_through_their_own_vertices_cancel_out() {
    // 2 boxes glued face to face store the shared wall once per box.
    let mut vertices = tetra_vertices();
    vertices.extend(tetra_vertices());
    let mesh = mesh(vertices, &[[0, 1, 2], [4, 6, 5]]);
    assert_eq!(mesh.opposite_face_pairs(), faces(&[0, 1]));
}

#[test]
fn a_clean_mesh_and_faces_with_repeated_positions_have_no_opposite_pairs() {
    assert!(tetrahedron().opposite_face_pairs().is_empty());
    assert!(tetrahedron_twice().opposite_face_pairs().is_empty());
    let mesh = mesh(
        tetra_vertices(),
        &[[0, 0, 1], [0, 1, 0], [9, 1, 2], [2, 1, 9]],
    );
    assert!(mesh.opposite_face_pairs().is_empty());
}
