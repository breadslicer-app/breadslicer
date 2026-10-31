/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

pub(super) mod collinear_faces;
pub(super) mod records;
pub(super) mod watertight;

use crate::primitives::FaceIndex;

use super::mesh::IndexedMesh;
use super::validate::MeshFaceError;

use records::MeshFix;

#[cfg(test)]
use crate::primitives::{IndexedTriangle, Vertex, VertexIndex};

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

/// Removes from `mesh.faces` every face that an error in `face_errors` names,
/// and returns the fixes: 1 per `ZeroLengthEdge` error, as
/// [`fix_zero_length_edges_impl`] gives them, and 1 per other face with an
/// `OutOfRangeVertexIndex` error. Every face that stays has its indices in
/// range and 3 distinct vertices when `face_errors` holds every face error of
/// the mesh.
pub(super) fn fix_invalid_faces_impl(
    mesh: &mut IndexedMesh,
    face_errors: &[MeshFaceError],
) -> Vec<MeshFix> {
    let mut fixes: Vec<MeshFix> = Vec::new();
    let mut remove = vec![false; mesh.faces.len()];
    for error in face_errors {
        if let MeshFaceError::ZeroLengthEdge { face_idx, .. } = error {
            fixes.push(MeshFix::RemovedZeroLengthEdgeFace {
                face_idx: *face_idx,
            });
            remove[face_idx.get()] = true;
        }
    }
    for error in face_errors {
        if let MeshFaceError::OutOfRangeVertexIndex { face_idx, .. } = error
            && !std::mem::replace(&mut remove[face_idx.get()], true)
        {
            fixes.push(MeshFix::RemovedOutOfRangeFace {
                face_idx: *face_idx,
            });
        }
    }
    let mut flags = remove.iter();
    mesh.faces
        .retain(|_| !*flags.next().expect("1 flag per face"));
    fixes
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
    fn errors_in_any_order_remove_the_faces_they_name() {
        let mut mesh = make_mesh(
            4,
            vec![
                face(0, 1, 2),
                face(0, 1, 1),
                face(0, 1, 3),
                face(2, 2, 3),
                face(1, 2, 3),
            ],
        );
        let mut errors = mesh.validate_faces().unwrap_err();
        errors.reverse();

        let fixes = fix_zero_length_edges_impl(&mut mesh, &errors);

        assert_eq!(
            mesh.faces,
            vec![face(0, 1, 2), face(0, 1, 3), face(1, 2, 3)]
        );
        assert_eq!(
            fixes,
            vec![
                MeshFix::RemovedZeroLengthEdgeFace {
                    face_idx: 3_usize.into()
                },
                MeshFix::RemovedZeroLengthEdgeFace {
                    face_idx: 1_usize.into()
                },
            ]
        );
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
