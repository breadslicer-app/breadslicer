/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use core::f32;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};
use float_cmp::approx_eq;

use crate::errors::{
    MeshFaceError, MeshFix, MeshManifoldError, MeshNormalVectorError, MeshVertexError,
};
use crate::geometry::compute_normal_from_triangle_vertices;
use crate::primitives::{IndexedTriangle, Vector3D, Vertex};

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
            for coord in 0..3u8 {
                if v.0[coord as usize].is_infinite() {
                    errors.push(MeshVertexError::InfiniteCoordinate {
                        vertex_idx: idx,
                        coord,
                    });
                } else if v.0[coord as usize].is_nan() {
                    errors.push(MeshVertexError::NaNCoordinate {
                        vertex_idx: idx,
                        coord,
                    });
                }
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
            // Checking out of range indices
            for pos in 0..3u8 {
                if f.vertices[pos as usize] as usize >= num_vertices {
                    errors.push(MeshFaceError::OutOfRangeVertexIndex {
                        face_idx: idx,
                        vertex_pos: pos,
                        vertex_idx: f.vertices[pos as usize] as usize,
                    });
                }
            }

            // Checking for collapsed edges
            for (pos_a, pos_b) in [(0u8, 1u8), (1, 2), (2, 0)] {
                if f.vertices[pos_a as usize] == f.vertices[pos_b as usize] {
                    errors.push(MeshFaceError::ZeroLengthEdge {
                        face_idx: idx,
                        vertex_pos_a: pos_a,
                        vertex_pos_b: pos_b,
                        vertex_idx: f.vertices[pos_a as usize] as usize,
                    });
                }
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
            // Indices are guaranteed to be correct (we check them in `validate_faces`)
            let [i0, i1, i2] = face.vertices.map(|i| i as usize);

            let v0 = Vector3D::<f64>::from(self.vertices[i0]);
            let v1 = Vector3D::<f64>::from(self.vertices[i1]);
            let v2 = Vector3D::<f64>::from(self.vertices[i2]);

            let e1 = v1.diff(&v0);
            let e2 = v2.diff(&v0);

            let cross = e1.cross_product(&e2);
            let cross_mag_sq = cross.squared_norm();

            // degenerate (zero-area) triangles; also handles NaN coordinates.
            if match cross_mag_sq.partial_cmp(&(f32::EPSILON as f64)) {
                Some(Ordering::Greater) => false, // We don't enter into the block
                _ => true,
            } {
                errors.push(MeshNormalVectorError::ZeroAreaTriangle { face_idx: idx });
                continue; // We don't continue processing this face
            }

            let n = Vector3D::<f64>::from(face.normal);
            let n_mag_sq = n.squared_norm();

            // f32 unit normals widened to f64 can have squared magnitude deviating from
            // 1.0 by up to ~3 × 2⁻²³ ≈ 3.6e-7 due to f32 quantization of each component.
            if !approx_eq!(f64, n_mag_sq, 1.0, epsilon = 1e-6) {
                errors.push(MeshNormalVectorError::NonUnitNorm {
                    face_idx: idx,
                    norm: n_mag_sq.sqrt() as f32,
                });
                continue; // We don't continue processing this face
            }

            // Normalize computed normal vector
            let cross_mag = cross_mag_sq.sqrt();
            let cross = Vector3D::new([
                cross[0] / cross_mag,
                cross[1] / cross_mag,
                cross[2] / cross_mag,
            ]);

            // Compare orientation via the dot product of the two unit vectors.
            // A per-component epsilon comparison is too sensitive: small triangles and
            // high-aspect-ratio triangles accumulate enough floating-point error that
            // their normals can diverge by a few degrees from the stored value. A 5°
            // threshold filters out this numerical noise while still catching normals
            // that are meaningfully wrong (for the most part).
            const COS_MAX_DEVIATION: f64 = 0.9961946980917455; // cos(5°)
            let dot = cross.dot_product(&n);
            if dot < COS_MAX_DEVIATION {
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
        fix_flipped_faces: bool,
        fix_face_normals: bool,
    ) -> Result<Vec<MeshFix>> {
        let mut fixes: Vec<MeshFix> = Vec::new();

        let mut possibly_flipped_faces: HashMap<usize, u8> = std::collections::HashMap::new();

        if fix_flipped_faces {
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

        if fix_flipped_faces {
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

                    if faces_with_wrong_normals.contains(face_idx) && fix_face_normals {
                        // We apply a more aggressive fix
                        let old_normal = face.normal;

                        face.normal = compute_normal_from_triangle_vertices(&[
                            self.vertices[face.vertices[0] as usize],
                            self.vertices[face.vertices[1] as usize],
                            self.vertices[face.vertices[2] as usize],
                        ])?;
                        faces_with_wrong_normals.remove(face_idx);
                        fixes.push(MeshFix::RecomputedNormal {
                            face_idx: *face_idx,
                            old_normal,
                            new_normal: face.normal,
                        });
                    } else {
                        // Simple fix
                        face.normal.0 = face.normal.0.map(|c| -c);
                    }
                }
            }
        }

        if fix_face_normals {
            // We fix remaining normal vectors after having flipped faces
            for face_idx in &faces_with_wrong_normals {
                let face = &mut self.faces[*face_idx];
                let old_normal = face.normal;
                face.normal = compute_normal_from_triangle_vertices(&[
                    self.vertices[face.vertices[0] as usize],
                    self.vertices[face.vertices[1] as usize],
                    self.vertices[face.vertices[2] as usize],
                ])?;
                fixes.push(MeshFix::RecomputedNormal {
                    face_idx: *face_idx,
                    old_normal,
                    new_normal: face.normal,
                });
            }
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

#[cfg(test)]
mod indexed_mesh_validate_vertices_tests {
    use super::*;
    use crate::errors::MeshVertexError;

    fn make_mesh(vertices: Vec<Vertex>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices,
            faces: vec![],
        }
    }

    #[test]
    fn ok_for_valid_vertices() {
        let mesh = make_mesh(vec![
            Vertex::new([0.0, 1.0, 2.0]),
            Vertex::new([-1.5, 3.0, 0.0]),
        ]);
        assert!(mesh.validate_vertices().is_ok());
    }

    #[test]
    fn ok_for_empty_mesh() {
        let mesh = make_mesh(vec![]);
        assert!(mesh.validate_vertices().is_ok());
    }

    #[test]
    fn detects_infinite_x() {
        let mesh = make_mesh(vec![Vertex::new([f32::INFINITY, 0.0, 0.0])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: 0
            }
        ));
    }

    #[test]
    fn detects_infinite_y() {
        let mesh = make_mesh(vec![Vertex::new([0.0, f32::NEG_INFINITY, 0.0])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: 1
            }
        ));
    }

    #[test]
    fn detects_infinite_z() {
        let mesh = make_mesh(vec![Vertex::new([0.0, 0.0, f32::INFINITY])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: 2
            }
        ));
    }

    #[test]
    fn detects_nan_x() {
        let mesh = make_mesh(vec![Vertex::new([f32::NAN, 0.0, 0.0])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: 0
            }
        ));
    }

    #[test]
    fn detects_nan_y() {
        let mesh = make_mesh(vec![Vertex::new([0.0, f32::NAN, 0.0])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: 1
            }
        ));
    }

    #[test]
    fn detects_nan_z() {
        let mesh = make_mesh(vec![Vertex::new([0.0, 0.0, f32::NAN])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: 2
            }
        ));
    }

    #[test]
    fn reports_multiple_errors_per_vertex() {
        let mesh = make_mesh(vec![Vertex::new([
            f32::INFINITY,
            f32::NAN,
            f32::NEG_INFINITY,
        ])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(errors.len(), 3);
        assert!(matches!(
            errors[0],
            MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: 0
            }
        ));
        assert!(matches!(
            errors[1],
            MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: 1
            }
        ));
        assert!(matches!(
            errors[2],
            MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: 2
            }
        ));
    }

    #[test]
    fn reports_errors_across_multiple_vertices() {
        let mesh = make_mesh(vec![
            Vertex::new([f32::NAN, 0.0, 0.0]),
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([0.0, f32::INFINITY, 0.0]),
        ]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(matches!(
            errors[0],
            MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: 0
            }
        ));
        assert!(matches!(
            errors[1],
            MeshVertexError::InfiniteCoordinate {
                vertex_idx: 2,
                coord: 1
            }
        ));
    }
}

#[cfg(test)]
mod indexed_mesh_validate_faces_tests {
    use super::*;
    use crate::errors::MeshFaceError;
    use crate::primitives::Normal;

    fn make_face(v0: u32, v1: u32, v2: u32) -> IndexedTriangle {
        IndexedTriangle {
            normal: Normal::new([0.0, 0.0, 1.0]),
            vertices: [v0, v1, v2],
        }
    }

    fn make_mesh_with_faces(num_vertices: usize, faces: Vec<IndexedTriangle>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0]); num_vertices],
            faces,
        }
    }

    #[test]
    fn ok_for_valid_faces() {
        let mesh = make_mesh_with_faces(4, vec![make_face(0, 1, 2), make_face(1, 2, 3)]);
        assert!(mesh.validate_faces().is_ok());
    }

    #[test]
    fn ok_for_empty_faces() {
        let mesh = make_mesh_with_faces(3, vec![]);
        assert!(mesh.validate_faces().is_ok());
    }

    #[test]
    fn detects_out_of_range_vertex_pos_0() {
        let mesh = make_mesh_with_faces(3, vec![make_face(5, 0, 1)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshFaceError::OutOfRangeVertexIndex {
                face_idx: 0,
                vertex_pos: 0,
                vertex_idx: 5
            }
        ));
    }

    #[test]
    fn detects_out_of_range_vertex_pos_1() {
        let mesh = make_mesh_with_faces(3, vec![make_face(0, 10, 1)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshFaceError::OutOfRangeVertexIndex {
                face_idx: 0,
                vertex_pos: 1,
                vertex_idx: 10
            }
        ));
    }

    #[test]
    fn detects_out_of_range_vertex_pos_2() {
        let mesh = make_mesh_with_faces(3, vec![make_face(0, 1, 99)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshFaceError::OutOfRangeVertexIndex {
                face_idx: 0,
                vertex_pos: 2,
                vertex_idx: 99
            }
        ));
    }

    #[test]
    fn detects_collapsed_edge_01() {
        let mesh = make_mesh_with_faces(3, vec![make_face(1, 1, 2)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshFaceError::ZeroLengthEdge {
                face_idx: 0,
                vertex_pos_a: 0,
                vertex_pos_b: 1,
                vertex_idx: 1
            }
        ));
    }

    #[test]
    fn detects_collapsed_edge_12() {
        let mesh = make_mesh_with_faces(3, vec![make_face(0, 2, 2)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshFaceError::ZeroLengthEdge {
                face_idx: 0,
                vertex_pos_a: 1,
                vertex_pos_b: 2,
                vertex_idx: 2
            }
        ));
    }

    #[test]
    fn detects_collapsed_edge_20() {
        let mesh = make_mesh_with_faces(3, vec![make_face(0, 1, 0)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshFaceError::ZeroLengthEdge {
                face_idx: 0,
                vertex_pos_a: 2,
                vertex_pos_b: 0,
                vertex_idx: 0
            }
        ));
    }

    #[test]
    fn degenerate_triangle_reports_all_collapsed_edges() {
        // All three vertices are the same → all three edges are collapsed
        let mesh = make_mesh_with_faces(3, vec![make_face(1, 1, 1)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 3);
        assert!(matches!(
            errors[0],
            MeshFaceError::ZeroLengthEdge {
                vertex_pos_a: 0,
                vertex_pos_b: 1,
                ..
            }
        ));
        assert!(matches!(
            errors[1],
            MeshFaceError::ZeroLengthEdge {
                vertex_pos_a: 1,
                vertex_pos_b: 2,
                ..
            }
        ));
        assert!(matches!(
            errors[2],
            MeshFaceError::ZeroLengthEdge {
                vertex_pos_a: 2,
                vertex_pos_b: 0,
                ..
            }
        ));
    }

    #[test]
    fn reports_both_out_of_range_and_collapsed() {
        // vertex 5 is out of range, and vertices 0 and 0 form a collapsed edge
        let mesh = make_mesh_with_faces(3, vec![make_face(5, 0, 0)]);
        let errors = mesh.validate_faces().unwrap_err();
        let out_of_range_count = errors
            .iter()
            .filter(|e| matches!(e, MeshFaceError::OutOfRangeVertexIndex { .. }))
            .count();
        let collapsed_count = errors
            .iter()
            .filter(|e| matches!(e, MeshFaceError::ZeroLengthEdge { .. }))
            .count();
        assert_eq!(out_of_range_count, 1);
        assert_eq!(collapsed_count, 1);
    }

    #[test]
    fn reports_correct_face_idx_across_faces() {
        let mesh = make_mesh_with_faces(
            3,
            vec![
                make_face(0, 1, 2), // valid
                make_face(0, 0, 2), // collapsed edge at face_idx 1
            ],
        );
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshFaceError::ZeroLengthEdge {
                face_idx: 1,
                vertex_pos_a: 0,
                vertex_pos_b: 1,
                vertex_idx: 0
            }
        ));
    }
}

#[cfg(test)]
mod indexed_mesh_validate_normal_vectors_tests {
    use super::*;
    use crate::errors::MeshNormalVectorError;
    use crate::primitives::Normal;

    fn make_mesh(vertices: Vec<Vertex>, faces: Vec<IndexedTriangle>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn face(normal: [f32; 3], v: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle {
            normal: Normal::new(normal),
            vertices: v,
        }
    }

    // A right triangle in the XY plane: (0,0,0), (1,0,0), (0,1,0).
    // Cross product = (0,0,1), so the correct unit normal is [0, 0, 1].
    fn xy_triangle_vertices() -> Vec<Vertex> {
        vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ]
    }

    #[test]
    fn ok_for_correct_normal() {
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, 1.0], [0, 1, 2])],
        );
        assert!(mesh.validate_normal_vectors().is_ok());
    }

    #[test]
    fn ok_for_empty_faces() {
        let mesh = make_mesh(xy_triangle_vertices(), vec![]);
        assert!(mesh.validate_normal_vectors().is_ok());
    }

    #[test]
    fn detects_zero_area_triangle_collinear_vertices() {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([2.0, 0.0, 0.0]), // collinear
        ];
        let mesh = make_mesh(vertices, vec![face([0.0, 0.0, 1.0], [0, 1, 2])]);
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshNormalVectorError::ZeroAreaTriangle { face_idx: 0 }
        ));
    }

    #[test]
    fn detects_zero_area_triangle_coincident_vertices() {
        let vertices = vec![
            Vertex::new([1.0, 2.0, 3.0]),
            Vertex::new([1.0, 2.0, 3.0]),
            Vertex::new([1.0, 2.0, 3.0]),
        ];
        let mesh = make_mesh(vertices, vec![face([0.0, 0.0, 1.0], [0, 1, 2])]);
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshNormalVectorError::ZeroAreaTriangle { face_idx: 0 }
        ));
    }

    #[test]
    fn detects_non_unit_norm_too_large() {
        // Normal with magnitude 2
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, 2.0], [0, 1, 2])],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            MeshNormalVectorError::NonUnitNorm { face_idx, norm } => {
                assert_eq!(*face_idx, 0);
                assert!((*norm - 2.0).abs() < 1e-6);
            }
            other => panic!("expected NonUnitNorm, got {:?}", other),
        }
    }

    #[test]
    fn detects_non_unit_norm_too_small() {
        // Normal with magnitude 0.5
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, 0.5], [0, 1, 2])],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            MeshNormalVectorError::NonUnitNorm { face_idx, norm } => {
                assert_eq!(*face_idx, 0);
                assert!((*norm - 0.5).abs() < 1e-6);
            }
            other => panic!("expected NonUnitNorm, got {:?}", other),
        }
    }

    #[test]
    fn detects_non_unit_norm_zero_vector() {
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, 0.0], [0, 1, 2])],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshNormalVectorError::NonUnitNorm { face_idx: 0, .. }
        ));
    }

    #[test]
    fn detects_expectation_mismatch_opposite_direction() {
        // Correct normal is [0,0,1], but we provide [0,0,-1] (unit, but wrong direction)
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, -1.0], [0, 1, 2])],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshNormalVectorError::ExpectationMismatch { face_idx: 0 }
        ));
    }

    #[test]
    fn detects_expectation_mismatch_perpendicular() {
        // Correct normal is [0,0,1], but we provide [1,0,0] (unit, but wrong axis)
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([1.0, 0.0, 0.0], [0, 1, 2])],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshNormalVectorError::ExpectationMismatch { face_idx: 0 }
        ));
    }

    #[test]
    fn reports_correct_face_idx_across_faces() {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([0.0, 0.0, 1.0]),
        ];
        // Face 0: XY plane, correct normal [0,0,1]
        // Face 1: XZ plane, vertices (0,0,0),(0,0,1),(1,0,0)
        //   cross = (0,0,1)-(0,0,0) × (1,0,0)-(0,0,0) = (0,0,1)×(1,0,0) = (0·0-1·0, 1·1-0·0, 0·0-0·1) = (0,1,0)
        //   correct normal is [0,1,0], but we give [0,-1,0]
        let mesh = make_mesh(
            vertices,
            vec![
                face([0.0, 0.0, 1.0], [0, 1, 2]),
                face([0.0, -1.0, 0.0], [0, 3, 1]),
            ],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            MeshNormalVectorError::ExpectationMismatch { face_idx: 1 }
        ));
    }

    #[test]
    fn reports_multiple_errors_across_faces() {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([2.0, 0.0, 0.0]), // collinear with v0 and v1
        ];
        let mesh = make_mesh(
            vertices,
            vec![
                face([0.0, 0.0, -1.0], [0, 1, 2]), // ExpectationMismatch
                face([0.0, 0.0, 1.0], [0, 1, 3]),  // ZeroAreaTriangle (collinear)
            ],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(matches!(
            errors[0],
            MeshNormalVectorError::ExpectationMismatch { face_idx: 0 }
        ));
        assert!(matches!(
            errors[1],
            MeshNormalVectorError::ZeroAreaTriangle { face_idx: 1 }
        ));
    }

    #[test]
    fn ok_for_non_axis_aligned_triangle() {
        // Triangle in a tilted plane
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 0.0, 1.0]),
        ];
        // cross = (1,0,0)×(0,0,1) = (0·1-0·0, 0·0-1·1, 1·0-0·0) = (0,-1,0)
        // unit normal = (0,-1,0)
        let mesh = make_mesh(vertices, vec![face([0.0, -1.0, 0.0], [0, 1, 2])]);
        assert!(mesh.validate_normal_vectors().is_ok());
    }
}

#[cfg(test)]
mod indexed_mesh_validate_manifold_tests {
    use super::*;
    use crate::errors::MeshManifoldError;
    use crate::primitives::Normal;

    fn face(v: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle {
            normal: Normal::new([0.0, 0.0, 1.0]),
            vertices: v,
        }
    }

    fn make_mesh(num_vertices: usize, faces: Vec<IndexedTriangle>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0]); num_vertices],
            faces,
        }
    }

    /// Builds a tetrahedron with consistent winding (all normals outward).
    ///
    /// Vertices:
    ///   0: (1, 1, 1)
    ///   1: (1, -1, -1)
    ///   2: (-1, 1, -1)
    ///   3: (-1, -1, 1)
    ///
    /// Each shared edge is traversed in opposite directions by its two faces.
    fn tetrahedron() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 1.0, 1.0]),
                Vertex::new([1.0, -1.0, -1.0]),
                Vertex::new([-1.0, 1.0, -1.0]),
                Vertex::new([-1.0, -1.0, 1.0]),
            ],
            faces: vec![
                face([0, 1, 2]), // edges: 0→1, 1→2, 2→0
                face([0, 3, 1]), // edges: 0→3, 3→1, 1→0
                face([0, 2, 3]), // edges: 0→2, 2→3, 3→0
                face([1, 3, 2]), // edges: 1→3, 3→2, 2→1
            ],
        }
    }

    #[test]
    fn ok_for_closed_manifold_tetrahedron() {
        assert!(tetrahedron().validate_manifold().is_ok());
    }

    #[test]
    fn rejects_empty_mesh() {
        let mesh = make_mesh(0, vec![]);
        let errors = mesh.validate_manifold().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], MeshManifoldError::EmptyMesh));
    }

    #[test]
    fn detects_boundary_edge_single_triangle() {
        // A single triangle has 3 boundary edges (each edge belongs to only one face)
        let mesh = make_mesh(3, vec![face([0, 1, 2])]);
        let errors = mesh.validate_manifold().unwrap_err();
        assert_eq!(errors.len(), 3);
        assert!(
            errors
                .iter()
                .all(|e| matches!(e, MeshManifoldError::BoundaryEdge { .. }))
        );
    }

    #[test]
    fn detects_boundary_edge_open_surface() {
        // Two triangles sharing one edge, leaving 4 boundary edges
        // Face 0: 0→1, 1→2, 2→0
        // Face 1: 1→0, 0→3, 3→1
        // Edge (0,1) is shared with consistent winding (0→1 and 1→0) — OK
        // Edges 1→2, 2→0, 0→3, 3→1 are boundary
        let mesh = make_mesh(4, vec![face([0, 1, 2]), face([1, 0, 3])]);
        let errors = mesh.validate_manifold().unwrap_err();
        let boundary_count = errors
            .iter()
            .filter(|e| matches!(e, MeshManifoldError::BoundaryEdge { .. }))
            .count();
        assert_eq!(boundary_count, 4);
    }

    #[test]
    fn detects_inconsistent_winding_order() {
        // Two triangles sharing edge (0,1) but both traverse it as 0→1
        // Face 0: 0→1, 1→2, 2→0
        // Face 1: 0→1, 1→3, 3→0
        // Edge (0,1) appears twice in the same direction → inconsistent winding
        let mesh = make_mesh(4, vec![face([0, 1, 2]), face([0, 1, 3])]);
        let errors = mesh.validate_manifold().unwrap_err();
        let winding_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, MeshManifoldError::InconsistentWindingOrder { .. }))
            .collect();
        assert_eq!(winding_errors.len(), 1);
        match &winding_errors[0] {
            MeshManifoldError::InconsistentWindingOrder { edge, face_indices } => {
                assert_eq!(*edge, (0, 1));
                assert!(face_indices.contains(&0) && face_indices.contains(&1));
            }
            other => panic!("expected InconsistentWindingOrder, got {:?}", other),
        }
    }

    #[test]
    fn detects_non_manifold_edge() {
        // Three faces all sharing edge (0,1)
        // Face 0: 0→1, 1→2, 2→0
        // Face 1: 1→0, 0→3, 3→1
        // Face 2: 0→1, 1→4, 4→0
        // Edge (0,1): forward [0, 2], reverse [1] → total 3 faces
        let mesh = make_mesh(5, vec![face([0, 1, 2]), face([1, 0, 3]), face([0, 1, 4])]);
        let errors = mesh.validate_manifold().unwrap_err();
        let non_manifold_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, MeshManifoldError::NonManifoldEdge { .. }))
            .collect();
        assert!(
            !non_manifold_errors.is_empty(),
            "expected at least one NonManifoldEdge error"
        );
        let has_edge_01 = non_manifold_errors.iter().any(|e| match e {
            MeshManifoldError::NonManifoldEdge { edge, face_indices } => {
                *edge == (0, 1) && face_indices.len() == 3
            }
            _ => false,
        });
        assert!(has_edge_01, "expected NonManifoldEdge for edge (0, 1)");
    }

    #[test]
    fn tetrahedron_with_one_flipped_face_detects_inconsistent_winding() {
        // Take a valid tetrahedron and flip one face's winding
        let mut mesh = tetrahedron();
        // Flip face 0: [0, 1, 2] → [0, 2, 1]
        // This reverses edges 0→1 to 0→2, 1→2 to 2→1, 2→0 to 1→0
        mesh.faces[0].vertices = [0, 2, 1];

        let errors = mesh.validate_manifold().unwrap_err();
        let winding_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, MeshManifoldError::InconsistentWindingOrder { .. }))
            .collect();
        // Flipping one face of a tetrahedron causes inconsistent winding on all 3 of its edges
        assert_eq!(winding_errors.len(), 3);
    }

    #[test]
    fn reports_multiple_error_types() {
        // Face 0 and Face 1 share edge (0,1) with inconsistent winding (both 0→1),
        // plus all other edges are boundary.
        let mesh = make_mesh(4, vec![face([0, 1, 2]), face([0, 1, 3])]);
        let errors = mesh.validate_manifold().unwrap_err();
        let has_winding = errors
            .iter()
            .any(|e| matches!(e, MeshManifoldError::InconsistentWindingOrder { .. }));
        let has_boundary = errors
            .iter()
            .any(|e| matches!(e, MeshManifoldError::BoundaryEdge { .. }));
        assert!(has_winding, "expected InconsistentWindingOrder errors");
        assert!(has_boundary, "expected BoundaryEdge errors");
    }
}

#[cfg(test)]
mod indexed_mesh_fix_faces_orientation_tests {
    use super::*;
    use crate::errors::{MeshManifoldError, MeshNormalVectorError};
    use crate::geometry::compute_normal_from_triangle_vertices;
    use crate::primitives::Normal;

    /// Builds a tetrahedron with consistent winding and correct normals.
    ///
    /// Vertices:
    ///   0: (1, 1, 1)
    ///   1: (1, -1, -1)
    ///   2: (-1, 1, -1)
    ///   3: (-1, -1, 1)
    fn tetrahedron() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
            Vertex::new([-1.0, 1.0, -1.0]),
            Vertex::new([-1.0, -1.0, 1.0]),
        ];
        let v = &vertices;
        let faces_data: [[u32; 3]; 4] = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];

        let faces = faces_data
            .iter()
            .map(|indices| {
                let tri_verts = [
                    v[indices[0] as usize],
                    v[indices[1] as usize],
                    v[indices[2] as usize],
                ];
                let normal =
                    compute_normal_from_triangle_vertices(&tri_verts).expect("valid triangle");
                IndexedTriangle {
                    normal,
                    vertices: *indices,
                }
            })
            .collect();

        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    #[test]
    fn no_op_when_no_errors() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();
        mesh.fix_faces_orientation(&[], &[], true, true).unwrap();
        assert_eq!(mesh, original);
    }

    #[test]
    fn fixes_single_flipped_face_vertices_and_normal() {
        let mut mesh = tetrahedron();

        // Flip face 0's winding but keep the original normal (now mismatched)
        mesh.faces[0].vertices.reverse();

        // Gather errors as the real workflow would
        let manifold_errors = mesh.validate_manifold().unwrap_err();
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        let expected = tetrahedron();
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true, true)
            .unwrap();

        // Face 0 vertices should be restored (reversed back)
        assert_eq!(mesh.faces[0].vertices, expected.faces[0].vertices);

        // Normal should match the correct normal
        let n = mesh.faces[0].normal.0;
        let en = expected.faces[0].normal.0;
        for i in 0..3 {
            assert!(
                (n[i] - en[i]).abs() < 1e-5,
                "normal component {i}: got {}, expected {}",
                n[i],
                en[i],
            );
        }
    }

    #[test]
    fn fixes_flipped_face_with_wrong_normal_by_recomputing() {
        let mut mesh = tetrahedron();

        // Flip face 0's winding and give it a completely wrong normal
        mesh.faces[0].vertices.reverse();
        mesh.faces[0].normal = Normal::new([1.0, 0.0, 0.0]);

        let manifold_errors = mesh.validate_manifold().unwrap_err();
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        let expected = tetrahedron();
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true, true)
            .unwrap();

        let n = mesh.faces[0].normal.0;
        let en = expected.faces[0].normal.0;
        for i in 0..3 {
            assert!(
                (n[i] - en[i]).abs() < 1e-5,
                "normal component {i}: got {}, expected {}",
                n[i],
                en[i],
            );
        }
    }

    #[test]
    fn fixes_wrong_normal_without_winding_error() {
        let mut mesh = tetrahedron();

        // Give face 0 a wrong normal but keep correct winding
        mesh.faces[0].normal = Normal::new([1.0, 0.0, 0.0]);

        let manifold_errors: Vec<MeshManifoldError> = vec![];
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        let expected = tetrahedron();
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, false, true)
            .unwrap();

        let n = mesh.faces[0].normal.0;
        let en = expected.faces[0].normal.0;
        for i in 0..3 {
            assert!(
                (n[i] - en[i]).abs() < 1e-5,
                "normal component {i}: got {}, expected {}",
                n[i],
                en[i],
            );
        }
    }

    #[test]
    fn fixes_non_unit_norm_without_winding_error() {
        let mut mesh = tetrahedron();

        // Scale the normal to non-unit length
        let correct = mesh.faces[1].normal.0;
        mesh.faces[1].normal = Normal::new([correct[0] * 2.0, correct[1] * 2.0, correct[2] * 2.0]);

        let manifold_errors: Vec<MeshManifoldError> = vec![];
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        let expected = tetrahedron();
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, false, true)
            .unwrap();

        let n = mesh.faces[1].normal.0;
        let en = expected.faces[1].normal.0;
        for i in 0..3 {
            assert!(
                (n[i] - en[i]).abs() < 1e-5,
                "normal component {i}: got {}, expected {}",
                n[i],
                en[i],
            );
        }
    }

    #[test]
    fn does_not_modify_correct_faces() {
        let mut mesh = tetrahedron();

        // Flip only face 0's winding (keep original normal → mismatched)
        mesh.faces[0].vertices.reverse();

        let expected = tetrahedron();

        let manifold_errors = mesh.validate_manifold().unwrap_err();
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true, true)
            .unwrap();

        // Faces 1, 2, 3 should be unchanged
        for i in 1..4 {
            assert_eq!(
                mesh.faces[i].vertices, expected.faces[i].vertices,
                "face {} vertices changed unexpectedly",
                i
            );
            let n = mesh.faces[i].normal.0;
            let en = expected.faces[i].normal.0;
            for j in 0..3 {
                assert!(
                    (n[j] - en[j]).abs() < 1e-5,
                    "face {} normal component {}: got {}, expected {}",
                    i,
                    j,
                    n[j],
                    en[j],
                );
            }
        }
    }

    #[test]
    fn errors_when_face_has_more_than_3_winding_errors() {
        let mut mesh = tetrahedron();

        // Craft manifold errors where face 0 appears in 4 InconsistentWindingOrder entries
        let manifold_errors = vec![
            MeshManifoldError::InconsistentWindingOrder {
                edge: (0, 1),
                face_indices: [0, 1],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (1, 2),
                face_indices: [0, 2],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (2, 0),
                face_indices: [0, 3],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (0, 3),
                face_indices: [0, 1],
            },
        ];

        let result = mesh.fix_faces_orientation(&manifold_errors, &[], true, true);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("more than 3 winding order errors")
        );
    }

    #[test]
    fn errors_when_too_many_faces_need_flipping() {
        // Build a mesh with 4 faces where 3 have winding issues (>= 4/2 = 2 threshold)
        let mut mesh = tetrahedron();

        // Create errors that mark faces 0, 1, 2 as needing flipping (each with 3 errors)
        let manifold_errors = vec![
            // Face 0: 3 errors
            MeshManifoldError::InconsistentWindingOrder {
                edge: (0, 1),
                face_indices: [0, 1],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (1, 2),
                face_indices: [0, 2],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (2, 0),
                face_indices: [0, 3],
            },
            // Face 1: 2 more errors (already has 1 from above)
            MeshManifoldError::InconsistentWindingOrder {
                edge: (0, 3),
                face_indices: [1, 2],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (3, 1),
                face_indices: [1, 3],
            },
            // Face 2: 1 more error (already has 2 from above)
            MeshManifoldError::InconsistentWindingOrder {
                edge: (2, 3),
                face_indices: [2, 3],
            },
        ];

        let result = mesh.fix_faces_orientation(&manifold_errors, &[], true, true);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Too many flipped faces")
        );
    }

    #[test]
    fn ignores_non_winding_manifold_errors() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        let manifold_errors = vec![
            MeshManifoldError::BoundaryEdge {
                edge: (0, 1),
                face_idx: 0,
            },
            MeshManifoldError::NonManifoldEdge {
                edge: (1, 2),
                face_indices: vec![0, 1, 2],
            },
        ];

        mesh.fix_faces_orientation(&manifold_errors, &[], true, true)
            .unwrap();
        assert_eq!(mesh, original);
    }

    #[test]
    fn ignores_zero_area_triangle_normal_errors() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        let normal_errors = vec![MeshNormalVectorError::ZeroAreaTriangle { face_idx: 0 }];

        mesh.fix_faces_orientation(&[], &normal_errors, false, false)
            .unwrap();
        assert_eq!(mesh, original);
    }

    #[test]
    fn face_with_one_winding_error_is_not_flipped() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        // Only 1 winding error for face 0 — below threshold of 2
        let manifold_errors = vec![MeshManifoldError::InconsistentWindingOrder {
            edge: (0, 1),
            face_indices: [0, 1],
        }];

        mesh.fix_faces_orientation(&manifold_errors, &[], true, true)
            .unwrap();
        assert_eq!(mesh.faces[0].vertices, original.faces[0].vertices);
    }

    #[test]
    fn skips_flipping_when_fix_flipped_faces_is_false() {
        let mut mesh = tetrahedron();

        // Flip face 0's winding but keep the original normal (now mismatched)
        mesh.faces[0].vertices.reverse();
        let flipped_vertices = mesh.faces[0].vertices;

        let manifold_errors = mesh.validate_manifold().unwrap_err();
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        // With fix_flipped_faces=false, winding must not be touched even though errors exist.
        // Normals are still recomputed (fix_face_normals=true) but since the face is still
        // wrongly wound, the recomputed normal will point inward (opposite to the correct one).
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, false, true)
            .unwrap();

        assert_eq!(
            mesh.faces[0].vertices, flipped_vertices,
            "vertices should be unchanged when fix_flipped_faces is false"
        );
    }

    #[test]
    fn skips_normal_fix_when_fix_face_normals_is_false() {
        let mut mesh = tetrahedron();

        // Give face 0 a wrong normal but keep correct winding
        mesh.faces[0].normal = Normal::new([1.0, 0.0, 0.0]);
        let wrong_normal = mesh.faces[0].normal;

        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        // With fix_face_normals=false, normals must not be touched even though errors exist
        mesh.fix_faces_orientation(&[], &normal_errors, false, false)
            .unwrap();

        assert_eq!(
            mesh.faces[0].normal, wrong_normal,
            "normal should be unchanged when fix_face_normals is false"
        );
    }

    #[test]
    fn flips_face_but_negates_normal_when_fix_face_normals_is_false() {
        let mut mesh = tetrahedron();

        // Flip face 0's winding and give it a completely wrong normal
        mesh.faces[0].vertices.reverse();
        mesh.faces[0].normal = Normal::new([1.0, 0.0, 0.0]);

        let manifold_errors = mesh.validate_manifold().unwrap_err();
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        // With fix_face_normals=false, the face is flipped back but the normal is only
        // negated (simple fix), not recomputed from vertices
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true, false)
            .unwrap();

        let expected_normal = Normal::new([-1.0, 0.0, 0.0]);
        assert_eq!(
            mesh.faces[0].normal, expected_normal,
            "normal should be negated, not recomputed, when fix_face_normals is false"
        );
    }

    #[test]
    fn mesh_is_valid_after_fixing_flipped_face() {
        let mut mesh = tetrahedron();

        // Flip face 0's winding (keep original normal → mismatched)
        mesh.faces[0].vertices.reverse();

        let manifold_errors = mesh.validate_manifold().unwrap_err();
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true, true)
            .unwrap();

        // After fix, the mesh should pass all validations
        assert!(mesh.validate_manifold().is_ok());
        assert!(mesh.validate_normal_vectors().is_ok());
    }
}

#[cfg(test)]
mod indexed_mesh_fix_zero_length_edges_tests {
    use super::*;
    use crate::errors::{MeshFaceError, MeshFix};
    use crate::primitives::Normal;

    fn make_mesh(num_vertices: usize, faces: Vec<IndexedTriangle>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0]); num_vertices],
            faces,
        }
    }

    fn face(v0: u32, v1: u32, v2: u32) -> IndexedTriangle {
        IndexedTriangle {
            normal: Normal::new([0.0, 0.0, 1.0]),
            vertices: [v0, v1, v2],
        }
    }

    #[test]
    fn no_errors_means_no_fixes_and_no_face_removed() {
        let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
        let fixes = mesh.fix_zero_length_edges(&[]);
        assert!(fixes.is_empty());
        assert_eq!(mesh.faces.len(), 1);
    }

    #[test]
    fn empty_face_list_with_unrelated_errors_is_a_no_op() {
        // Pass face errors that are *not* ZeroLengthEdge — the fix routine
        // should ignore them and leave the mesh untouched.
        let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
        let errors = vec![MeshFaceError::OutOfRangeVertexIndex {
            face_idx: 0,
            vertex_pos: 0,
            vertex_idx: 99,
        }];
        let fixes = mesh.fix_zero_length_edges(&errors);
        assert!(fixes.is_empty());
        assert_eq!(mesh.faces.len(), 1);
    }

    #[test]
    fn single_collapsed_face_is_removed_with_a_fix_record() {
        let mut mesh = make_mesh(3, vec![face(0, 1, 1)]);
        let errors = mesh.validate_faces().unwrap_err();

        let fixes = mesh.fix_zero_length_edges(&errors);

        assert_eq!(mesh.faces.len(), 0);
        assert_eq!(fixes.len(), 1);
        assert!(matches!(
            fixes[0],
            MeshFix::RemovedZeroLengthEdgeFace { face_idx: 0 }
        ));
    }

    #[test]
    fn dedup_keeps_face_removed_only_once_when_multiple_edges_collapse() {
        // A degenerate triangle (all three vertices equal) emits three
        // ZeroLengthEdge errors for the same face. Dedup must reduce
        // those to a single removal so we don't try to remove face 0 three
        // times (and panic on the second).
        let mut mesh = make_mesh(3, vec![face(0, 0, 0)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 3);

        let fixes = mesh.fix_zero_length_edges(&errors);

        assert_eq!(mesh.faces.len(), 0);
        // One face was removed, but the fix log still mirrors the original
        // error list — caller can choose to deduplicate the report.
        assert_eq!(fixes.len(), 3);
        assert!(
            fixes
                .iter()
                .all(|f| matches!(f, MeshFix::RemovedZeroLengthEdgeFace { face_idx: 0 }))
        );
    }

    #[test]
    fn removes_in_descending_order_so_remaining_indices_stay_valid() {
        // Two collapsed faces at positions 0 and 2; face 1 is healthy.
        // The implementation removes higher indices first, so face 1's
        // index doesn't shift while face 2 is being removed.
        let mut mesh = make_mesh(4, vec![face(0, 1, 1), face(0, 1, 2), face(2, 3, 3)]);
        let errors = mesh.validate_faces().unwrap_err();

        let fixes = mesh.fix_zero_length_edges(&errors);

        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
        assert_eq!(fixes.len(), 2);
    }

    #[test]
    fn mixed_error_kinds_only_remove_collapsed_faces() {
        let mut mesh = make_mesh(3, vec![face(0, 1, 2), face(0, 1, 1)]);
        let mut errors = vec![MeshFaceError::OutOfRangeVertexIndex {
            face_idx: 0,
            vertex_pos: 0,
            vertex_idx: 99,
        }];
        errors.extend(mesh.validate_faces().unwrap_err());

        let fixes = mesh.fix_zero_length_edges(&errors);

        // Only the collapsed face (index 1) was removed; the OutOfRange
        // error is irrelevant to this fix routine.
        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
        assert!(
            fixes
                .iter()
                .all(|f| matches!(f, MeshFix::RemovedZeroLengthEdgeFace { .. }))
        );
    }
}
