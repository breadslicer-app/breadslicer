/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::collections::HashMap;
use std::fmt;

use crate::primitives::FaceIndex;

use super::mesh::IndexedMesh;
use super::validate::MeshFaceError;
use super::validated::MeshEdgeManifoldError;

#[cfg(test)]
use crate::primitives::{IndexedTriangle, Vertex, VertexIndex};

/// Records of the repair operations that a fix routine applied to a mesh. The
/// routine changes the mesh in place and returns these records, so that callers
/// can show a summary: the CLI prints it, the GUI shows it in a dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum MeshFix {
    /// A face was removed because it had a zero-length edge.
    RemovedZeroLengthEdgeFace { face_idx: FaceIndex },
    /// A face's winding order was flipped (vertices reversed).
    FlippedFaceOrientation { face_idx: FaceIndex },
}

/// Failure modes of `fix_faces_orientation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshFixError {
    /// The input manifold-error set names this face in more than 3
    /// `InconsistentWindingOrder` entries. A triangle has only 3 edges, so the
    /// input is self-inconsistent.
    FaceWithTooManyWindingErrors { face_idx: FaceIndex },
    /// At least half of the mesh's faces, rounded down, would have to be
    /// flipped. The routine takes this to mean that the whole mesh is inside
    /// out, and refuses the per-face fix.
    TooManyFlippedFaces,
}

impl fmt::Display for MeshFixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FaceWithTooManyWindingErrors { face_idx } => {
                write!(f, "face {face_idx} has more than 3 winding-order errors")
            }
            Self::TooManyFlippedFaces => f.write_str("too many flipped faces"),
        }
    }
}

impl std::error::Error for MeshFixError {}

/// Plans the orientation fix, then applies it. On `Err` the mesh is unchanged.
pub(super) fn fix_faces_orientation_impl(
    mesh: &mut IndexedMesh,
    manifold_errors: &[MeshEdgeManifoldError],
) -> Result<Vec<MeshFix>, MeshFixError> {
    let faces_to_flip = plan_faces_orientation_impl(mesh, manifold_errors)?;
    Ok(apply_orientation_plan(mesh, faces_to_flip))
}

/// Returns the faces the orientation fix would flip, the ones named in 2 or 3
/// `InconsistentWindingOrder` entries, or the reason it would refuse.
pub(super) fn plan_faces_orientation_impl(
    mesh: &IndexedMesh,
    manifold_errors: &[MeshEdgeManifoldError],
) -> Result<Vec<FaceIndex>, MeshFixError> {
    let mut possibly_flipped_faces: HashMap<FaceIndex, u8> = HashMap::new();
    for error in manifold_errors {
        if let MeshEdgeManifoldError::InconsistentWindingOrder {
            edge: _,
            face_indices,
        } = error
        {
            *possibly_flipped_faces.entry(face_indices[0]).or_insert(0) += 1;
            *possibly_flipped_faces.entry(face_indices[1]).or_insert(0) += 1;
        }
    }

    let mut faces_to_flip: Vec<FaceIndex> = Vec::new();
    for (&face_idx, &n_errors) in &possibly_flipped_faces {
        match n_errors {
            2 | 3 => faces_to_flip.push(face_idx),
            4.. => return Err(MeshFixError::FaceWithTooManyWindingErrors { face_idx }),
            _ => {}
        }
    }

    if faces_to_flip.len() >= mesh.faces.len() / 2 {
        return Err(MeshFixError::TooManyFlippedFaces);
    }
    Ok(faces_to_flip)
}

fn apply_orientation_plan(mesh: &mut IndexedMesh, faces_to_flip: Vec<FaceIndex>) -> Vec<MeshFix> {
    faces_to_flip
        .into_iter()
        .map(|face_idx| {
            let face = &mut mesh.faces[face_idx.get()];
            face.vertices.reverse();
            MeshFix::FlippedFaceOrientation { face_idx }
        })
        .collect()
}

/// Removes from `mesh.faces` every face that a `ZeroLengthEdge` error in
/// `face_errors` names, and returns 1 fix per such error; other errors are
/// ignored. The remaining faces keep their vertex indices in range, since
/// `vertices.len()` does not change.
pub(super) fn fix_zero_length_edges_impl(
    mesh: &mut IndexedMesh,
    face_errors: &[MeshFaceError],
) -> Vec<MeshFix> {
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
        // `remove` is O(n); `swap_remove` is O(1) but changes the face order.
        mesh.faces.remove(face_idx.get());
    }

    fixes
}

#[cfg(test)]
mod indexed_mesh_fix_faces_orientation_tests {
    use super::*;

    /// Shortens `VertexIndex::from(n)`, to keep the dense
    /// `MeshEdgeManifoldError::*` literals readable.
    fn vi(n: u32) -> VertexIndex {
        VertexIndex::from(n)
    }

    /// Builds a tetrahedron with consistent winding.
    fn tetrahedron() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
            Vertex::new([-1.0, 1.0, -1.0]),
            Vertex::new([-1.0, -1.0, 1.0]),
        ];
        let faces_data: [[u32; 3]; 4] = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];

        let faces = faces_data
            .iter()
            .map(|indices| IndexedTriangle::from_raw_indices(*indices))
            .collect();

        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// Returns the tetrahedron with the winding of face 0 reversed.
    fn tetrahedron_with_face_0_inside_out() -> IndexedMesh {
        let mut mesh = tetrahedron();
        let face = &mut mesh.faces[0];
        face.vertices.reverse();
        mesh
    }

    #[test]
    fn no_op_when_no_errors() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();
        mesh.validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&[])
            .unwrap();
        assert_eq!(mesh, original);
    }

    #[test]
    fn fixes_a_single_flipped_face() {
        let mut mesh = tetrahedron_with_face_0_inside_out();

        // Gather errors as the real workflow would
        let manifold_errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();

        mesh.validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors)
            .unwrap();

        assert_eq!(mesh, tetrahedron());
    }

    #[test]
    fn does_not_modify_correct_faces() {
        let mut mesh = tetrahedron();

        mesh.faces[0].vertices.reverse();

        let expected = tetrahedron();

        let manifold_errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        mesh.validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors)
            .unwrap();

        for i in 1..4 {
            assert_eq!(
                mesh.faces[i].vertices, expected.faces[i].vertices,
                "face {} vertices changed unexpectedly",
                i
            );
        }
    }

    #[test]
    fn errors_when_face_has_more_than_3_winding_errors() {
        let mut mesh = tetrahedron();

        // Craft manifold errors where face 0 appears in 4
        // InconsistentWindingOrder entries
        let manifold_errors = vec![
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(0), vi(1)),
                face_indices: [0_usize.into(), 1_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(1), vi(2)),
                face_indices: [0_usize.into(), 2_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(2), vi(0)),
                face_indices: [0_usize.into(), 3_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(0), vi(3)),
                face_indices: [0_usize.into(), 1_usize.into()],
            },
        ];

        let result = mesh
            .validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors);
        assert!(matches!(
            result,
            Err(MeshFixError::FaceWithTooManyWindingErrors { face_idx }) if face_idx == FaceIndex::from(0_usize)
        ));
    }

    #[test]
    fn errors_when_too_many_faces_need_flipping() {
        // The errors below name each of the 4 faces 3 times, so 4 faces need
        // flipping, at or above the threshold of 4 / 2 = 2.
        let mut mesh = tetrahedron();

        let manifold_errors = vec![
            // Face 0: 3 errors
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(0), vi(1)),
                face_indices: [0_usize.into(), 1_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(1), vi(2)),
                face_indices: [0_usize.into(), 2_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(2), vi(0)),
                face_indices: [0_usize.into(), 3_usize.into()],
            },
            // Face 1: 2 more errors (already has 1 from above)
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(0), vi(3)),
                face_indices: [1_usize.into(), 2_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(3), vi(1)),
                face_indices: [1_usize.into(), 3_usize.into()],
            },
            // Face 2: 1 more error (already has 2 from above)
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(2), vi(3)),
                face_indices: [2_usize.into(), 3_usize.into()],
            },
        ];

        let result = mesh
            .validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors);
        assert!(matches!(result, Err(MeshFixError::TooManyFlippedFaces)));
    }

    #[test]
    fn ignores_non_winding_manifold_errors() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        let manifold_errors = vec![
            MeshEdgeManifoldError::BoundaryEdge {
                edge: (vi(0), vi(1)),
                face_idx: 0_usize.into(),
            },
            MeshEdgeManifoldError::NonManifoldEdge {
                edge: (vi(1), vi(2)),
                face_indices: vec![0_usize.into(), 1_usize.into(), 2_usize.into()],
            },
        ];

        mesh.validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors)
            .unwrap();
        assert_eq!(mesh, original);
    }

    #[test]
    fn face_with_one_winding_error_is_not_flipped() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        // Only 1 winding error for face 0, below threshold of 2
        let manifold_errors = vec![MeshEdgeManifoldError::InconsistentWindingOrder {
            edge: (vi(0), vi(1)),
            face_indices: [0_usize.into(), 1_usize.into()],
        }];

        mesh.validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors)
            .unwrap();
        assert_eq!(mesh.faces[0].vertices, original.faces[0].vertices);
    }

    #[test]
    fn mesh_is_valid_after_fixing_flipped_face() {
        let mut mesh = tetrahedron_with_face_0_inside_out();

        let manifold_errors = mesh
            .validated_faces()
            .unwrap()
            .validate_edge_manifold()
            .unwrap_err();
        mesh.validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors)
            .unwrap();

        // After the fix, the mesh passes the face, edge-manifold and face-area
        // validations.
        assert!(
            mesh.validated_faces()
                .unwrap()
                .validate_edge_manifold()
                .is_ok()
        );
        assert!(
            mesh.validated_faces()
                .unwrap()
                .validate_face_areas()
                .is_ok()
        );
    }

    /// Pins that the refusal of a face with more than 3 winding errors leaves
    /// the mesh unchanged.
    #[test]
    fn fix_faces_orientation_is_atomic_when_face_has_more_than_3_winding_errors() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        let manifold_errors = vec![
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(0), vi(1)),
                face_indices: [0_usize.into(), 1_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(1), vi(2)),
                face_indices: [0_usize.into(), 2_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(2), vi(0)),
                face_indices: [0_usize.into(), 3_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(0), vi(3)),
                face_indices: [0_usize.into(), 1_usize.into()],
            },
        ];

        let result = mesh
            .validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors);
        assert!(result.is_err());
        assert_eq!(mesh, original);
    }

    /// Pins that the refusal for too many flipped faces leaves the mesh
    /// unchanged.
    #[test]
    fn fix_faces_orientation_is_atomic_when_too_many_faces_need_flipping() {
        let mut mesh = tetrahedron();
        let original = mesh.clone();

        let manifold_errors = vec![
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(0), vi(1)),
                face_indices: [0_usize.into(), 1_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(1), vi(2)),
                face_indices: [0_usize.into(), 2_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(2), vi(0)),
                face_indices: [0_usize.into(), 3_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(0), vi(3)),
                face_indices: [1_usize.into(), 2_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(3), vi(1)),
                face_indices: [1_usize.into(), 3_usize.into()],
            },
            MeshEdgeManifoldError::InconsistentWindingOrder {
                edge: (vi(2), vi(3)),
                face_indices: [2_usize.into(), 3_usize.into()],
            },
        ];

        let result = mesh
            .validated_faces_mut()
            .unwrap()
            .fix_faces_orientation(&manifold_errors);
        assert!(result.is_err());
        assert_eq!(mesh, original);
    }
}

#[cfg(test)]
mod indexed_mesh_fix_zero_length_edges_tests {
    use super::*;
    use crate::primitives::TriVertexSlot;

    fn make_mesh(num_vertices: usize, faces: Vec<IndexedTriangle>) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0]); num_vertices],
            faces,
        }
    }

    fn face(v0: u32, v1: u32, v2: u32) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices([v0, v1, v2])
    }

    #[test]
    fn no_errors_means_no_fixes_and_no_face_removed() {
        let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
        let (fixes, _) = mesh.fix_zero_length_edges_into_validated(&[]);
        assert!(fixes.is_empty());
        assert_eq!(mesh.faces.len(), 1);
    }

    #[test]
    fn empty_face_list_with_unrelated_errors_is_a_no_op() {
        // Pass face errors that are *not* ZeroLengthEdge: the fix routine
        // should ignore them and leave the mesh untouched.
        let mut mesh = make_mesh(3, vec![face(0, 1, 2)]);
        let errors = vec![MeshFaceError::OutOfRangeVertexIndex {
            face_idx: 0_usize.into(),
            vertex_pos: TriVertexSlot::V0,
            vertex_idx: VertexIndex::from(99_u32),
        }];
        let (fixes, _) = mesh.fix_zero_length_edges_into_validated(&errors);
        assert!(fixes.is_empty());
        assert_eq!(mesh.faces.len(), 1);
    }

    #[test]
    fn single_collapsed_face_is_removed_with_a_fix_record() {
        let mut mesh = make_mesh(3, vec![face(0, 1, 1)]);
        let errors = mesh.validate_faces().unwrap_err();

        let (fixes, _) = mesh.fix_zero_length_edges_into_validated(&errors);

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
        // A degenerate triangle (all 3 vertices equal) emits 3 ZeroLengthEdge
        // errors for the same face. Dedup must reduce those to a single
        // removal, so face 0 is not removed 3 times (the second would panic).
        let mut mesh = make_mesh(3, vec![face(0, 0, 0)]);
        let errors = mesh.validate_faces().unwrap_err();
        assert_eq!(errors.len(), 3);

        let (fixes, _) = mesh.fix_zero_length_edges_into_validated(&errors);

        assert_eq!(mesh.faces.len(), 0);
        // 1 face was removed, but the fix log still mirrors the original error
        // list; caller can choose to deduplicate the report.
        let zero = MeshFix::RemovedZeroLengthEdgeFace {
            face_idx: 0_usize.into(),
        };
        assert_eq!(fixes, vec![zero.clone(), zero.clone(), zero]);
    }

    #[test]
    fn removes_in_descending_order_so_remaining_indices_stay_valid() {
        // 2 collapsed faces at positions 0 and 2; face 1 is healthy. The
        // implementation removes higher indices first, so index 2 still names
        // face 2 when it is removed.
        let mut mesh = make_mesh(4, vec![face(0, 1, 1), face(0, 1, 2), face(2, 3, 3)]);
        let errors = mesh.validate_faces().unwrap_err();

        let (fixes, _) = mesh.fix_zero_length_edges_into_validated(&errors);

        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.faces[0].vertices.map(u32::from), [0, 1, 2]);
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

        let (fixes, _) = mesh.fix_zero_length_edges_into_validated(&errors);

        // Only the collapsed face (index 1) was removed; the OutOfRange error
        // is irrelevant to this fix routine.
        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.faces[0].vertices.map(u32::from), [0, 1, 2]);
        assert!(
            fixes
                .iter()
                .all(|f| matches!(f, MeshFix::RemovedZeroLengthEdgeFace { .. }))
        );
    }
}
