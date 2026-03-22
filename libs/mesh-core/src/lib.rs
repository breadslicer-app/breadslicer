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

pub enum MeshVertexError {
    InfiniteCoordinate { vertex_idx: usize, coord: u8 },
    NaNCoordinate { vertex_idx: usize, coord: u8 },
}

pub enum MeshNormalVectorError {
    ZeroAreaTriangle { face_idx: usize },
    NonUnitNorm { face_idx: usize, norm: f32 },
    ExpectationMismatch { face_idx: usize },
}

pub enum MeshFaceError {
    OutOfRangeVertexIndex {
        /// Index for the problematic face
        face_idx: usize,
        /// 0, 1, or 2. Indicates which vertex index of the triangle is wrong
        vertex_pos: u8,
        /// Value of the problematic vertex index
        vertex_idx: usize,
    },
    CollapsedEdge {
        /// Index for the problematic face
        face_idx: usize,
        /// Position of one of the "first" vertex of the collapsed edge
        vertex_pos_a: u8,
        /// Position of one of the "second" vertex of the collapsed edge
        vertex_pos_b: u8,
        /// Value of the problematic vertex index
        vertex_idx: usize,
    },
}

impl IndexedMesh {
    pub fn validate_vertices(&self) -> Result<(), Vec<MeshVertexError>> {
        let mut errors: Vec<MeshVertexError> = vec![];

        for (idx, v) in self.vertices.iter().enumerate() {
            if v.0[0].is_infinite() {
                errors.push(MeshVertexError::InfiniteCoordinate {
                    vertex_idx: idx,
                    coord: 0,
                });
            } else if v.0[0].is_nan() {
                errors.push(MeshVertexError::NaNCoordinate {
                    vertex_idx: idx,
                    coord: 0,
                });
            }
            if v.0[1].is_infinite() {
                errors.push(MeshVertexError::InfiniteCoordinate {
                    vertex_idx: idx,
                    coord: 1,
                });
            } else if v.0[1].is_nan() {
                errors.push(MeshVertexError::NaNCoordinate {
                    vertex_idx: idx,
                    coord: 1,
                });
            }
            if v.0[2].is_infinite() {
                errors.push(MeshVertexError::InfiniteCoordinate {
                    vertex_idx: idx,
                    coord: 2,
                });
            } else if v.0[2].is_nan() {
                errors.push(MeshVertexError::NaNCoordinate {
                    vertex_idx: idx,
                    coord: 2,
                });
            }
        }

        if errors.len() > 0 {
            Err(errors)
        } else {
            Ok(())
        }
    }

    pub fn validate_faces(&self) -> Result<(), Vec<MeshFaceError>> {
        let num_vertices = self.vertices.len();
        let mut errors: Vec<MeshFaceError> = vec![];

        for (idx, f) in self.faces.iter().enumerate() {
            // Checking out of reange indices
            if f.vertices[0] as usize >= num_vertices {
                errors.push(MeshFaceError::OutOfRangeVertexIndex {
                    face_idx: idx,
                    vertex_pos: 0,
                    vertex_idx: f.vertices[0] as usize,
                });
            }
            if f.vertices[1] as usize >= num_vertices {
                errors.push(MeshFaceError::OutOfRangeVertexIndex {
                    face_idx: idx,
                    vertex_pos: 1,
                    vertex_idx: f.vertices[1] as usize,
                });
            }
            if f.vertices[2] as usize >= num_vertices {
                errors.push(MeshFaceError::OutOfRangeVertexIndex {
                    face_idx: idx,
                    vertex_pos: 2,
                    vertex_idx: f.vertices[2] as usize,
                });
            }

            // Checking for collapsed edges
            if f.vertices[0] == f.vertices[1] {
                errors.push(MeshFaceError::CollapsedEdge {
                    face_idx: idx,
                    vertex_pos_a: 0,
                    vertex_pos_b: 1,
                    vertex_idx: f.vertices[0] as usize,
                });
            }
            if f.vertices[1] == f.vertices[2] {
                errors.push(MeshFaceError::CollapsedEdge {
                    face_idx: idx,
                    vertex_pos_a: 1,
                    vertex_pos_b: 2,
                    vertex_idx: f.vertices[1] as usize,
                });
            }
            if f.vertices[2] == f.vertices[0] {
                errors.push(MeshFaceError::CollapsedEdge {
                    face_idx: idx,
                    vertex_pos_a: 2,
                    vertex_pos_b: 0,
                    vertex_idx: f.vertices[2] as usize,
                });
            }
        }

        if errors.len() > 0 {
            Err(errors)
        } else {
            Ok(())
        }
    }

    pub fn validate_normal_vectors(&self) -> Result<(), Vec<MeshNormalVectorError>> {
        let mut errors: Vec<MeshNormalVectorError> = vec![];

        for (idx, face) in self.faces.iter().enumerate() {
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
                errors.push(MeshNormalVectorError::ZeroAreaTriangle { face_idx: idx });
                continue; // We don't continue processing this face
            }

            let n = face.normal.0;

            let n_mag_sq = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];

            if !approx_eq!(f32, n_mag_sq, 1.0, ulps = 2) {
                errors.push(MeshNormalVectorError::NonUnitNorm {
                    face_idx: idx,
                    norm: n_mag_sq.sqrt(),
                });
                continue; // We don't continue processing this face
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
                errors.push(MeshNormalVectorError::ExpectationMismatch { face_idx: idx });
                continue;
            }
        }

        if errors.len() > 0 {
            Err(errors)
        } else {
            Ok(())
        }
    }

    pub fn check_manifold(&self) -> Result<(), String> {
        Ok(())
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
