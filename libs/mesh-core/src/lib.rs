/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

pub mod errors;
pub mod geometry;
pub mod indexed_mesh;
pub mod primitives;
pub mod scene;

use std::collections::HashMap;

use anyhow::Result;

// Re-exporting "core" symbols for ease of use
pub use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
pub use crate::primitives::{IndexedTriangle, Normal, Triangle, Vector3D, Vertex};

pub trait TriangleIterator: Iterator<Item = Result<Triangle>> {
    fn as_indexed_mesh(&mut self, meta: Option<MeshMetadata>) -> Result<IndexedMesh> {
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
                let bitpattern = vertex.0.map(|f| {
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
        mesh.fix_faces_orientation(&[], &[], true).unwrap();
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
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true)
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
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true)
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
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, false)
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
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, false)
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
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true)
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

        let result = mesh.fix_faces_orientation(&manifold_errors, &[], true);
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

        let result = mesh.fix_faces_orientation(&manifold_errors, &[], true);
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

        mesh.fix_faces_orientation(&manifold_errors, &[], true)
            .unwrap();
        assert_eq!(mesh, original);
    }

    #[test]
    fn ignores_zero_area_triangle_normal_errors() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        let normal_errors = vec![MeshNormalVectorError::ZeroAreaTriangle { face_idx: 0 }];

        mesh.fix_faces_orientation(&[], &normal_errors, false)
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

        mesh.fix_faces_orientation(&manifold_errors, &[], true)
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

        // With fix_flipped_faces=false, winding must not be touched even though errors exist
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, false)
            .unwrap();

        assert_eq!(
            mesh.faces[0].vertices, flipped_vertices,
            "vertices should be unchanged when fix_flipped_faces is false"
        );
    }

    #[test]
    fn mesh_is_valid_after_fixing_flipped_face() {
        let mut mesh = tetrahedron();

        // Flip face 0's winding (keep original normal → mismatched)
        mesh.faces[0].vertices.reverse();

        let manifold_errors = mesh.validate_manifold().unwrap_err();
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();
        mesh.fix_faces_orientation(&manifold_errors, &normal_errors, true)
            .unwrap();

        // After fix, the mesh should pass all validations
        assert!(mesh.validate_manifold().is_ok());
        assert!(mesh.validate_normal_vectors().is_ok());
    }
}
