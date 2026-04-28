/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};

use crate::errors::{FaceIndex, MeshFaceError, MeshFix, MeshManifoldError, MeshNormalVectorError};
use crate::geometry::compute_normal_from_triangle_vertices;

use super::{FixFacesOptions, IndexedMesh};

#[cfg(test)]
use crate::primitives::{IndexedTriangle, Vertex};

impl IndexedMesh {
    pub fn fix_faces_orientation(
        &mut self,
        manifold_errors: &[MeshManifoldError],
        normal_vector_errors: &[MeshNormalVectorError],
        options: FixFacesOptions,
    ) -> Result<Vec<MeshFix>> {
        let mut fixes: Vec<MeshFix> = Vec::new();

        let mut possibly_flipped_faces: HashMap<FaceIndex, u8> = HashMap::new();

        if options.flipped {
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

        let mut faces_with_wrong_normals: HashSet<FaceIndex> = HashSet::new();
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

        if options.flipped {
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
                    let face = &mut self.faces[face_idx.get()];
                    face.vertices.reverse();
                    fixes.push(MeshFix::FlippedFaceOrientation {
                        face_idx: *face_idx,
                    });

                    if faces_with_wrong_normals.contains(face_idx) && options.normals {
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
                        face.normal = face.normal.negate();
                    }
                }
            }
        }

        if options.normals {
            // We fix remaining normal vectors after having flipped faces
            for face_idx in &faces_with_wrong_normals {
                let face = &mut self.faces[face_idx.get()];
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
        let mut faces_to_remove: Vec<FaceIndex> = face_errors
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
            self.faces.remove(face_idx.get());
        }

        fixes
    }
}

#[cfg(test)]
mod indexed_mesh_fix_faces_orientation_tests {
    use super::*;
    use crate::errors::{MeshManifoldError, MeshNormalVectorError};
    use crate::geometry::compute_normal_from_triangle_vertices;
    use crate::primitives::Normal;

    fn assert_normal_close(actual: Normal, expected: Normal, eps: f32) {
        for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            assert!(
                (a - e).abs() < eps,
                "normal component {i}: got {a}, expected {e}"
            );
        }
    }

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
        mesh.fix_faces_orientation(
            &[],
            &[],
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        )
        .unwrap();
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
        mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_errors,
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        )
        .unwrap();

        // Face 0 vertices should be restored (reversed back)
        assert_eq!(mesh.faces[0].vertices, expected.faces[0].vertices);

        // Normal should match the correct normal
        assert_normal_close(mesh.faces[0].normal, expected.faces[0].normal, 1e-5);
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
        mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_errors,
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        )
        .unwrap();

        assert_normal_close(mesh.faces[0].normal, expected.faces[0].normal, 1e-5);
    }

    #[test]
    fn fixes_wrong_normal_without_winding_error() {
        let mut mesh = tetrahedron();

        // Give face 0 a wrong normal but keep correct winding
        mesh.faces[0].normal = Normal::new([1.0, 0.0, 0.0]);

        let manifold_errors: Vec<MeshManifoldError> = vec![];
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        let expected = tetrahedron();
        mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_errors,
            FixFacesOptions {
                flipped: false,
                normals: true,
            },
        )
        .unwrap();

        assert_normal_close(mesh.faces[0].normal, expected.faces[0].normal, 1e-5);
    }

    #[test]
    fn fixes_non_unit_norm_without_winding_error() {
        let mut mesh = tetrahedron();

        // Scale the normal to non-unit length
        let correct = mesh.faces[1].normal.coords();
        mesh.faces[1].normal = Normal::new([correct[0] * 2.0, correct[1] * 2.0, correct[2] * 2.0]);

        let manifold_errors: Vec<MeshManifoldError> = vec![];
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();

        let expected = tetrahedron();
        mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_errors,
            FixFacesOptions {
                flipped: false,
                normals: true,
            },
        )
        .unwrap();

        assert_normal_close(mesh.faces[1].normal, expected.faces[1].normal, 1e-5);
    }

    #[test]
    fn does_not_modify_correct_faces() {
        let mut mesh = tetrahedron();

        // Flip only face 0's winding (keep original normal → mismatched)
        mesh.faces[0].vertices.reverse();

        let expected = tetrahedron();

        let manifold_errors = mesh.validate_manifold().unwrap_err();
        let normal_errors = mesh.validate_normal_vectors().unwrap_err();
        mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_errors,
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        )
        .unwrap();

        // Faces 1, 2, 3 should be unchanged
        for i in 1..4 {
            assert_eq!(
                mesh.faces[i].vertices, expected.faces[i].vertices,
                "face {} vertices changed unexpectedly",
                i
            );
            assert_normal_close(mesh.faces[i].normal, expected.faces[i].normal, 1e-5);
        }
    }

    #[test]
    fn errors_when_face_has_more_than_3_winding_errors() {
        let mut mesh = tetrahedron();

        // Craft manifold errors where face 0 appears in 4 InconsistentWindingOrder entries
        let manifold_errors = vec![
            MeshManifoldError::InconsistentWindingOrder {
                edge: (0, 1),
                face_indices: [0_usize.into(), 1_usize.into()],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (1, 2),
                face_indices: [0_usize.into(), 2_usize.into()],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (2, 0),
                face_indices: [0_usize.into(), 3_usize.into()],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (0, 3),
                face_indices: [0_usize.into(), 1_usize.into()],
            },
        ];

        let result = mesh.fix_faces_orientation(
            &manifold_errors,
            &[],
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        );
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
                face_indices: [0_usize.into(), 1_usize.into()],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (1, 2),
                face_indices: [0_usize.into(), 2_usize.into()],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (2, 0),
                face_indices: [0_usize.into(), 3_usize.into()],
            },
            // Face 1: 2 more errors (already has 1 from above)
            MeshManifoldError::InconsistentWindingOrder {
                edge: (0, 3),
                face_indices: [1_usize.into(), 2_usize.into()],
            },
            MeshManifoldError::InconsistentWindingOrder {
                edge: (3, 1),
                face_indices: [1_usize.into(), 3_usize.into()],
            },
            // Face 2: 1 more error (already has 2 from above)
            MeshManifoldError::InconsistentWindingOrder {
                edge: (2, 3),
                face_indices: [2_usize.into(), 3_usize.into()],
            },
        ];

        let result = mesh.fix_faces_orientation(
            &manifold_errors,
            &[],
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        );
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
                face_idx: 0_usize.into(),
            },
            MeshManifoldError::NonManifoldEdge {
                edge: (1, 2),
                face_indices: vec![0_usize.into(), 1_usize.into(), 2_usize.into()],
            },
        ];

        mesh.fix_faces_orientation(
            &manifold_errors,
            &[],
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        )
        .unwrap();
        assert_eq!(mesh, original);
    }

    #[test]
    fn ignores_zero_area_triangle_normal_errors() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        let normal_errors = vec![MeshNormalVectorError::ZeroAreaTriangle {
            face_idx: 0_usize.into(),
        }];

        mesh.fix_faces_orientation(
            &[],
            &normal_errors,
            FixFacesOptions {
                flipped: false,
                normals: false,
            },
        )
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
            face_indices: [0_usize.into(), 1_usize.into()],
        }];

        mesh.fix_faces_orientation(
            &manifold_errors,
            &[],
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        )
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
        mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_errors,
            FixFacesOptions {
                flipped: false,
                normals: true,
            },
        )
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
        mesh.fix_faces_orientation(
            &[],
            &normal_errors,
            FixFacesOptions {
                flipped: false,
                normals: false,
            },
        )
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
        mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_errors,
            FixFacesOptions {
                flipped: true,
                normals: false,
            },
        )
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
        mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_errors,
            FixFacesOptions {
                flipped: true,
                normals: true,
            },
        )
        .unwrap();

        // After fix, the mesh should pass all validations
        assert!(mesh.validate_manifold().is_ok());
        assert!(mesh.validate_normal_vectors().is_ok());
    }
}

#[cfg(test)]
mod indexed_mesh_fix_zero_length_edges_tests {
    use super::*;
    use crate::errors::{MeshFaceError, MeshFix, TriVertexSlot, VertexIndex};
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
            face_idx: 0_usize.into(),
            vertex_pos: TriVertexSlot::V0,
            vertex_idx: VertexIndex::from(99_u32),
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
        assert_eq!(
            fixes,
            vec![MeshFix::RemovedZeroLengthEdgeFace {
                face_idx: 0_usize.into()
            }]
        );
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
        let zero = MeshFix::RemovedZeroLengthEdgeFace {
            face_idx: 0_usize.into(),
        };
        assert_eq!(fixes, vec![zero.clone(), zero.clone(), zero]);
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
            face_idx: 0_usize.into(),
            vertex_pos: TriVertexSlot::V0,
            vertex_idx: VertexIndex::from(99_u32),
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
