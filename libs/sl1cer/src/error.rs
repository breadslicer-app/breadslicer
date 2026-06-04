/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed errors for the public slicer API.
//!
//! [`SliceError`] aggregates every failure mode of `slice_leaf`:
//! invalid profile, scene-graph lookup / kind mismatches, mesh
//! validation failures, scene bake failures, and runtime detection of
//! non-manifold geometry during polygon stitching.

use std::fmt;

use sc_mesh_core::SceneError;
use sc_mesh_core::indexed_mesh::{
    MeshErrorReport, MeshFaceError, MeshManifoldError, group_errors_by_variant,
};
use sc_mesh_core::scene::node::NodeId;

use crate::profile::SliceProfileError;

/// Maximum number of representative face indices listed per variant
/// in the [`SliceError`] summary output. Larger lists are truncated;
/// the full list is reachable via the underlying error vec on the
/// [`SliceError::FacesValidationFailed`] /
/// [`SliceError::ManifoldCheckFailed`] variants.
const SUMMARY_FACE_EXAMPLE_LIMIT: usize = 5;

#[derive(Debug)]
pub enum SliceError {
    /// Scene has no nodes; nothing to slice.
    EmptyScene,
    /// `slice_leaf` was passed a `NodeId` that does not exist in the
    /// scene.
    NodeNotFound { node_id: NodeId },
    /// The targeted node is a `Group` container, not a leaf carrying
    /// a mesh.
    NotALeaf { node_id: NodeId },
    /// The targeted node is a CSG combinator. CSG slicing is deferred
    /// to a follow-on plan.
    CsgNotYetSupported { node_id: NodeId },
    /// The targeted node is an `Instance` reference. Instance slicing
    /// is deferred to a follow-on plan.
    InstanceNotYetSupported { node_id: NodeId },
    /// The baked mesh failed face-index / zero-length-edge validation.
    FacesValidationFailed(Vec<MeshFaceError>),
    /// The baked mesh is not a closed manifold.
    ManifoldCheckFailed(Vec<MeshManifoldError>),
    /// Baking the leaf to world space failed.
    BakeFailed(SceneError),
    /// Stitching at a Z layer found segment endpoints without mates,
    /// implying the input mesh was less watertight than the manifold
    /// check declared. Acts as a runtime safety net.
    NonManifoldDuringStitching {
        z_mm: f64,
        dangling_endpoints: usize,
    },
    /// A [`SliceProfile`](crate::profile::SliceProfile) component
    /// constructor rejected its input.
    ProfileInvalid(SliceProfileError),
}

/// Append a per-variant breakdown of `errors` to `f`. Each line lists
/// the variant name, a count, and up to
/// [`SUMMARY_FACE_EXAMPLE_LIMIT`] representative face indices.
///
/// The breakdown is precisely the [`group_errors_by_variant`] output:
/// groups appear in first-occurrence order, with members preserving
/// input order.
fn write_aggregated_summary<E>(f: &mut fmt::Formatter<'_>, errors: &[E]) -> fmt::Result
where
    E: MeshErrorReport,
{
    for group in group_errors_by_variant(errors) {
        let count = group.members.len();
        let face_examples: Vec<String> = group
            .members
            .iter()
            .filter_map(|e| e.example_face())
            .take(SUMMARY_FACE_EXAMPLE_LIMIT)
            .map(|idx| format!("#{idx}"))
            .collect();
        if face_examples.is_empty() {
            write!(f, "\n  {count}x {}", group.variant_name)?;
        } else {
            write!(
                f,
                "\n  {count}x {} (e.g. faces {})",
                group.variant_name,
                face_examples.join(", "),
            )?;
        }
    }
    Ok(())
}

impl fmt::Display for SliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyScene => write!(f, "scene is empty: no nodes to slice"),
            Self::NodeNotFound { node_id } => {
                write!(f, "node #{} not found in scene", node_id.get())
            }
            Self::NotALeaf { node_id } => write!(
                f,
                "node #{} is a Group, expected a Leaf carrying a mesh",
                node_id.get(),
            ),
            Self::CsgNotYetSupported { node_id } => write!(
                f,
                "node #{} is a CSG combinator; CSG slicing is not yet supported",
                node_id.get(),
            ),
            Self::InstanceNotYetSupported { node_id } => write!(
                f,
                "node #{} is an Instance reference; instance slicing is not yet supported",
                node_id.get(),
            ),
            Self::FacesValidationFailed(errors) => {
                write!(f, "mesh face validation failed: {} error(s)", errors.len())?;
                write_aggregated_summary(f, errors)
            }
            Self::ManifoldCheckFailed(errors) => {
                write!(
                    f,
                    "mesh manifold validation failed: {} error(s)",
                    errors.len(),
                )?;
                write_aggregated_summary(f, errors)
            }
            Self::BakeFailed(err) => write!(f, "baking the leaf to world space failed: {err}"),
            Self::NonManifoldDuringStitching {
                z_mm,
                dangling_endpoints,
            } => write!(
                f,
                "non-manifold geometry detected during stitching at z = {z_mm} mm: \
                 {dangling_endpoints} dangling endpoint(s)",
            ),
            Self::ProfileInvalid(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SliceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BakeFailed(err) => Some(err),
            Self::ProfileInvalid(err) => Some(err),
            Self::EmptyScene
            | Self::NodeNotFound { .. }
            | Self::NotALeaf { .. }
            | Self::CsgNotYetSupported { .. }
            | Self::InstanceNotYetSupported { .. }
            | Self::FacesValidationFailed(_)
            | Self::ManifoldCheckFailed(_)
            | Self::NonManifoldDuringStitching { .. } => None,
        }
    }
}

impl From<SceneError> for SliceError {
    fn from(err: SceneError) -> Self {
        Self::BakeFailed(err)
    }
}

impl From<SliceProfileError> for SliceError {
    fn from(err: SliceProfileError) -> Self {
        Self::ProfileInvalid(err)
    }
}

#[cfg(test)]
mod slice_error_tests {
    use std::error::Error as _;

    use sc_mesh_core::Scene;
    use sc_mesh_core::scene::error::SceneError;
    use sc_mesh_core::transform::Transform3D;

    use crate::profile::{BuildVolume, SliceProfileError};

    use super::*;

    fn mint_node_id() -> NodeId {
        let mut scene = Scene::new();
        scene.add_group(None, Transform3D::Identity)
    }

    #[test]
    fn display_empty_scene() {
        let msg = format!("{}", SliceError::EmptyScene);
        assert!(msg.contains("empty"), "msg: {msg}");
    }

    #[test]
    fn display_node_not_found_includes_node_id() {
        let id = mint_node_id();
        let msg = format!("{}", SliceError::NodeNotFound { node_id: id });
        assert!(msg.contains(&format!("#{}", id.get())), "msg: {msg}");
    }

    #[test]
    fn display_not_a_leaf_mentions_group() {
        let id = mint_node_id();
        let msg = format!("{}", SliceError::NotALeaf { node_id: id });
        assert!(msg.contains("Group"), "msg: {msg}");
    }

    #[test]
    fn display_csg_not_yet_supported_mentions_csg() {
        let id = mint_node_id();
        let msg = format!("{}", SliceError::CsgNotYetSupported { node_id: id });
        assert!(msg.contains("CSG"), "msg: {msg}");
    }

    #[test]
    fn display_instance_not_yet_supported_mentions_instance() {
        let id = mint_node_id();
        let msg = format!("{}", SliceError::InstanceNotYetSupported { node_id: id });
        assert!(msg.contains("Instance"), "msg: {msg}");
    }

    #[test]
    fn display_non_manifold_during_stitching_renders_z_and_count() {
        let msg = format!(
            "{}",
            SliceError::NonManifoldDuringStitching {
                z_mm: 0.42,
                dangling_endpoints: 3,
            }
        );
        assert!(msg.contains("0.42"), "msg: {msg}");
        assert!(msg.contains("3"), "msg: {msg}");
    }

    #[test]
    fn from_scene_error_routes_to_bake_failed() {
        let scene_err = SceneError::DanglingNodeId { id: mint_node_id() };
        let err: SliceError = scene_err.into();
        assert!(matches!(err, SliceError::BakeFailed(_)));
    }

    #[test]
    fn from_profile_error_routes_to_profile_invalid() {
        let profile_err = BuildVolume::new(0.0, 60.0, 100.0).unwrap_err();
        let err: SliceError = profile_err.into();
        assert!(matches!(err, SliceError::ProfileInvalid(_)));
    }

    #[test]
    fn source_returns_inner_for_bake_failed_and_profile_invalid() {
        let bake_err = SliceError::BakeFailed(SceneError::DanglingNodeId { id: mint_node_id() });
        assert!(bake_err.source().is_some());

        let profile_err = SliceError::ProfileInvalid(SliceProfileError::TotalLayersZero);
        assert!(profile_err.source().is_some());
    }

    #[test]
    fn source_returns_none_for_leaf_variants() {
        assert!(SliceError::EmptyScene.source().is_none());
        assert!(SliceError::FacesValidationFailed(vec![]).source().is_none());
        assert!(SliceError::ManifoldCheckFailed(vec![]).source().is_none());
        assert!(
            SliceError::NonManifoldDuringStitching {
                z_mm: 0.1,
                dangling_endpoints: 1,
            }
            .source()
            .is_none(),
        );
    }
}

#[cfg(test)]
mod faces_validation_summary_tests {
    use sc_mesh_core::indexed_mesh::MeshFaceError;
    use sc_mesh_core::primitives::{TriVertexSlot, VertexIndex};

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
    fn header_carries_total_count() {
        let errors: Vec<MeshFaceError> = (0..554).map(zle).collect();
        let msg = format!("{}", SliceError::FacesValidationFailed(errors));
        assert!(
            msg.starts_with("mesh face validation failed: 554 error(s)"),
            "msg: {msg}",
        );
    }

    #[test]
    fn single_variant_collapses_into_one_summary_line() {
        let errors = vec![zle(2), zle(5), zle(11)];
        let msg = format!("{}", SliceError::FacesValidationFailed(errors));
        let lines: Vec<&str> = msg.lines().collect();
        assert_eq!(lines.len(), 2, "msg: {msg}");
        assert_eq!(lines[0], "mesh face validation failed: 3 error(s)");
        assert_eq!(lines[1], "  3x ZeroLengthEdge (e.g. faces #2, #5, #11)");
    }

    #[test]
    fn examples_are_capped_at_five_face_indices() {
        let errors: Vec<MeshFaceError> = [2_usize, 5, 11, 17, 23, 31, 47]
            .into_iter()
            .map(zle)
            .collect();
        let msg = format!("{}", SliceError::FacesValidationFailed(errors));
        assert!(
            msg.contains("7x ZeroLengthEdge (e.g. faces #2, #5, #11, #17, #23)"),
            "msg: {msg}",
        );
        assert!(!msg.contains("#31"), "msg should cap at five: {msg}");
    }

    #[test]
    fn mixed_variants_produce_one_summary_line_per_variant_in_first_seen_order() {
        let errors = vec![zle(2), oor(99), zle(5), oor(101), zle(11)];
        let msg = format!("{}", SliceError::FacesValidationFailed(errors));
        let lines: Vec<&str> = msg.lines().collect();
        assert_eq!(lines.len(), 3, "msg: {msg}");
        assert_eq!(lines[0], "mesh face validation failed: 5 error(s)");
        assert_eq!(lines[1], "  3x ZeroLengthEdge (e.g. faces #2, #5, #11)");
        assert_eq!(
            lines[2],
            "  2x OutOfRangeVertexIndex (e.g. faces #99, #101)"
        );
    }

    #[test]
    fn no_debug_dump_in_output() {
        let errors: Vec<MeshFaceError> = (0..50).map(zle).collect();
        let msg = format!("{}", SliceError::FacesValidationFailed(errors));
        assert!(
            !msg.contains("ZeroLengthEdge {"),
            "regression: Debug formatter leaked into Display output: {msg}",
        );
        assert!(
            !msg.contains("face_idx:"),
            "regression: Debug formatter leaked into Display output: {msg}",
        );
    }
}

#[cfg(test)]
mod manifold_check_summary_tests {
    use sc_mesh_core::indexed_mesh::MeshManifoldError;
    use sc_mesh_core::primitives::VertexIndex;

    use super::*;

    fn boundary(face: usize, a: u32, b: u32) -> MeshManifoldError {
        MeshManifoldError::BoundaryEdge {
            edge: (VertexIndex::from(a), VertexIndex::from(b)),
            face_idx: face.into(),
        }
    }

    fn winding(a: u32, b: u32, fa: usize, fb: usize) -> MeshManifoldError {
        MeshManifoldError::InconsistentWindingOrder {
            edge: (VertexIndex::from(a), VertexIndex::from(b)),
            face_indices: [fa.into(), fb.into()],
        }
    }

    #[test]
    fn empty_mesh_variant_renders_without_face_examples() {
        let errors = vec![MeshManifoldError::EmptyMesh];
        let msg = format!("{}", SliceError::ManifoldCheckFailed(errors));
        let lines: Vec<&str> = msg.lines().collect();
        assert_eq!(lines.len(), 2, "msg: {msg}");
        assert_eq!(lines[0], "mesh manifold validation failed: 1 error(s)");
        assert_eq!(lines[1], "  1x EmptyMesh");
    }

    #[test]
    fn mixed_variants_each_get_their_own_summary_line() {
        let errors = vec![boundary(17, 0, 1), winding(2, 3, 4, 5), boundary(42, 6, 7)];
        let msg = format!("{}", SliceError::ManifoldCheckFailed(errors));
        let lines: Vec<&str> = msg.lines().collect();
        assert_eq!(lines.len(), 3, "msg: {msg}");
        assert_eq!(lines[0], "mesh manifold validation failed: 3 error(s)");
        assert_eq!(lines[1], "  2x BoundaryEdge (e.g. faces #17, #42)");
        assert_eq!(lines[2], "  1x InconsistentWindingOrder (e.g. faces #4)");
    }
}
