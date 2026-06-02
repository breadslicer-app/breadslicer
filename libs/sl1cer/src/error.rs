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
use sc_mesh_core::indexed_mesh::{MeshFaceError, MeshManifoldError};
use sc_mesh_core::scene::node::NodeId;

use crate::profile::SliceProfileError;

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
            Self::FacesValidationFailed(errors) => write!(
                f,
                "mesh face validation failed with {} error(s): {errors:?}",
                errors.len(),
            ),
            Self::ManifoldCheckFailed(errors) => write!(
                f,
                "mesh manifold validation failed with {} error(s): {errors:?}",
                errors.len(),
            ),
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
