/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use core::f32;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};
use float_cmp::approx_eq;

use crate::errors::{MeshFaceError, MeshFix, MeshManifoldError, MeshNormalVectorError, MeshVertexError};
use crate::geometry::compute_normal_from_triangle_vertices;
use crate::primitives::{IndexedTriangle, Vertex};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshMetadata {
    pub name: Option<String>,
    pub header_comment: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedMesh {
    pub meta: Option<MeshMetadata>,
    pub vertices: Vec<Vertex>, // TODO: Make it generic (don't rush it)
    pub faces: Vec<IndexedTriangle>,
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

        if !errors.is_empty() {
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
                errors.push(MeshFaceError::ZeroLengthEdge {
                    face_idx: idx,
                    vertex_pos_a: 0,
                    vertex_pos_b: 1,
                    vertex_idx: f.vertices[0] as usize,
                });
            }
            if f.vertices[1] == f.vertices[2] {
                errors.push(MeshFaceError::ZeroLengthEdge {
                    face_idx: idx,
                    vertex_pos_a: 1,
                    vertex_pos_b: 2,
                    vertex_idx: f.vertices[1] as usize,
                });
            }
            if f.vertices[2] == f.vertices[0] {
                errors.push(MeshFaceError::ZeroLengthEdge {
                    face_idx: idx,
                    vertex_pos_a: 2,
                    vertex_pos_b: 0,
                    vertex_idx: f.vertices[2] as usize,
                });
            }
        }

        if !errors.is_empty() {
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
            if match cross_mag_sq.partial_cmp(&f32::EPSILON) {
                Some(Ordering::Greater) => false, // We don't enter into the block
                _ => true,
            } {
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

            // Normalize computed normal vector
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

        if !errors.is_empty() {
            Err(errors)
        } else {
            Ok(())
        }
    }

    pub fn validate_manifold(&self) -> Result<(), Vec<MeshManifoldError>> {
        use std::collections::{HashMap, HashSet};

        let mut errors: Vec<MeshManifoldError> = vec![];

        // Map each directed half-edge (a, b) to the list of face indices that contain it.
        let mut half_edges: HashMap<(u32, u32), Vec<usize>> = HashMap::new();

        if self.faces.is_empty() {
            errors.push(MeshManifoldError::EmptyMesh);
            return Err(errors);
        }

        for (face_idx, face) in self.faces.iter().enumerate() {
            let [v0, v1, v2] = face.vertices;
            for &(a, b) in &[(v0, v1), (v1, v2), (v2, v0)] {
                half_edges.entry((a, b)).or_default().push(face_idx);
            }
        }

        // Track undirected edges we've already checked to avoid double-reporting.
        let mut checked: HashSet<(u32, u32)> = HashSet::new();

        for &(a, b) in half_edges.keys() {
            // Canonical form for the undirected edge
            let canonical = if a <= b { (a, b) } else { (b, a) };
            if !checked.insert(canonical) {
                continue;
            }

            let forward = half_edges.get(&(a, b)).cloned().unwrap_or_default();
            let reverse = half_edges.get(&(b, a)).cloned().unwrap_or_default();

            let total = forward.len() + reverse.len();

            if total < 2 {
                // Boundary edge: only one face uses this edge
                let face_idx = forward.first().or(reverse.first()).copied().unwrap();
                errors.push(MeshManifoldError::BoundaryEdge {
                    edge: canonical,
                    face_idx,
                });
            } else if total > 2 {
                // Non-manifold edge: more than two faces share this edge
                let mut face_indices = forward.clone();
                face_indices.extend(&reverse);
                errors.push(MeshManifoldError::NonManifoldEdge {
                    edge: canonical,
                    face_indices,
                });
            } else {
                // Exactly two faces share this edge.
                // For consistent orientation, one should have (a,b) and the other (b,a).
                if forward.len() == 2 {
                    // Both faces traverse the edge in the same direction
                    errors.push(MeshManifoldError::InconsistentWindingOrder {
                        edge: canonical,
                        face_indices: [forward[0], forward[1]],
                    });
                } else if reverse.len() == 2 {
                    // Both faces traverse the edge in the same direction
                    errors.push(MeshManifoldError::InconsistentWindingOrder {
                        edge: canonical,
                        face_indices: [reverse[0], reverse[1]],
                    });
                }
                // else: forward.len() == 1 && reverse.len() == 1 → consistent, no error
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn fix_faces_orientation(
        &mut self,
        manifold_errors: &[MeshManifoldError],
        normal_vector_errors: &[MeshNormalVectorError],
    ) -> Result<Vec<MeshFix>> {
        let mut fixes: Vec<MeshFix> = Vec::new();

        let mut possibly_flipped_faces: HashMap<usize, u8> = std::collections::HashMap::new();
        for error in manifold_errors {
            if let MeshManifoldError::InconsistentWindingOrder {
                edge: _,
                face_indices,
            } = error
            {
                *possibly_flipped_faces.entry(face_indices[0]).or_insert(0) += 1;
                *possibly_flipped_faces.entry(face_indices[1]).or_insert(0) += 1;
            }
        }

        let mut faces_with_wrong_normals: HashSet<usize> = std::collections::HashSet::new();
        for error in normal_vector_errors {
            match error {
                MeshNormalVectorError::ExpectationMismatch { face_idx }
                | MeshNormalVectorError::NonUnitNorm { face_idx, norm: _ } => {
                    faces_with_wrong_normals.insert(*face_idx);
                }
                _ => {}
            }
        }

        let mut num_flipped_faces: u32 = 0;

        // We do a first counting round
        for (face_idx, n_errors) in &possibly_flipped_faces {
            if *n_errors > 3 {
                return Err(anyhow!(
                    "Face with ID {} has more than 3 winding order errors",
                    face_idx
                ));
            } else if *n_errors == 3 || *n_errors == 2 {
                num_flipped_faces += 1;
            }
        }

        let flipped_threshold = (self.faces.len() / 2) as u32;
        if num_flipped_faces >= flipped_threshold {
            return Err(anyhow!("Too many flipped faces"));
        }

        for (face_idx, n_errors) in &possibly_flipped_faces {
            if *n_errors == 3 || *n_errors == 2 {
                let face = &mut self.faces[*face_idx];
                face.vertices.reverse();
                fixes.push(MeshFix::FlippedFaceOrientation {
                    face_idx: *face_idx,
                });

                if faces_with_wrong_normals.contains(face_idx) {
                    // We apply a more aggressive fix
                    face.normal = compute_normal_from_triangle_vertices(&[
                        self.vertices[face.vertices[0] as usize],
                        self.vertices[face.vertices[1] as usize],
                        self.vertices[face.vertices[2] as usize],
                    ])?;
                    faces_with_wrong_normals.remove(face_idx);
                    fixes.push(MeshFix::RecomputedNormal {
                        face_idx: *face_idx,
                    });
                } else {
                    // Simple fix
                    face.normal.0 = face.normal.0.map(|c| -c);
                }
            }
        }

        // We fix remaining normal vectors after having flipped faces
        for face_idx in &faces_with_wrong_normals {
            let face = &mut self.faces[*face_idx];
            face.normal = compute_normal_from_triangle_vertices(&[
                self.vertices[face.vertices[0] as usize],
                self.vertices[face.vertices[1] as usize],
                self.vertices[face.vertices[2] as usize],
            ])?;
            fixes.push(MeshFix::RecomputedNormal {
                face_idx: *face_idx,
            });
        }

        Ok(fixes)
    }

    pub fn fix_zero_length_edges(&mut self, face_errors: &[MeshFaceError]) -> Vec<MeshFix> {
        let mut faces_to_remove: Vec<usize> = face_errors
            .iter()
            .filter_map(|e| {
                if let MeshFaceError::ZeroLengthEdge {
                    face_idx,
                    vertex_pos_a: _,
                    vertex_pos_b: _,
                    vertex_idx: _,
                } = e
                {
                    Some(*face_idx)
                } else {
                    None
                }
            })
            .collect();

        let fixes: Vec<MeshFix> = faces_to_remove
            .iter()
            .map(|&face_idx| MeshFix::RemovedZeroLengthEdgeFace { face_idx })
            .collect();

        faces_to_remove.sort();
        faces_to_remove.dedup();

        for face_idx in faces_to_remove.iter().rev() {
            // Note: `remove` is O(n), we could use `remove_swap` (O(1)), but
            //       it can mess up the ordering of our faces.
            self.faces.remove(*face_idx);
        }

        fixes
    }
}
