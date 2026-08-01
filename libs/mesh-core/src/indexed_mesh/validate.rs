/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fmt;

use crate::primitives::{CoordAxis, FaceIndex, TriVertexSlot, VertexIndex};

use super::IndexedMesh;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshVertexError {
    InfiniteCoordinate { vertex_idx: usize, coord: CoordAxis },
    NaNCoordinate { vertex_idx: usize, coord: CoordAxis },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshFaceError {
    OutOfRangeVertexIndex {
        /// Index for the problematic face
        face_idx: FaceIndex,
        /// Which corner of the triangle holds the offending index.
        vertex_pos: TriVertexSlot,
        /// Value of the problematic vertex index, as found in the face.
        vertex_idx: VertexIndex,
    },
    ZeroLengthEdge {
        /// Index for the problematic face
        face_idx: FaceIndex,
        /// One of the two corners that share the same vertex.
        vertex_pos_a: TriVertexSlot,
        /// The other corner that shares the same vertex.
        vertex_pos_b: TriVertexSlot,
        /// The shared vertex index.
        vertex_idx: VertexIndex,
    },
}

impl fmt::Display for MeshVertexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InfiniteCoordinate { vertex_idx, coord } => {
                write!(f, "vertex #{vertex_idx} has an infinite {coord} coordinate",)
            }
            Self::NaNCoordinate { vertex_idx, coord } => {
                write!(f, "vertex #{vertex_idx} has a NaN {coord} coordinate",)
            }
        }
    }
}

impl fmt::Display for MeshFaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRangeVertexIndex {
                face_idx,
                vertex_pos,
                vertex_idx,
            } => write!(
                f,
                "face #{face_idx} slot {vertex_pos} references vertex #{vertex_idx}, which is out of range",
            ),
            Self::ZeroLengthEdge {
                face_idx,
                vertex_pos_a,
                vertex_pos_b,
                vertex_idx,
            } => write!(
                f,
                "face #{face_idx} has a zero-length edge: slots {vertex_pos_a} and {vertex_pos_b} both bound to vertex #{vertex_idx}",
            ),
        }
    }
}

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
                if usize::from(vertex_idx) >= num_vertices {
                    errors.push(MeshFaceError::OutOfRangeVertexIndex {
                        face_idx,
                        vertex_pos,
                        vertex_idx,
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
                        vertex_idx: a,
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
}

#[cfg(test)]
mod indexed_mesh_validate_vertices_tests {
    use super::*;
    use crate::primitives::CoordAxis;

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
    use crate::primitives::{Normal, TriVertexSlot};

    fn make_face(v0: u32, v1: u32, v2: u32) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [v0, v1, v2])
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
        // All three vertices are the same -> all three edges are collapsed
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
    use crate::indexed_mesh::validated::MeshNormalVectorError;
    use crate::primitives::Normal;

    fn make_mesh(vertices: Vec<Vertex>, faces: Vec<IndexedTriangle>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn face(normal: [f32; 3], v: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(Normal::new(normal), v)
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
        assert!(
            mesh.validated_faces()
                .unwrap()
                .validate_normal_vectors()
                .is_ok()
        );
    }

    #[test]
    fn ok_for_empty_faces() {
        let mesh = make_mesh(xy_triangle_vertices(), vec![]);
        assert!(
            mesh.validated_faces()
                .unwrap()
                .validate_normal_vectors()
                .is_ok()
        );
    }

    #[test]
    fn ok_for_small_but_valid_triangle() {
        // Sub-millimetre detail (~1 µm edges). Pre-fix this would have
        // been flagged ZeroAreaTriangle by the absolute-area threshold
        // an earlier version of the validator used.
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1e-3, 0.0, 0.0]),
            Vertex::new([0.0, 1e-3, 0.0]),
        ];
        let mesh = make_mesh(vertices, vec![face([0.0, 0.0, 1.0], [0, 1, 2])]);
        assert!(
            mesh.validated_faces()
                .unwrap()
                .validate_normal_vectors()
                .is_ok()
        );
    }

    #[test]
    fn detects_zero_area_triangle_collinear_vertices() {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([2.0, 0.0, 0.0]), // collinear
        ];
        let mesh = make_mesh(vertices, vec![face([0.0, 0.0, 1.0], [0, 1, 2])]);
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            MeshNormalVectorError::NonUnitNorm { face_idx, norm: _ } => {
                assert_eq!(*face_idx, 0_usize.into());
            }
            other => panic!("expected NonUnitNorm, got {:?}", other),
        }
    }

    #[test]
    fn accepts_normal_just_inside_unit_norm_tolerance() {
        // n_mag_sq = 0.9999998² ≈ 0.99999960 in f64 after f32 round-trip.
        // Deviation from 1.0 is ~4e-7, comfortably inside the 1e-6
        // tolerance. The normal is Z-aligned so the direction check
        // following the magnitude check also passes.
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, 0.999_999_8], [0, 1, 2])],
        );
        assert!(
            mesh.validated_faces()
                .unwrap()
                .validate_normal_vectors()
                .is_ok()
        );
    }

    #[test]
    fn rejects_normal_just_outside_unit_norm_tolerance_below() {
        // n_mag_sq = 0.999998² ≈ 0.999996. Deviation ~4e-6, four times
        // the 1e-6 tolerance: must fire NonUnitNorm without falling
        // through to the direction check.
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, 0.999_998], [0, 1, 2])],
        );
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            MeshNormalVectorError::NonUnitNorm { face_idx, norm: _ } => {
                assert_eq!(*face_idx, 0_usize.into());
            }
            other => panic!("expected NonUnitNorm, got {:?}", other),
        }
    }

    #[test]
    fn rejects_normal_just_outside_unit_norm_tolerance_above() {
        // n_mag_sq = 1.000002² ≈ 1.000004. Same ~4e-6 deviation as
        // the below case but on the other side; pins that the
        // tolerance check is symmetric.
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, 1.000_002], [0, 1, 2])],
        );
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            MeshNormalVectorError::NonUnitNorm { face_idx, norm: _ } => {
                assert_eq!(*face_idx, 0_usize.into());
            }
            other => panic!("expected NonUnitNorm, got {:?}", other),
        }
    }

    #[test]
    fn rejects_nan_magnitude_normal() {
        // NaN propagates through the squared-norm sum and lands at
        // n_mag_sq = NaN. `approx_eq` returns false for any NaN
        // operand, so the magnitude check rejects without ever
        // reaching the direction check.
        let mesh = make_mesh(
            xy_triangle_vertices(),
            vec![face([0.0, 0.0, f32::NAN], [0, 1, 2])],
        );
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_normal_vectors()
            .unwrap_err();
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
        assert!(
            mesh.validated_faces()
                .unwrap()
                .validate_normal_vectors()
                .is_ok()
        );
    }
}

#[cfg(test)]
mod indexed_mesh_validate_edge_manifold_tests {
    use super::*;
    use crate::indexed_mesh::validated::MeshEdgeManifoldError;
    use crate::primitives::{FaceIndex, Normal, VertexIndex};

    fn face(v: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), v)
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
                face([0, 1, 2]), // edges: 0->1, 1->2, 2->0
                face([0, 3, 1]), // edges: 0->3, 3->1, 1->0
                face([0, 2, 3]), // edges: 0->2, 2->3, 3->0
                face([1, 3, 2]), // edges: 1->3, 3->2, 2->1
            ],
        }
    }

    #[test]
    fn ok_for_closed_manifold_tetrahedron() {
        assert!(
            tetrahedron()
                .validated_faces()
                .unwrap()
                .validate_edge_manifold()
                .is_ok()
        );
    }

    #[test]
    fn rejects_empty_mesh() {
        let mesh = make_mesh(0, vec![]);
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], MeshEdgeManifoldError::EmptyMesh));
    }

    #[test]
    fn detects_boundary_edge_single_triangle() {
        // A single triangle has 3 boundary edges (each edge belongs to only one face)
        let mesh = make_mesh(3, vec![face([0, 1, 2])]);
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        assert_eq!(errors.len(), 3);
        assert!(
            errors
                .iter()
                .all(|e| matches!(e, MeshEdgeManifoldError::BoundaryEdge { .. }))
        );
    }

    #[test]
    fn detects_boundary_edge_open_surface() {
        // Two triangles sharing one edge, leaving 4 boundary edges
        // Face 0: 0->1, 1->2, 2->0
        // Face 1: 1->0, 0->3, 3->1
        // Edge (0,1) is shared with consistent winding (0->1 and 1->0) : OK
        // Edges 1->2, 2->0, 0->3, 3->1 are boundary
        let mesh = make_mesh(4, vec![face([0, 1, 2]), face([1, 0, 3])]);
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        let boundary_count = errors
            .iter()
            .filter(|e| matches!(e, MeshEdgeManifoldError::BoundaryEdge { .. }))
            .count();
        assert_eq!(boundary_count, 4);
    }

    #[test]
    fn detects_inconsistent_winding_order() {
        // Two triangles sharing edge (0,1) but both traverse it as 0->1
        // Face 0: 0->1, 1->2, 2->0
        // Face 1: 0->1, 1->3, 3->0
        // Edge (0,1) appears twice in the same direction -> inconsistent winding
        let mesh = make_mesh(4, vec![face([0, 1, 2]), face([0, 1, 3])]);
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        let winding_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, MeshEdgeManifoldError::InconsistentWindingOrder { .. }))
            .collect();
        assert_eq!(winding_errors.len(), 1);
        match &winding_errors[0] {
            MeshEdgeManifoldError::InconsistentWindingOrder { edge, face_indices } => {
                assert_eq!(*edge, (VertexIndex::from(0_u32), VertexIndex::from(1_u32)));
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
        // Face 0: 0->1, 1->2, 2->0
        // Face 1: 1->0, 0->3, 3->1
        // Face 2: 0->1, 1->4, 4->0
        // Edge (0,1): forward [0, 2], reverse [1] -> total 3 faces
        let mesh = make_mesh(5, vec![face([0, 1, 2]), face([1, 0, 3]), face([0, 1, 4])]);
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        let non_manifold_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, MeshEdgeManifoldError::NonManifoldEdge { .. }))
            .collect();
        assert!(
            !non_manifold_errors.is_empty(),
            "expected at least one NonManifoldEdge error"
        );
        let has_edge_01 = non_manifold_errors.iter().any(|e| match e {
            MeshEdgeManifoldError::NonManifoldEdge { edge, face_indices } => {
                *edge == (VertexIndex::from(0_u32), VertexIndex::from(1_u32))
                    && face_indices.len() == 3
            }
            _ => false,
        });
        assert!(has_edge_01, "expected NonManifoldEdge for edge (0, 1)");
    }

    #[test]
    fn tetrahedron_with_one_flipped_face_detects_inconsistent_winding() {
        // Take a valid tetrahedron and flip one face's winding
        let mut mesh = tetrahedron();
        // Flip face 0: [0, 1, 2] -> [0, 2, 1]
        // This reverses edges 0->1 to 0->2, 1->2 to 2->1, 2->0 to 1->0
        mesh.faces[0].vertices = [0, 2, 1].map(VertexIndex::from);

        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        let winding_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, MeshEdgeManifoldError::InconsistentWindingOrder { .. }))
            .collect();
        // Flipping one face of a tetrahedron causes inconsistent winding on all 3 of its edges
        assert_eq!(winding_errors.len(), 3);
    }

    #[test]
    fn reports_multiple_error_types() {
        // Face 0 and Face 1 share edge (0,1) with inconsistent winding (both 0->1),
        // plus all other edges are boundary.
        let mesh = make_mesh(4, vec![face([0, 1, 2]), face([0, 1, 3])]);
        let errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        let has_winding = errors
            .iter()
            .any(|e| matches!(e, MeshEdgeManifoldError::InconsistentWindingOrder { .. }));
        let has_boundary = errors
            .iter()
            .any(|e| matches!(e, MeshEdgeManifoldError::BoundaryEdge { .. }));
        assert!(has_winding, "expected InconsistentWindingOrder errors");
        assert!(has_boundary, "expected BoundaryEdge errors");
    }
}

#[cfg(test)]
mod mesh_vertex_error_display_tests {
    use super::*;

    #[test]
    fn infinite_coordinate_renders_index_and_axis() {
        let msg = format!(
            "{}",
            MeshVertexError::InfiniteCoordinate {
                vertex_idx: 42,
                coord: CoordAxis::Z,
            }
        );
        assert_eq!(msg, "vertex #42 has an infinite Z coordinate");
    }

    #[test]
    fn nan_coordinate_renders_index_and_axis() {
        let msg = format!(
            "{}",
            MeshVertexError::NaNCoordinate {
                vertex_idx: 7,
                coord: CoordAxis::X,
            }
        );
        assert_eq!(msg, "vertex #7 has a NaN X coordinate");
    }
}

#[cfg(test)]
mod mesh_face_error_display_tests {
    use super::*;

    #[test]
    fn out_of_range_renders_face_slot_and_target_vertex() {
        let msg = format!(
            "{}",
            MeshFaceError::OutOfRangeVertexIndex {
                face_idx: 17_usize.into(),
                vertex_pos: TriVertexSlot::V1,
                vertex_idx: 999_u32.into(),
            }
        );
        assert_eq!(
            msg,
            "face #17 slot v1 references vertex #999, which is out of range",
        );
    }

    #[test]
    fn zero_length_edge_renders_face_two_slots_and_shared_vertex() {
        let msg = format!(
            "{}",
            MeshFaceError::ZeroLengthEdge {
                face_idx: 2_usize.into(),
                vertex_pos_a: TriVertexSlot::V0,
                vertex_pos_b: TriVertexSlot::V1,
                vertex_idx: 1_u32.into(),
            }
        );
        assert_eq!(
            msg,
            "face #2 has a zero-length edge: slots v0 and v1 both bound to vertex #1",
        );
    }
}

#[cfg(test)]
mod manifold_sort_equivalence_tests {
    use std::collections::{HashMap, HashSet};

    use proptest::prelude::*;

    use super::*;
    use crate::indexed_mesh::validated::MeshEdgeManifoldError;
    use crate::primitives::{FaceIndex, Normal, VertexIndex};

    fn face(v: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), v)
    }

    fn make_mesh(num_vertices: usize, faces: Vec<IndexedTriangle>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0]); num_vertices],
            faces,
        }
    }

    fn vi(i: u32) -> VertexIndex {
        VertexIndex::from(i)
    }

    /// The previous hash-based validator, kept verbatim as the
    /// reference the sort-based implementation must agree with (up to
    /// the error order, which the reference never fixed).
    fn reference_validate_edge_manifold(
        mesh: &IndexedMesh,
    ) -> Result<(), Vec<MeshEdgeManifoldError>> {
        let mut errors: Vec<MeshEdgeManifoldError> = vec![];

        let mut half_edges: HashMap<(VertexIndex, VertexIndex), Vec<FaceIndex>> = HashMap::new();

        if mesh.faces.is_empty() {
            errors.push(MeshEdgeManifoldError::EmptyMesh);
            return Err(errors);
        }

        for (idx, mesh_face) in mesh.faces.iter().enumerate() {
            let face_idx = FaceIndex::from(idx);
            let [v0, v1, v2] = mesh_face.vertices;
            for &(a, b) in &[(v0, v1), (v1, v2), (v2, v0)] {
                half_edges.entry((a, b)).or_default().push(face_idx);
            }
        }

        let mut checked: HashSet<(VertexIndex, VertexIndex)> = HashSet::new();

        for &(a, b) in half_edges.keys() {
            let canonical = if a <= b { (a, b) } else { (b, a) };
            if !checked.insert(canonical) {
                continue;
            }

            let forward = half_edges.get(&(a, b)).cloned().unwrap_or_default();
            let reverse = half_edges.get(&(b, a)).cloned().unwrap_or_default();

            let total = forward.len() + reverse.len();

            if total < 2 {
                let face_idx = forward.first().or(reverse.first()).copied().unwrap();
                errors.push(MeshEdgeManifoldError::BoundaryEdge {
                    edge: canonical,
                    face_idx,
                });
            } else if total > 2 {
                let mut face_indices = forward.clone();
                face_indices.extend(&reverse);
                errors.push(MeshEdgeManifoldError::NonManifoldEdge {
                    edge: canonical,
                    face_indices,
                });
            } else if forward.len() == 2 {
                errors.push(MeshEdgeManifoldError::InconsistentWindingOrder {
                    edge: canonical,
                    face_indices: [forward[0], forward[1]],
                });
            } else if reverse.len() == 2 {
                errors.push(MeshEdgeManifoldError::InconsistentWindingOrder {
                    edge: canonical,
                    face_indices: [reverse[0], reverse[1]],
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Total order over errors so both validators' outputs can be
    /// compared as sets: variant rank, canonical edge, then the face
    /// list sorted (the implementations may legitimately disagree on
    /// face order inside one error).
    fn sort_key(e: &MeshEdgeManifoldError) -> (u8, u32, u32, Vec<usize>) {
        match e {
            MeshEdgeManifoldError::EmptyMesh => (0, 0, 0, vec![]),
            MeshEdgeManifoldError::BoundaryEdge { edge, face_idx } => {
                (1, edge.0.get(), edge.1.get(), vec![face_idx.get()])
            }
            MeshEdgeManifoldError::NonManifoldEdge { edge, face_indices } => {
                let mut faces: Vec<usize> = face_indices.iter().map(|f| f.get()).collect();
                faces.sort_unstable();
                (2, edge.0.get(), edge.1.get(), faces)
            }
            MeshEdgeManifoldError::InconsistentWindingOrder { edge, face_indices } => {
                let mut faces = vec![face_indices[0].get(), face_indices[1].get()];
                faces.sort_unstable();
                (3, edge.0.get(), edge.1.get(), faces)
            }
        }
    }

    fn normalized(
        result: Result<(), Vec<MeshEdgeManifoldError>>,
    ) -> Vec<(u8, u32, u32, Vec<usize>)> {
        match result {
            Ok(()) => vec![],
            Err(errors) => {
                let mut keys: Vec<_> = errors.iter().map(sort_key).collect();
                keys.sort();
                keys
            }
        }
    }

    /// Closed grid torus: an n x m wrap-around lattice, two triangles
    /// per cell, consistent winding. The manifold check only reads
    /// face indices, so vertex placement is a trivial lattice.
    fn torus_mesh(n: usize, m: usize) -> IndexedMesh {
        let mut vertices = Vec::with_capacity(n * m);
        for i in 0..n {
            for j in 0..m {
                vertices.push(Vertex::new([i as f32, j as f32, (i + j) as f32]));
            }
        }
        let idx = |i: usize, j: usize| -> u32 { (i * m + j) as u32 };
        let mut faces = Vec::with_capacity(2 * n * m);
        for i in 0..n {
            let i1 = (i + 1) % n;
            for j in 0..m {
                let j1 = (j + 1) % m;
                faces.push(face([idx(i, j), idx(i1, j), idx(i1, j1)]));
                faces.push(face([idx(i, j), idx(i1, j1), idx(i, j1)]));
            }
        }
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn validate(mesh: &IndexedMesh) -> Result<(), Vec<MeshEdgeManifoldError>> {
        mesh.validated_faces()
            .expect("test meshes keep faces in range")
            .validate_edge_manifold()
    }

    proptest! {
        #[test]
        fn agrees_with_the_reference_on_valid_tori(n in 3usize..8, m in 3usize..8) {
            let mesh = torus_mesh(n, m);
            prop_assert!(validate(&mesh).is_ok());
            prop_assert!(reference_validate_edge_manifold(&mesh).is_ok());
        }

        #[test]
        fn agrees_with_the_reference_on_mutated_tori(
            n in 3usize..8,
            m in 3usize..8,
            mutation in 0u8..3,
            pick in any::<prop::sample::Index>(),
        ) {
            let mut mesh = torus_mesh(n, m);
            let f = pick.index(mesh.faces.len());
            match mutation {
                // Boundary edges appear where the face is missing.
                0 => {
                    mesh.faces.remove(f);
                }
                // The duplicated face over-shares all three edges.
                1 => {
                    let dup = mesh.faces[f];
                    mesh.faces.push(dup);
                }
                // The flipped face traverses its edges backwards.
                _ => {
                    let [a, b, c] = mesh.faces[f].vertices;
                    mesh.faces[f].vertices = [a, c, b];
                }
            }
            let ours = normalized(validate(&mesh));
            let reference = normalized(reference_validate_edge_manifold(&mesh));
            prop_assert!(!ours.is_empty(), "mutation must produce errors");
            prop_assert_eq!(ours, reference);
        }
    }

    #[test]
    fn errors_are_sorted_by_canonical_edge_and_deterministic() {
        // A single triangle: three boundary edges, reported in
        // canonical edge order.
        let mesh = make_mesh(3, vec![face([0, 1, 2])]);
        let errors = validate(&mesh).unwrap_err();
        let edges: Vec<_> = errors
            .iter()
            .map(|e| match e {
                MeshEdgeManifoldError::BoundaryEdge { edge, .. } => *edge,
                other => panic!("expected BoundaryEdge, got {other:?}"),
            })
            .collect();
        assert_eq!(edges, vec![(vi(0), vi(1)), (vi(0), vi(2)), (vi(1), vi(2))]);
        assert_eq!(errors, validate(&mesh).unwrap_err());
    }

    #[test]
    fn winding_error_lists_faces_ascending() {
        let mesh = make_mesh(4, vec![face([0, 1, 2]), face([0, 1, 3])]);
        let errors = validate(&mesh).unwrap_err();
        let winding = errors
            .iter()
            .find_map(|e| match e {
                MeshEdgeManifoldError::InconsistentWindingOrder { face_indices, .. } => {
                    Some(*face_indices)
                }
                _ => None,
            })
            .expect("winding error present");
        assert_eq!(
            winding,
            [FaceIndex::from(0_usize), FaceIndex::from(1_usize)]
        );
    }

    #[test]
    fn non_manifold_edge_lists_faces_ascending() {
        // Faces 0, 1, 2 all share edge (0, 1); the old validator
        // listed them grouped by direction, the sorted one ascending.
        let mesh = make_mesh(5, vec![face([0, 1, 2]), face([1, 0, 3]), face([0, 1, 4])]);
        let errors = validate(&mesh).unwrap_err();
        let shared = errors
            .iter()
            .find_map(|e| match e {
                MeshEdgeManifoldError::NonManifoldEdge { edge, face_indices }
                    if *edge == (vi(0), vi(1)) =>
                {
                    Some(face_indices.clone())
                }
                _ => None,
            })
            .expect("non-manifold edge present");
        let ascending: Vec<FaceIndex> = (0..3usize).map(FaceIndex::from).collect();
        assert_eq!(shared, ascending);
    }
}
