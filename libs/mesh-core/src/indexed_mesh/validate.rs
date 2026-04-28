/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use float_cmp::approx_eq;

use crate::errors::{
    CoordAxis, FaceIndex, MeshFaceError, MeshManifoldError, MeshNormalVectorError, MeshVertexError,
    TriVertexSlot, VertexIndex,
};
use crate::geometry::{
    DEGENERATE_AREA_SQUARED, NORMAL_ANGLE_TOLERANCE_COS, UNIT_NORM_TOLERANCE, gt_strict,
};
use crate::primitives::Vector3D;

use super::IndexedMesh;

#[cfg(test)]
use crate::primitives::{IndexedTriangle, Vertex};

impl IndexedMesh {
    pub fn validate_vertices(&self) -> Result<(), Vec<MeshVertexError>> {
        let mut errors: Vec<MeshVertexError> = vec![];

        for (idx, v) in self.vertices.iter().enumerate() {
            for &coord in &CoordAxis::ALL {
                let value = v[coord.as_index()];
                if value.is_infinite() {
                    errors.push(MeshVertexError::InfiniteCoordinate {
                        vertex_idx: idx,
                        coord,
                    });
                } else if value.is_nan() {
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
            let face_idx = FaceIndex::from(idx);
            // Checking out of range indices
            for &vertex_pos in &TriVertexSlot::ALL {
                let vertex_idx = f.vertices[vertex_pos.as_index()];
                if vertex_idx as usize >= num_vertices {
                    errors.push(MeshFaceError::OutOfRangeVertexIndex {
                        face_idx,
                        vertex_pos,
                        vertex_idx: VertexIndex::from(vertex_idx),
                    });
                }
            }

            // Checking for collapsed edges
            for (pos_a, pos_b) in [
                (TriVertexSlot::V0, TriVertexSlot::V1),
                (TriVertexSlot::V1, TriVertexSlot::V2),
                (TriVertexSlot::V2, TriVertexSlot::V0),
            ] {
                let a = f.vertices[pos_a.as_index()];
                let b = f.vertices[pos_b.as_index()];
                if a == b {
                    errors.push(MeshFaceError::ZeroLengthEdge {
                        face_idx,
                        vertex_pos_a: pos_a,
                        vertex_pos_b: pos_b,
                        vertex_idx: VertexIndex::from(a),
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
            let face_idx = FaceIndex::from(idx);
            // Indices are guaranteed to be correct (we check them in `validate_faces`)
            let [i0, i1, i2] = face.vertices.map(|i| i as usize);

            let v0 = self.vertices[i0].to_f64();
            let v1 = self.vertices[i1].to_f64();
            let v2 = self.vertices[i2].to_f64();

            let e1 = v1.diff(&v0);
            let e2 = v2.diff(&v0);

            let cross = e1.cross_product(&e2);
            let cross_mag_sq = cross.squared_norm();

            // degenerate (zero-area) triangles; also handles NaN coordinates.
            if !gt_strict(cross_mag_sq, DEGENERATE_AREA_SQUARED) {
                errors.push(MeshNormalVectorError::ZeroAreaTriangle { face_idx });
                continue; // We don't continue processing this face
            }

            let n = face.normal.to_f64();
            let n_mag_sq = n.squared_norm();

            if !approx_eq!(f64, n_mag_sq, 1.0, epsilon = UNIT_NORM_TOLERANCE) {
                errors.push(MeshNormalVectorError::NonUnitNorm {
                    face_idx,
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

            let dot = cross.dot_product(&n);
            if dot < NORMAL_ANGLE_TOLERANCE_COS {
                errors.push(MeshNormalVectorError::ExpectationMismatch { face_idx });
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
        let mut half_edges: HashMap<(u32, u32), Vec<FaceIndex>> = HashMap::new();

        if self.faces.is_empty() {
            errors.push(MeshManifoldError::EmptyMesh);
            return Err(errors);
        }

        for (idx, face) in self.faces.iter().enumerate() {
            let face_idx = FaceIndex::from(idx);
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
}

#[cfg(test)]
mod indexed_mesh_validate_vertices_tests {
    use super::*;
    use crate::errors::{CoordAxis, MeshVertexError};

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
        assert_eq!(
            errors,
            vec![MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: CoordAxis::X,
            }]
        );
    }

    #[test]
    fn detects_infinite_y() {
        let mesh = make_mesh(vec![Vertex::new([0.0, f32::NEG_INFINITY, 0.0])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: CoordAxis::Y,
            }]
        );
    }

    #[test]
    fn detects_infinite_z() {
        let mesh = make_mesh(vec![Vertex::new([0.0, 0.0, f32::INFINITY])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: CoordAxis::Z,
            }]
        );
    }

    #[test]
    fn detects_nan_x() {
        let mesh = make_mesh(vec![Vertex::new([f32::NAN, 0.0, 0.0])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: CoordAxis::X,
            }]
        );
    }

    #[test]
    fn detects_nan_y() {
        let mesh = make_mesh(vec![Vertex::new([0.0, f32::NAN, 0.0])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: CoordAxis::Y,
            }]
        );
    }

    #[test]
    fn detects_nan_z() {
        let mesh = make_mesh(vec![Vertex::new([0.0, 0.0, f32::NAN])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: CoordAxis::Z,
            }]
        );
    }

    #[test]
    fn reports_multiple_errors_per_vertex() {
        let mesh = make_mesh(vec![Vertex::new([
            f32::INFINITY,
            f32::NAN,
            f32::NEG_INFINITY,
        ])]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(
            errors,
            vec![
                MeshVertexError::InfiniteCoordinate {
                    vertex_idx: 0,
                    coord: CoordAxis::X,
                },
                MeshVertexError::NaNCoordinate {
                    vertex_idx: 0,
                    coord: CoordAxis::Y,
                },
                MeshVertexError::InfiniteCoordinate {
                    vertex_idx: 0,
                    coord: CoordAxis::Z,
                },
            ]
        );
    }

    #[test]
    fn reports_errors_across_multiple_vertices() {
        let mesh = make_mesh(vec![
            Vertex::new([f32::NAN, 0.0, 0.0]),
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([0.0, f32::INFINITY, 0.0]),
        ]);
        let errors = mesh.validate_vertices().unwrap_err();
        assert_eq!(
            errors,
            vec![
                MeshVertexError::NaNCoordinate {
                    vertex_idx: 0,
                    coord: CoordAxis::X,
                },
                MeshVertexError::InfiniteCoordinate {
                    vertex_idx: 2,
                    coord: CoordAxis::Y,
                },
            ]
        );
    }
}

#[cfg(test)]
mod indexed_mesh_validate_faces_tests {
    use super::*;
    use crate::errors::{MeshFaceError, TriVertexSlot};
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
        assert_eq!(
            errors,
            vec![MeshFaceError::OutOfRangeVertexIndex {
                face_idx: 0_usize.into(),
                vertex_pos: TriVertexSlot::V0,
                vertex_idx: 5_u32.into(),
            }]
        );
    }

    #[test]
    fn detects_out_of_range_vertex_pos_1() {
        let mesh = make_mesh_with_faces(3, vec![make_face(0, 10, 1)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshFaceError::OutOfRangeVertexIndex {
                face_idx: 0_usize.into(),
                vertex_pos: TriVertexSlot::V1,
                vertex_idx: 10_u32.into(),
            }]
        );
    }

    #[test]
    fn detects_out_of_range_vertex_pos_2() {
        let mesh = make_mesh_with_faces(3, vec![make_face(0, 1, 99)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshFaceError::OutOfRangeVertexIndex {
                face_idx: 0_usize.into(),
                vertex_pos: TriVertexSlot::V2,
                vertex_idx: 99_u32.into(),
            }]
        );
    }

    #[test]
    fn detects_collapsed_edge_01() {
        let mesh = make_mesh_with_faces(3, vec![make_face(1, 1, 2)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshFaceError::ZeroLengthEdge {
                face_idx: 0_usize.into(),
                vertex_pos_a: TriVertexSlot::V0,
                vertex_pos_b: TriVertexSlot::V1,
                vertex_idx: 1_u32.into(),
            }]
        );
    }

    #[test]
    fn detects_collapsed_edge_12() {
        let mesh = make_mesh_with_faces(3, vec![make_face(0, 2, 2)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshFaceError::ZeroLengthEdge {
                face_idx: 0_usize.into(),
                vertex_pos_a: TriVertexSlot::V1,
                vertex_pos_b: TriVertexSlot::V2,
                vertex_idx: 2_u32.into(),
            }]
        );
    }

    #[test]
    fn detects_collapsed_edge_20() {
        let mesh = make_mesh_with_faces(3, vec![make_face(0, 1, 0)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshFaceError::ZeroLengthEdge {
                face_idx: 0_usize.into(),
                vertex_pos_a: TriVertexSlot::V2,
                vertex_pos_b: TriVertexSlot::V0,
                vertex_idx: 0_u32.into(),
            }]
        );
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
                vertex_pos_a: TriVertexSlot::V0,
                vertex_pos_b: TriVertexSlot::V1,
                ..
            }
        ));
        assert!(matches!(
            errors[1],
            MeshFaceError::ZeroLengthEdge {
                vertex_pos_a: TriVertexSlot::V1,
                vertex_pos_b: TriVertexSlot::V2,
                ..
            }
        ));
        assert!(matches!(
            errors[2],
            MeshFaceError::ZeroLengthEdge {
                vertex_pos_a: TriVertexSlot::V2,
                vertex_pos_b: TriVertexSlot::V0,
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
        assert_eq!(
            errors,
            vec![MeshFaceError::ZeroLengthEdge {
                face_idx: 1_usize.into(),
                vertex_pos_a: TriVertexSlot::V0,
                vertex_pos_b: TriVertexSlot::V1,
                vertex_idx: 0_u32.into(),
            }]
        );
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
        assert_eq!(
            errors,
            vec![MeshNormalVectorError::ZeroAreaTriangle {
                face_idx: 0_usize.into()
            }]
        );
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
        assert_eq!(
            errors,
            vec![MeshNormalVectorError::ZeroAreaTriangle {
                face_idx: 0_usize.into()
            }]
        );
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
                assert_eq!(*face_idx, 0_usize.into());
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
                assert_eq!(*face_idx, 0_usize.into());
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
        match &errors[0] {
            MeshNormalVectorError::NonUnitNorm { face_idx, norm: _ } => {
                assert_eq!(*face_idx, 0_usize.into());
            }
            other => panic!("expected NonUnitNorm, got {:?}", other),
        }
    }

    #[test]
    fn detects_expectation_mismatch_opposite_direction() {
        // Correct normal is [0,0,1], but we provide [0,0,-1] (unit, but wrong direction)
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, -1.0], [0, 1, 2])],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshNormalVectorError::ExpectationMismatch {
                face_idx: 0_usize.into()
            }]
        );
    }

    #[test]
    fn detects_expectation_mismatch_perpendicular() {
        // Correct normal is [0,0,1], but we provide [1,0,0] (unit, but wrong axis)
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([1.0, 0.0, 0.0], [0, 1, 2])],
        );
        let errors = mesh.validate_normal_vectors().unwrap_err();
        assert_eq!(
            errors,
            vec![MeshNormalVectorError::ExpectationMismatch {
                face_idx: 0_usize.into()
            }]
        );
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
        assert_eq!(
            errors,
            vec![MeshNormalVectorError::ExpectationMismatch {
                face_idx: 1_usize.into()
            }]
        );
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
        assert_eq!(
            errors,
            vec![
                MeshNormalVectorError::ExpectationMismatch {
                    face_idx: 0_usize.into()
                },
                MeshNormalVectorError::ZeroAreaTriangle {
                    face_idx: 1_usize.into()
                },
            ]
        );
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
    use crate::errors::{FaceIndex, MeshManifoldError};
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
                let zero: FaceIndex = 0_usize.into();
                let one: FaceIndex = 1_usize.into();
                assert!(face_indices.contains(&zero) && face_indices.contains(&one));
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
