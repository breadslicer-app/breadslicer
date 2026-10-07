/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
use crate::primitives::{IndexedTriangle, Normal, Vertex};

const TRIANGLE: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

const SQUARE: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [1.0, 1.0, 0.0],
];

/// Builds a mesh with the same placeholder normal on every face.
fn mesh(vertices: &[[f32; 3]], faces: &[[u32; 3]]) -> IndexedMesh {
    IndexedMesh::new(
        None,
        vertices.iter().copied().map(Vertex::new).collect(),
        faces
            .iter()
            .map(|&indices| {
                IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), indices)
            })
            .collect(),
    )
}

#[test]
fn encode_geometry_of_a_triangle_matches_the_literal_layout() {
    let expected: [u8; 64] = [
        3, 0, 0, 0, 0, 0, 0, 0, // vertex count
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // (0, 0, 0)
        0, 0, 0x80, 0x3f, 0, 0, 0, 0, 0, 0, 0, 0, // (1, 0, 0)
        0, 0, 0, 0, 0, 0, 0x80, 0x3f, 0, 0, 0, 0, // (0, 1, 0)
        1, 0, 0, 0, 0, 0, 0, 0, // face count
        0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, // (0, 1, 2)
    ];
    let mut actual = Vec::new();
    mesh(&TRIANGLE, &[[0, 1, 2]]).encode_geometry(|bytes| actual.extend_from_slice(bytes));
    assert_eq!(actual, expected);
}

#[test]
fn geometry_fingerprint_of_a_triangle_matches_the_sha384_oracle() {
    // Computed with Python's `hashlib.sha384` over the 64 bytes of the
    // triangle's encoding.
    let expected: [u8; 48] = [
        0x7f, 0x13, 0x1a, 0x3b, 0xda, 0xf4, 0xb3, 0x04, 0xc0, 0xb1, 0x6e, 0x43, 0x5c, 0xce, 0xf6,
        0xd0, 0x2d, 0x17, 0xf1, 0x1c, 0xf6, 0xf3, 0xd5, 0x73, 0xcc, 0xa7, 0x8b, 0xbf, 0x18, 0xb2,
        0xe6, 0x52, 0x1e, 0xbe, 0x42, 0xa9, 0x8e, 0x2e, 0x7d, 0x8d, 0x0a, 0xdf, 0x2f, 0xde, 0xc3,
        0x68, 0xf7, 0x0c,
    ];
    let actual: [u8; 48] = mesh(&TRIANGLE, &[[0, 1, 2]]).geometry_fingerprint().into();
    assert_eq!(actual, expected);
}

#[test]
fn geometry_fingerprint_changes_when_a_coordinate_turns_from_0_to_negative_0() {
    let negative_zero = [[-0.0, 0.0, 0.0], TRIANGLE[1], TRIANGLE[2]];
    assert_ne!(
        mesh(&negative_zero, &[[0, 1, 2]]).geometry_fingerprint(),
        mesh(&TRIANGLE, &[[0, 1, 2]]).geometry_fingerprint(),
    );
}

#[test]
fn geometry_fingerprint_changes_when_1_index_changes() {
    assert_ne!(
        mesh(&SQUARE, &[[0, 1, 2], [2, 1, 0]]).geometry_fingerprint(),
        mesh(&SQUARE, &[[0, 1, 2], [2, 1, 3]]).geometry_fingerprint(),
    );
}

#[test]
fn geometry_fingerprint_changes_when_2_faces_swap() {
    assert_ne!(
        mesh(&SQUARE, &[[2, 1, 3], [0, 1, 2]]).geometry_fingerprint(),
        mesh(&SQUARE, &[[0, 1, 2], [2, 1, 3]]).geometry_fingerprint(),
    );
}

#[test]
fn geometry_fingerprint_changes_when_an_unused_vertex_is_appended() {
    let with_unused = [TRIANGLE[0], TRIANGLE[1], TRIANGLE[2], [5.0, 5.0, 5.0]];
    assert_ne!(
        mesh(&with_unused, &[[0, 1, 2]]).geometry_fingerprint(),
        mesh(&TRIANGLE, &[[0, 1, 2]]).geometry_fingerprint(),
    );
}

#[test]
fn geometry_fingerprint_ignores_the_metadata() {
    let mut named = mesh(&TRIANGLE, &[[0, 1, 2]]);
    named.set_meta(Some(MeshMetadata {
        name: Some("named".to_string()),
        header_comment: Some("comment".to_string()),
    }));
    assert_eq!(
        named.geometry_fingerprint(),
        mesh(&TRIANGLE, &[[0, 1, 2]]).geometry_fingerprint(),
    );
}

#[test]
fn geometry_fingerprint_ignores_the_stored_face_normals() {
    let mut flipped_normal = mesh(&TRIANGLE, &[[0, 1, 2]]);
    flipped_normal.faces[0].normal = Normal::new([0.0, 0.0, -1.0]);
    assert_eq!(
        flipped_normal.geometry_fingerprint(),
        mesh(&TRIANGLE, &[[0, 1, 2]]).geometry_fingerprint(),
    );
}
