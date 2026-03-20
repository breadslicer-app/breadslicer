/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use anyhow::Result;
use float_cmp::approx_eq;

#[derive(Default, Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Vector3D<F: Copy + PartialEq + PartialOrd>(pub [F; 3]);

impl<F: Copy + PartialEq + PartialOrd> Vector3D<F> {
    pub const fn new(v: [F; 3]) -> Self {
        Self(v)
    }
}

impl<F: Copy + PartialEq + PartialOrd> From<Vector3D<F>> for [F; 3] {
    fn from(v: Vector3D<F>) -> Self {
        v.0
    }
}

impl<F: Copy + PartialEq + PartialOrd> std::ops::Index<usize> for Vector3D<F> {
    type Output = F;
    fn index(&self, i: usize) -> &Self::Output {
        assert!(i < 3); // TODO: Replace with debug_assert once we have proper tests in place
        &self.0[i]
    }
}

pub type Vertex = Vector3D<f32>;
pub type Normal = Vector3D<f32>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshMetadata {
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub normal: Normal,
    pub vertices: [Vertex; 3],
}

/// Struct that represents a triangle in the context of a mesh, in which its
/// vertices are indexed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IndexedTriangle {
    pub normal: Normal,
    pub vertices: [u32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedMesh {
    pub meta: Option<MeshMetadata>,
    pub vertices: Vec<Vertex>, // TODO: Make it generic (don't rush it)
    pub faces: Vec<IndexedTriangle>,
}

impl IndexedMesh {
    pub fn has_invalid_vertices(&self) -> bool {
        for v in &self.vertices {
            if v.0[0].is_infinite()
                || v.0[0].is_nan()
                || v.0[1].is_infinite()
                || v.0[1].is_nan()
                || v.0[2].is_infinite()
                || v.0[2].is_nan()
            {
                return true;
            }
        }

        false
    }

    pub fn has_invalid_faces(&self) -> bool {
        let num_vertices = self.vertices.len();

        for f in &self.faces {
            // Invalid references to vertices
            if f.vertices[0] as usize >= num_vertices
                || f.vertices[1] as usize >= num_vertices
                || f.vertices[2] as usize >= num_vertices
                // Not a triangle, collapsed edges:
                || f.vertices[0] == f.vertices[1]
                || f.vertices[1] == f.vertices[2]
                || f.vertices[2] == f.vertices[0]
            {
                return true;
            }
        }

        false
    }

    /// Returns `true` if every face's stored normal agrees with the right-hand rule
    /// orientation implied by its vertex winding order.
    ///
    /// Concretely, for each face the geometric normal is computed as
    /// `(v1 - v0) × (v2 - v0)`.  The stored normal is considered consistent when
    /// its dot product with that cross product is strictly positive.
    /// Degenerate (zero-area) faces are skipped.
    pub fn are_normal_vectors_consistent(&self) -> bool {
        for face in &self.faces {
            // Indices are guaranteed to be correct (we check them in `has_invalid_faces`)
            let [i0, i1, i2] = face.vertices.map(|i| i as usize);

            let v0 = self.vertices[i0].0;
            let v1 = self.vertices[i1].0;
            let v2 = self.vertices[i2].0;

            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

            // Right-hand rule: (v1 - v0) × (v2 - v0)
            let cross = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];

            let cross_mag_sq = cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2];

            // degenerate (zero-area) triangles; also handles NaN coordinates.
            if !(cross_mag_sq > 0.0) {
                // TODO: Generate user-facing warnings for this specific condition
                return false; //continue;
            }

            let n = face.normal.0;

            let n_mag_sq = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];

            if !approx_eq!(f32, n_mag_sq, 1.0, ulps = 2) {
                // TODO: Inform user about non-unitary normal vectors
                return false;
            }

            // Normalize computed normal vecctor
            let cross_mag = cross_mag_sq.sqrt();
            let cross = [
                cross[0] / cross_mag,
                cross[1] / cross_mag,
                cross[2] / cross_mag,
            ];

            if !approx_eq!(f32, cross[0], n[0], ulps = 2)
                || !approx_eq!(f32, cross[1], n[1], ulps = 2)
                || !approx_eq!(f32, cross[2], n[2], ulps = 2)
            {
                // TODO: inform about the specific normal vectors mismatch
                return false;
            }
        }
        true
    }

    pub fn is_manifold(&self) -> bool {
        todo!("Not implemented yet")
    }
}

pub trait TriangleIterator: Iterator<Item = Result<Triangle>> {
    fn as_indexed_mesh(&mut self, meta: Option<MeshMetadata>) -> Result<IndexedMesh> {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut vertex_to_index = std::collections::HashMap::new();

        // Do not reserve memory in those structures based on size_hint, because we might have just
        // read bogus data.
        let mut vertex_indices: [u32; 3] = [0; 3];
        for t in self {
            let t = t?;
            for (i, vertex) in t.vertices.iter().enumerate() {
                // f32 has no Eq and no Hash, but comparing the bits will do.
                // This has the effect that if any coordinate is NaN (which does not make sense
                // anyway), its NaN payload bits will be used as the identity of the vertex.
                let bitpattern = vertex.0.map(f32::to_bits);
                let index = *vertex_to_index
                    .entry(bitpattern)
                    .or_insert_with(|| vertices.len());

                if index == vertices.len() {
                    vertices.push(*vertex);
                }

                vertex_indices[i] = index as u32;
            }

            triangles.push(IndexedTriangle {
                normal: t.normal,
                vertices: vertex_indices,
            });
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
