/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The mesh chunk: one triangle mesh, lossless.
//!
//! Vertices and triangles go through meshoptimizer's codecs, which keep every
//! vertex bit for bit. The triangle codec may start a triangle at another of
//! its corners (same winding), so the payload also lists those triangles and
//! the decoder turns them back: the mesh comes back identical, face order and
//! first corners included.
//!
//! The mesh's name and header comment are kept. Facet normals a source STL
//! declared are not: they are file data, not geometry, and are recomputed when
//! the mesh is written out again.
//!
//! ```text
//! metadata    u8 0 | 1, then name and comment (u8 0 | 1, u32 length, UTF-8)
//! vertices    u32 count, u32 length, meshopt vertex buffer of [f32; 3]
//! triangles   u32 count, u32 length, meshopt index buffer
//! rotations   u32 count, then per triangle: u32 gap from the previous
//!             listed triangle, u8 corner (1 | 2) the stored one starts at
//! ```

use std::fmt;

use sc_mesh_core::indexed_mesh::{IndexedMesh, MeshMetadata};
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

use crate::bytes::{In, Out};
use crate::container::ChunkTag;

/// The tag of a mesh chunk.
pub const MESH_TAG: ChunkTag = ChunkTag(*b"MESH");

/// The mesh payload layout this build writes and reads.
pub const MESH_VERSION: u16 = 1;

/// Bounds on what a damaged count may ask for, from the codecs' densest
/// output: meshoptimizer spends at least a byte on each triangle, and at
/// least 3 control bytes on each block of 256 vertices (3072 vertex bytes)
/// when every vertex repeats the one before it.
const MAX_TRIANGLES_PER_BYTE: usize = 2;
const MAX_VERTEX_BYTES_PER_BYTE: usize = 1024;

/// Why a mesh payload could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeMeshError {
    /// The payload is cut short, padded or self-contradictory.
    Malformed,
    /// A triangle names a vertex the mesh does not have.
    VertexOutOfRange,
}

impl fmt::Display for DecodeMeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "the mesh data is malformed",
            Self::VertexOutOfRange => "a triangle names a vertex the mesh does not have",
        })
    }
}

impl std::error::Error for DecodeMeshError {}

/// Encodes `mesh` as a mesh chunk payload.
pub fn encode_mesh(mesh: &IndexedMesh) -> Vec<u8> {
    let vertices: Vec<[f32; 3]> = mesh.vertices().iter().map(|v| *v.as_array()).collect();
    let indices: Vec<u32> = mesh
        .faces()
        .iter()
        .flat_map(|face| face.vertices.map(|index| index.get()))
        .collect();
    let encoded_vertices =
        meshopt::encode_vertex_buffer(&vertices).expect("12-byte vertices encode");
    let encoded_indices =
        meshopt::encode_index_buffer(&indices, vertices.len()).expect("whole triangles encode");
    let stored: Vec<u32> = meshopt::decode_index_buffer(&encoded_indices, indices.len())
        .expect("the codec reads its own output");

    let mut out = Out::default();
    match mesh.meta() {
        None => out.u8(0),
        Some(meta) => {
            out.u8(1);
            out.text(meta.name.as_deref());
            out.text(meta.header_comment.as_deref());
        }
    }
    out.len(vertices.len());
    out.bytes(&encoded_vertices);
    out.len(mesh.faces().len());
    out.bytes(&encoded_indices);

    let rotations: Vec<(usize, u8)> = stored
        .chunks_exact(3)
        .zip(indices.chunks_exact(3))
        .enumerate()
        .filter(|(_, (stored, original))| stored != original)
        .map(|(triangle, (stored, original))| {
            let corner = (1..3u8)
                .find(|&r| (0..3).all(|k| stored[(k + usize::from(r)) % 3] == original[k]))
                .expect("the codec only rotates triangles");
            (triangle, corner)
        })
        .collect();
    out.len(rotations.len());
    let mut previous = 0;
    for (triangle, corner) in rotations {
        out.len(triangle - previous);
        out.u8(corner);
        previous = triangle;
    }
    out.0
}

/// Decodes a mesh chunk payload.
pub fn decode_mesh(payload: &[u8]) -> Result<IndexedMesh, DecodeMeshError> {
    decode(&mut In(payload)).ok_or(DecodeMeshError::Malformed)?
}

fn decode(input: &mut In<'_>) -> Option<Result<IndexedMesh, DecodeMeshError>> {
    let meta = match input.u8()? {
        0 => None,
        1 => Some(MeshMetadata {
            name: input.text()?,
            header_comment: input.text()?,
            declared_facet_normals: None,
        }),
        _ => return None,
    };
    let vertex_count = input.len()?;
    let encoded_vertices = input.bytes()?;
    if vertex_count.checked_mul(12)? > encoded_vertices.len() * MAX_VERTEX_BYTES_PER_BYTE {
        return None;
    }
    let triangle_count = input.len()?;
    let encoded_indices = input.bytes()?;
    if triangle_count > encoded_indices.len() * MAX_TRIANGLES_PER_BYTE {
        return None;
    }
    let vertices: Vec<[f32; 3]> =
        meshopt::decode_vertex_buffer(encoded_vertices, vertex_count).ok()?;
    let mut indices: Vec<u32> =
        meshopt::decode_index_buffer(encoded_indices, triangle_count * 3).ok()?;

    let rotations = input.count(5)?;
    let mut triangle = 0usize;
    for _ in 0..rotations {
        triangle = triangle.checked_add(input.len()?)?;
        let corner = usize::from(input.u8()?);
        if !(1..3).contains(&corner) {
            return None;
        }
        let face = indices.get_mut(triangle * 3..triangle * 3 + 3)?;
        face.rotate_left(corner);
    }
    if !input.is_empty() {
        return None;
    }
    if indices.iter().any(|&index| index as usize >= vertex_count) {
        return Some(Err(DecodeMeshError::VertexOutOfRange));
    }
    Some(Ok(IndexedMesh::new(
        meta,
        vertices.into_iter().map(Vertex::new).collect(),
        indices
            .chunks_exact(3)
            .map(|face| IndexedTriangle::from_raw_indices([face[0], face[1], face[2]]))
            .collect(),
    )))
}
