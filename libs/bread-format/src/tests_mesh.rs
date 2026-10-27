/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::indexed_mesh::{IndexedMesh, MeshMetadata};
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

use crate::mesh::{DecodeMeshError, decode_mesh, encode_mesh};

/// A bumpy grid with its triangles shuffled and started at varied corners,
/// so the codec has triangles to rotate, and with awkward coordinates.
pub(crate) fn grid(side: u32) -> IndexedMesh {
    let mut vertices = Vec::new();
    for y in 0..=side {
        for x in 0..=side {
            let (fx, fy) = (x as f32 * 0.1 + 1.0 / 3.0, y as f32 * 0.1);
            vertices.push(Vertex::new([fx, fy, (fx * 7.3).sin() * fy]));
        }
    }
    vertices[0] = Vertex::new([-0.0, f32::MIN_POSITIVE / 4.0, f32::MAX]);
    let at = |x: u32, y: u32| y * (side + 1) + x;
    let mut faces = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let [a, b, c, d] = [at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1)];
            faces.push([a, b, c]);
            faces.push([c, d, a]);
        }
    }
    // A fixed scramble of face order and first corners.
    let n = faces.len();
    let faces = (0..n)
        .map(|i| {
            let mut face = faces[(i * 7919) % n];
            face.rotate_left(i % 3);
            IndexedTriangle::from_raw_indices(face)
        })
        .collect();
    IndexedMesh::new(
        Some(MeshMetadata {
            name: Some("grid ñ".into()),
            header_comment: None,
            declared_facet_normals: None,
        }),
        vertices,
        faces,
    )
}

fn assert_identical(back: &IndexedMesh, mesh: &IndexedMesh) {
    let bits = |m: &IndexedMesh| {
        m.vertices()
            .iter()
            .map(|v| v.as_array().map(f32::to_bits))
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(back), bits(mesh));
    assert_eq!(back.faces(), mesh.faces());
    assert_eq!(back.meta(), mesh.meta());
}

#[test]
fn a_mesh_comes_back_identical_with_its_first_corners() {
    let mesh = grid(40);
    // The test only means something if the codec rotates triangles.
    let indices: Vec<u32> = mesh
        .faces()
        .iter()
        .flat_map(|f| f.vertices.map(|i| i.get()))
        .collect();
    let stored: Vec<u32> = meshopt::decode_index_buffer(
        &meshopt::encode_index_buffer(&indices, mesh.vertices().len()).unwrap(),
        indices.len(),
    )
    .unwrap();
    assert_ne!(stored, indices, "the codec rotates some triangles");

    assert_identical(&decode_mesh(&encode_mesh(&mesh)).unwrap(), &mesh);
}

#[test]
fn metadata_and_its_absence_come_back() {
    let mut mesh = grid(2);
    mesh.set_meta(Some(MeshMetadata {
        name: None,
        header_comment: Some("solid exported".into()),
        declared_facet_normals: None,
    }));
    assert_identical(&decode_mesh(&encode_mesh(&mesh)).unwrap(), &mesh);
    mesh.set_meta(None);
    assert_identical(&decode_mesh(&encode_mesh(&mesh)).unwrap(), &mesh);
}

#[test]
fn an_empty_mesh_round_trips() {
    let mesh = IndexedMesh::new(None, vec![], vec![]);
    assert_identical(&decode_mesh(&encode_mesh(&mesh)).unwrap(), &mesh);
}

#[test]
fn a_cut_or_padded_payload_is_malformed() {
    let payload = encode_mesh(&grid(5));
    for cut in [0, 1, 10, payload.len() / 2, payload.len() - 1] {
        assert_eq!(
            decode_mesh(&payload[..cut]).unwrap_err(),
            DecodeMeshError::Malformed,
            "cut at {cut}"
        );
    }
    let mut padded = payload;
    padded.push(0);
    assert_eq!(
        decode_mesh(&padded).unwrap_err(),
        DecodeMeshError::Malformed
    );
}

#[test]
fn a_damaged_count_cannot_ask_for_huge_memory() {
    let mut payload = encode_mesh(&IndexedMesh::new(None, vec![], vec![]));
    // The vertex count follows the one-byte "no metadata" flag.
    payload[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        decode_mesh(&payload).unwrap_err(),
        DecodeMeshError::Malformed
    );
}
