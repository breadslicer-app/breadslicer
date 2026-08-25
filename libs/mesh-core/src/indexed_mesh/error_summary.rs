/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Aggregation helpers for mesh-validation error lists.
//!
//! [`group_errors_by_variant`] folds a flat slice of validation
//! errors into per-variant buckets, preserving the order in which
//! each variant first appears. Consumers use this to produce
//! summaries of the form
//! `"N error(s)\n  M VariantName (e.g. faces #2, #5, ...)"`
//! without re-implementing the bucketing every time.
//!
//! The buckets are powered by the [`MeshErrorReport`] trait, which
//! exposes a variant name and an optional representative face index
//! for each error. The trait is implemented for
//! [`MeshVertexError`](super::MeshVertexError),
//! [`MeshFaceError`](super::MeshFaceError) and
//! [`MeshEdgeManifoldError`](super::MeshEdgeManifoldError); other validation
//! error enums can opt in by implementing it.

use crate::primitives::FaceIndex;

use super::{MeshEdgeManifoldError, MeshFaceError, MeshVertexError};

/// Surface a variant name and (where applicable) a representative
/// face index for aggregation into summaries.
pub trait MeshErrorReport {
    /// Stable identifier for the enum variant. Used as a grouping
    /// key and as the human-readable label in summaries.
    fn variant_name(&self) -> &'static str;

    /// Face index this error is anchored on, when the variant
    /// carries one. `None` for variants that do not refer to a
    /// specific face (for example, [`MeshEdgeManifoldError::EmptyMesh`]).
    fn example_face(&self) -> Option<FaceIndex>;
}

impl MeshErrorReport for MeshVertexError {
    fn variant_name(&self) -> &'static str {
        match self {
            Self::InfiniteCoordinate { .. } => "InfiniteCoordinate",
            Self::NaNCoordinate { .. } => "NaNCoordinate",
        }
    }

    /// Vertex errors anchor on a vertex, not a face; the summary's
    /// face-typed example hook has nothing to offer here. Per-vertex
    /// detail stays available through each error's `Display`.
    fn example_face(&self) -> Option<FaceIndex> {
        None
    }
}

impl MeshErrorReport for MeshFaceError {
    fn variant_name(&self) -> &'static str {
        match self {
            Self::OutOfRangeVertexIndex { .. } => "OutOfRangeVertexIndex",
            Self::ZeroLengthEdge { .. } => "ZeroLengthEdge",
        }
    }

    fn example_face(&self) -> Option<FaceIndex> {
        Some(match self {
            Self::OutOfRangeVertexIndex { face_idx, .. } => *face_idx,
            Self::ZeroLengthEdge { face_idx, .. } => *face_idx,
        })
    }
}

impl MeshErrorReport for MeshEdgeManifoldError {
    fn variant_name(&self) -> &'static str {
        match self {
            Self::EmptyMesh => "EmptyMesh",
            Self::BoundaryEdge { .. } => "BoundaryEdge",
            Self::NonManifoldEdge { .. } => "NonManifoldEdge",
            Self::InconsistentWindingOrder { .. } => "InconsistentWindingOrder",
        }
    }

    fn example_face(&self) -> Option<FaceIndex> {
        match self {
            Self::EmptyMesh => None,
            Self::BoundaryEdge { face_idx, .. } => Some(*face_idx),
            Self::NonManifoldEdge { face_indices, .. } => face_indices.first().copied(),
            Self::InconsistentWindingOrder { face_indices, .. } => Some(face_indices[0]),
        }
    }
}

/// One variant's slice of an error list, in input order.
#[derive(Debug)]
pub struct ErrorVariantGroup<'a, E> {
    pub variant_name: &'static str,
    pub members: Vec<&'a E>,
}

/// Group `errors` by variant name. The returned groups appear in the
/// order each variant first occurs in `errors`; members within a
/// group also preserve their input order.
pub fn group_errors_by_variant<E: MeshErrorReport>(errors: &[E]) -> Vec<ErrorVariantGroup<'_, E>> {
    let mut groups: Vec<ErrorVariantGroup<'_, E>> = Vec::new();
    for err in errors {
        let name = err.variant_name();
        if let Some(group) = groups.iter_mut().find(|g| g.variant_name == name) {
            group.members.push(err);
        } else {
            groups.push(ErrorVariantGroup {
                variant_name: name,
                members: vec![err],
            });
        }
    }
    groups
}

#[cfg(test)]
mod variant_name_tests {
    use crate::primitives::{TriVertexSlot, VertexIndex};

    use super::*;

    #[test]
    fn mesh_vertex_error_variant_names_round_trip() {
        let infinite = MeshVertexError::InfiniteCoordinate {
            vertex_idx: 0,
            coord: crate::primitives::CoordAxis::X,
        };
        let nan = MeshVertexError::NaNCoordinate {
            vertex_idx: 0,
            coord: crate::primitives::CoordAxis::Z,
        };
        assert_eq!(infinite.variant_name(), "InfiniteCoordinate");
        assert_eq!(nan.variant_name(), "NaNCoordinate");
    }

    #[test]
    fn mesh_face_error_variant_names_round_trip() {
        let out_of_range = MeshFaceError::OutOfRangeVertexIndex {
            face_idx: 0_usize.into(),
            vertex_pos: TriVertexSlot::V0,
            vertex_idx: VertexIndex::from(0_u32),
        };
        let zle = MeshFaceError::ZeroLengthEdge {
            face_idx: 0_usize.into(),
            vertex_pos_a: TriVertexSlot::V0,
            vertex_pos_b: TriVertexSlot::V1,
            vertex_idx: VertexIndex::from(0_u32),
        };
        assert_eq!(out_of_range.variant_name(), "OutOfRangeVertexIndex");
        assert_eq!(zle.variant_name(), "ZeroLengthEdge");
    }

    #[test]
    fn mesh_edge_manifold_error_variant_names_round_trip() {
        let empty = MeshEdgeManifoldError::EmptyMesh;
        let boundary = MeshEdgeManifoldError::BoundaryEdge {
            edge: (VertexIndex::from(0_u32), VertexIndex::from(1_u32)),
            face_idx: 0_usize.into(),
        };
        let non_manifold = MeshEdgeManifoldError::NonManifoldEdge {
            edge: (VertexIndex::from(0_u32), VertexIndex::from(1_u32)),
            face_indices: vec![0_usize.into(), 1_usize.into(), 2_usize.into()],
        };
        let winding = MeshEdgeManifoldError::InconsistentWindingOrder {
            edge: (VertexIndex::from(0_u32), VertexIndex::from(1_u32)),
            face_indices: [0_usize.into(), 1_usize.into()],
        };
        assert_eq!(empty.variant_name(), "EmptyMesh");
        assert_eq!(boundary.variant_name(), "BoundaryEdge");
        assert_eq!(non_manifold.variant_name(), "NonManifoldEdge");
        assert_eq!(winding.variant_name(), "InconsistentWindingOrder");
    }
}

#[cfg(test)]
mod example_face_tests {
    use crate::primitives::{TriVertexSlot, VertexIndex};

    use super::*;

    #[test]
    fn mesh_vertex_errors_have_no_example_face() {
        let infinite = MeshVertexError::InfiniteCoordinate {
            vertex_idx: 7,
            coord: crate::primitives::CoordAxis::Y,
        };
        let nan = MeshVertexError::NaNCoordinate {
            vertex_idx: 7,
            coord: crate::primitives::CoordAxis::Y,
        };
        assert_eq!(infinite.example_face(), None);
        assert_eq!(nan.example_face(), None);
    }

    #[test]
    fn mesh_face_error_example_face_is_the_face_index() {
        let err = MeshFaceError::ZeroLengthEdge {
            face_idx: 42_usize.into(),
            vertex_pos_a: TriVertexSlot::V0,
            vertex_pos_b: TriVertexSlot::V1,
            vertex_idx: VertexIndex::from(0_u32),
        };
        assert_eq!(err.example_face(), Some(42_usize.into()));
    }

    #[test]
    fn manifold_empty_mesh_has_no_example_face() {
        assert_eq!(MeshEdgeManifoldError::EmptyMesh.example_face(), None);
    }

    #[test]
    fn manifold_non_manifold_edge_returns_first_face_index() {
        let err = MeshEdgeManifoldError::NonManifoldEdge {
            edge: (VertexIndex::from(0_u32), VertexIndex::from(1_u32)),
            face_indices: vec![17_usize.into(), 18_usize.into(), 19_usize.into()],
        };
        assert_eq!(err.example_face(), Some(17_usize.into()));
    }

    #[test]
    fn manifold_inconsistent_winding_returns_first_face_index() {
        let err = MeshEdgeManifoldError::InconsistentWindingOrder {
            edge: (VertexIndex::from(0_u32), VertexIndex::from(1_u32)),
            face_indices: [42_usize.into(), 43_usize.into()],
        };
        assert_eq!(err.example_face(), Some(42_usize.into()));
    }
}

#[cfg(test)]
mod group_errors_by_variant_tests {
    use crate::primitives::{TriVertexSlot, VertexIndex};

    use super::*;

    fn zle(face: usize) -> MeshFaceError {
        MeshFaceError::ZeroLengthEdge {
            face_idx: face.into(),
            vertex_pos_a: TriVertexSlot::V0,
            vertex_pos_b: TriVertexSlot::V1,
            vertex_idx: VertexIndex::from(0_u32),
        }
    }

    fn oor(face: usize) -> MeshFaceError {
        MeshFaceError::OutOfRangeVertexIndex {
            face_idx: face.into(),
            vertex_pos: TriVertexSlot::V0,
            vertex_idx: VertexIndex::from(999_u32),
        }
    }

    #[test]
    fn empty_input_returns_no_groups() {
        let groups = group_errors_by_variant::<MeshFaceError>(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn single_variant_collapses_into_one_group() {
        let errors = vec![zle(0), zle(2), zle(5)];
        let groups = group_errors_by_variant(&errors);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].variant_name, "ZeroLengthEdge");
        assert_eq!(groups[0].members.len(), 3);
    }

    #[test]
    fn group_order_follows_first_occurrence_of_each_variant() {
        let errors = vec![zle(0), oor(1), zle(2), oor(3), zle(4)];
        let groups = group_errors_by_variant(&errors);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].variant_name, "ZeroLengthEdge");
        assert_eq!(groups[1].variant_name, "OutOfRangeVertexIndex");
        assert_eq!(groups[0].members.len(), 3);
        assert_eq!(groups[1].members.len(), 2);
    }

    #[test]
    fn members_within_a_group_preserve_input_order() {
        let errors = vec![zle(7), oor(1), zle(3), zle(9)];
        let groups = group_errors_by_variant(&errors);
        let zle_group = groups
            .iter()
            .find(|g| g.variant_name == "ZeroLengthEdge")
            .unwrap();
        let face_indices: Vec<_> = zle_group
            .members
            .iter()
            .filter_map(|e| e.example_face())
            .collect();
        assert_eq!(
            face_indices,
            vec![7_usize.into(), 3_usize.into(), 9_usize.into()],
        );
    }
}
