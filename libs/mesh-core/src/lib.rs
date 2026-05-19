/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Core mesh data structures and validation routines for the slicer.
//!
//! # Conventions
//!
//! - **Coordinate frame:** right-handed, with **Z up**. The XY plane is the
//!   build platform; +Z points away from it.
//!   `OrbitCamera::up = Vec3::Z` and `Axis::Z.world_dir() = Vec3::Z` in the
//!   GUI cement this convention; the slicer engine, format readers/writers,
//!   and any new geometry code must follow it.
//! - **Units:** every coordinate, length, and translation in this crate is
//!   in **millimetres**. Mesh storage is `f32` for vertex positions and
//!   `f64` for transform matrices; both share the same millimetre scale.
//!   Format readers (STL, OBJ, …) are unit-less by spec, so the convention
//!   is "the file's numbers are millimetres"; converting other units is
//!   the caller's job.

pub mod aabb;
pub mod geometry;
pub mod indexed_mesh;
pub mod primitives;
pub mod scene_edit;
pub mod scene_graph;
pub mod scene_graph_aabb;
pub mod scene_graph_bake;
pub mod scene_graph_node;
pub mod transform;

use std::collections::HashMap;

// Re-exporting "core" symbols for ease of use
pub use crate::aabb::Aabb;
pub use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
pub use crate::primitives::{IndexedTriangle, Normal, Triangle, Vector3D, Vertex};
pub use crate::scene_graph::{SceneGraph, SceneGraphError};
pub use crate::scene_graph_bake::bake_leaf_to_world;
pub use crate::scene_graph_node::{
    CsgOp, LeafPayload, MeshResourceId, Node, NodeId, NodeKind, SceneMetadata, SubtreeResourceId,
};

/// Fallible streaming iterator of triangles, used by format readers as the
/// bridge between byte-level parsing and the indexed-mesh aggregate.
///
/// The associated `Error` is owned by the adapter; the domain core only
/// requires that yielded items report a `Result<Triangle, Self::Error>`.
/// This keeps `sc-mesh-core` free of any error-type dependency on the
/// crate that drives the stream.
pub trait TriangleIterator:
    Iterator<Item = Result<Triangle, <Self as TriangleIterator>::Error>>
{
    type Error;

    fn as_indexed_mesh(&mut self, meta: Option<MeshMetadata>) -> Result<IndexedMesh, Self::Error> {
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut triangles: Vec<IndexedTriangle> = Vec::new();
        let mut vertex_to_index: HashMap<[u32; 3], usize> = std::collections::HashMap::new();

        // Do not reserve memory in those structures based on size_hint, because we might have just
        // read bogus data.
        let mut vertex_indices: [u32; 3] = [0; 3];
        for t in self {
            let t = t?;
            for (i, vertex) in t.vertices.iter().enumerate() {
                // f32 has no Eq and no Hash, but comparing the bits will do.
                // This has the effect that if any coordinate is NaN (which does not make sense
                // anyway), its NaN payload bits will be used as the identity of the vertex.
                let bitpattern = vertex.coords().map(|f| {
                    if f == -0.0 {
                        0.0_f32.to_bits()
                    } else {
                        f.to_bits()
                    }
                });
                let index = *vertex_to_index
                    .entry(bitpattern)
                    .or_insert_with(|| vertices.len());

                if index == vertices.len() {
                    vertices.push(*vertex);
                }

                vertex_indices[i] = index as u32;
            }

            triangles.push(IndexedTriangle::from_raw_indices(t.normal, vertex_indices));
        }

        vertices.shrink_to_fit();
        triangles.shrink_to_fit();

        Ok(IndexedMesh {
            meta,
            vertices,
            faces: triangles,
        })
    }
}
