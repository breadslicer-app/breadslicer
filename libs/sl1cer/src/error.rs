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
    MeshEdgeManifoldError, MeshErrorReport, MeshFaceError, MeshVertexError, group_errors_by_variant,
};
use sc_mesh_core::scene::node::NodeId;

/// Maximum number of representative face indices listed per variant
/// in the [`SliceError`] summary output. Larger lists are truncated;
/// the full list is reachable via the underlying error vec on the
/// [`SliceError::VerticesValidationFailed`] /
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
    /// The baked mesh carries NaN or infinite vertex coordinates.
    /// Detected on the baked vertices, so transform overflow is
    /// caught here too, not only poisoned source files.
    VerticesValidationFailed(Vec<MeshVertexError>),
    /// The baked mesh failed face-index / zero-length-edge validation.
    FacesValidationFailed(Vec<MeshFaceError>),
    /// The baked mesh is not a closed manifold.
    ManifoldCheckFailed(Vec<MeshEdgeManifoldError>),
    /// Baking the leaf to world space failed.
    BakeFailed(SceneError),
    /// Stitching at a Z layer found segment endpoints without mates,
    /// implying the input mesh was less watertight than the manifold
    /// check declared. Acts as a runtime safety net.
    NonManifoldDuringStitching {
        z_mm: f64,
        dangling_endpoints: usize,
    },
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
            Self::VerticesValidationFailed(errors) => {
                write!(
                    f,
                    "mesh vertex validation failed: {} error(s)",
                    errors.len(),
                )?;
                write_aggregated_summary(f, errors)
            }
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
        }
    }
}

impl std::error::Error for SliceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BakeFailed(err) => Some(err),
            Self::EmptyScene
            | Self::NodeNotFound { .. }
            | Self::NotALeaf { .. }
            | Self::CsgNotYetSupported { .. }
            | Self::InstanceNotYetSupported { .. }
            | Self::VerticesValidationFailed(_)
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

/// Which validation stage rejected a leaf's baked mesh. The 3 stages
/// run in order (vertices, then faces, then manifold), each requiring
/// the previous to pass, so one leaf carries exactly one error list.
#[derive(Debug)]
pub enum InvalidLeafGeometry {
    /// A vertex coordinate is NaN or infinite.
    Vertices(Vec<MeshVertexError>),
    /// Face-index / zero-length-edge validation failed.
    Faces(Vec<MeshFaceError>),
    /// The mesh passed face validation but is not a closed manifold.
    Manifold(Vec<MeshEdgeManifoldError>),
}

impl From<InvalidLeafGeometry> for SliceError {
    fn from(geometry: InvalidLeafGeometry) -> Self {
        match geometry {
            InvalidLeafGeometry::Vertices(errors) => Self::VerticesValidationFailed(errors),
            InvalidLeafGeometry::Faces(errors) => Self::FacesValidationFailed(errors),
            InvalidLeafGeometry::Manifold(errors) => Self::ManifoldCheckFailed(errors),
        }
    }
}

/// One leaf that failed pre-slice geometry validation, identified for
/// reporting.
#[derive(Debug)]
pub struct LeafValidationFailure {
    /// The failing leaf node.
    pub leaf: NodeId,
    /// The leaf's display name from its scene metadata, when present.
    pub leaf_name: Option<String>,
    /// What the validation stages rejected.
    pub geometry: InvalidLeafGeometry,
}

/// Failure modes of `slice_scene`.
///
/// Geometry validation aggregates: every reachable leaf is validated
/// even after the first failure, so `LeavesValidationFailed` always
/// lists all invalid leaves of the scene. All other variants abort
/// immediately.
#[derive(Debug)]
pub enum SliceSceneError {
    /// No mesh leaves are reachable from the scene roots.
    EmptyScene,
    /// Discovery reached a CSG combinator. Slicing its operand leaves
    /// as independent solids would silently produce union semantics
    /// regardless of the operator, so the scene is rejected instead.
    CsgNotYetSupported { node_id: NodeId },
    /// Discovery reached an `Instance` reference. Instance slicing is
    /// deferred together with CSG.
    InstanceNotYetSupported { node_id: NodeId },
    /// Baking or slicing one specific leaf failed. In practice the
    /// wrapped error is `BakeFailed` or `NonManifoldDuringStitching`;
    /// validation failures never take this path.
    LeafSlicingFailed {
        leaf: NodeId,
        leaf_name: Option<String>,
        error: SliceError,
    },
    /// One or more leaves failed geometry validation. Non-empty, in
    /// leaf discovery order.
    LeavesValidationFailed(Vec<LeafValidationFailure>),
}

/// Write `leaf #<id>` or `leaf #<id> ("<name>")` to `f`.
fn write_leaf_label(
    f: &mut fmt::Formatter<'_>,
    leaf: NodeId,
    leaf_name: &Option<String>,
) -> fmt::Result {
    match leaf_name {
        Some(name) => write!(f, "leaf #{} (\"{name}\")", leaf.get()),
        None => write!(f, "leaf #{}", leaf.get()),
    }
}

impl fmt::Display for InvalidLeafGeometry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vertices(errors) => {
                write!(
                    f,
                    "mesh vertex validation failed: {} error(s)",
                    errors.len(),
                )?;
                write_aggregated_summary(f, errors)
            }
            Self::Faces(errors) => {
                write!(f, "mesh face validation failed: {} error(s)", errors.len())?;
                write_aggregated_summary(f, errors)
            }
            Self::Manifold(errors) => {
                write!(
                    f,
                    "mesh manifold validation failed: {} error(s)",
                    errors.len(),
                )?;
                write_aggregated_summary(f, errors)
            }
        }
    }
}

impl fmt::Display for SliceSceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyScene => write!(f, "scene has no mesh leaves to slice"),
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
            Self::LeafSlicingFailed {
                leaf,
                leaf_name,
                error,
            } => {
                write!(f, "slicing ")?;
                write_leaf_label(f, *leaf, leaf_name)?;
                write!(f, " failed: {error}")
            }
            Self::LeavesValidationFailed(failures) => {
                write!(
                    f,
                    "mesh validation failed for {} of the scene's leaves:",
                    failures.len(),
                )?;
                for failure in failures {
                    writeln!(f)?;
                    write_leaf_label(f, failure.leaf, &failure.leaf_name)?;
                    write!(f, ": {}", failure.geometry)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SliceSceneError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LeafSlicingFailed { error, .. } => Some(error),
            Self::EmptyScene
            | Self::CsgNotYetSupported { .. }
            | Self::InstanceNotYetSupported { .. }
            | Self::LeavesValidationFailed(_) => None,
        }
    }
}

#[cfg(test)]
mod slice_error_tests {
    use std::error::Error as _;

    use sc_mesh_core::Scene;
    use sc_mesh_core::scene::error::SceneError;
    use sc_mesh_core::transform::Transform3D;

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
    fn source_returns_inner_for_bake_failed() {
        let bake_err = SliceError::BakeFailed(SceneError::DanglingNodeId { id: mint_node_id() });
        assert!(bake_err.source().is_some());
    }

    #[test]
    fn source_returns_none_for_leaf_variants() {
        assert!(SliceError::EmptyScene.source().is_none());
        assert!(
            SliceError::VerticesValidationFailed(vec![])
                .source()
                .is_none(),
        );
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
mod vertices_validation_summary_tests {
    use sc_mesh_core::indexed_mesh::MeshVertexError;
    use sc_mesh_core::primitives::CoordAxis;

    use super::*;

    fn nan(vertex: usize) -> MeshVertexError {
        MeshVertexError::NaNCoordinate {
            vertex_idx: vertex,
            coord: CoordAxis::X,
        }
    }

    fn infinite(vertex: usize) -> MeshVertexError {
        MeshVertexError::InfiniteCoordinate {
            vertex_idx: vertex,
            coord: CoordAxis::Z,
        }
    }

    #[test]
    fn header_carries_total_count_and_variant_lines_have_no_face_examples() {
        let errors = vec![nan(0), infinite(3), nan(7)];
        let msg = format!("{}", SliceError::VerticesValidationFailed(errors));
        let lines: Vec<&str> = msg.lines().collect();
        assert_eq!(lines.len(), 3, "msg: {msg}");
        assert_eq!(lines[0], "mesh vertex validation failed: 3 error(s)");
        assert_eq!(lines[1], "  2x NaNCoordinate");
        assert_eq!(lines[2], "  1x InfiniteCoordinate");
    }

    #[test]
    fn no_debug_dump_in_output() {
        let errors = vec![nan(0), infinite(1)];
        let msg = format!("{}", SliceError::VerticesValidationFailed(errors));
        assert!(
            !msg.contains("NaNCoordinate {"),
            "regression: Debug formatter leaked into Display output: {msg}",
        );
        assert!(
            !msg.contains("vertex_idx:"),
            "regression: Debug formatter leaked into Display output: {msg}",
        );
    }
}

#[cfg(test)]
mod manifold_check_summary_tests {
    use sc_mesh_core::indexed_mesh::MeshEdgeManifoldError;
    use sc_mesh_core::primitives::VertexIndex;

    use super::*;

    fn boundary(face: usize, a: u32, b: u32) -> MeshEdgeManifoldError {
        MeshEdgeManifoldError::BoundaryEdge {
            edge: (VertexIndex::from(a), VertexIndex::from(b)),
            face_idx: face.into(),
        }
    }

    fn winding(a: u32, b: u32, fa: usize, fb: usize) -> MeshEdgeManifoldError {
        MeshEdgeManifoldError::InconsistentWindingOrder {
            edge: (VertexIndex::from(a), VertexIndex::from(b)),
            face_indices: [fa.into(), fb.into()],
        }
    }

    #[test]
    fn empty_mesh_variant_renders_without_face_examples() {
        let errors = vec![MeshEdgeManifoldError::EmptyMesh];
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

#[cfg(test)]
mod slice_scene_error_tests {
    use std::error::Error as _;

    use sc_mesh_core::Scene;
    use sc_mesh_core::indexed_mesh::{MeshEdgeManifoldError, MeshVertexError};
    use sc_mesh_core::primitives::{CoordAxis, TriVertexSlot, VertexIndex};
    use sc_mesh_core::transform::Transform3D;

    use super::*;

    fn mint_node_id() -> NodeId {
        let mut scene = Scene::new();
        scene.add_group(None, Transform3D::Identity)
    }

    fn zle(face: usize) -> MeshFaceError {
        MeshFaceError::ZeroLengthEdge {
            face_idx: face.into(),
            vertex_pos_a: TriVertexSlot::V0,
            vertex_pos_b: TriVertexSlot::V1,
            vertex_idx: VertexIndex::from(0_u32),
        }
    }

    #[test]
    fn display_empty_scene_mentions_leaves() {
        let msg = format!("{}", SliceSceneError::EmptyScene);
        assert!(msg.contains("no mesh leaves"), "msg: {msg}");
    }

    #[test]
    fn display_csg_not_yet_supported_mentions_csg() {
        let id = mint_node_id();
        let msg = format!("{}", SliceSceneError::CsgNotYetSupported { node_id: id });
        assert!(msg.contains("CSG"), "msg: {msg}");
        assert!(msg.contains(&format!("#{}", id.get())), "msg: {msg}");
    }

    #[test]
    fn display_instance_not_yet_supported_mentions_instance() {
        let id = mint_node_id();
        let msg = format!(
            "{}",
            SliceSceneError::InstanceNotYetSupported { node_id: id },
        );
        assert!(msg.contains("Instance"), "msg: {msg}");
    }

    #[test]
    fn display_leaf_slicing_failed_renders_label_and_inner_error() {
        let id = mint_node_id();
        let msg = format!(
            "{}",
            SliceSceneError::LeafSlicingFailed {
                leaf: id,
                leaf_name: Some("front-cover".to_string()),
                error: SliceError::NonManifoldDuringStitching {
                    z_mm: 0.42,
                    dangling_endpoints: 3,
                },
            },
        );
        assert!(
            msg.contains(&format!("leaf #{} (\"front-cover\")", id.get())),
            "msg: {msg}",
        );
        assert!(msg.contains("0.42"), "msg: {msg}");
    }

    #[test]
    fn display_leaves_validation_failed_lists_each_leaf_with_its_summary() {
        let a = mint_node_id();
        let b = mint_node_id();
        let err = SliceSceneError::LeavesValidationFailed(vec![
            LeafValidationFailure {
                leaf: a,
                leaf_name: Some("cracked".to_string()),
                geometry: InvalidLeafGeometry::Faces(vec![zle(2), zle(5)]),
            },
            LeafValidationFailure {
                leaf: b,
                leaf_name: None,
                geometry: InvalidLeafGeometry::Manifold(vec![MeshEdgeManifoldError::EmptyMesh]),
            },
        ]);
        let msg = format!("{err}");
        assert!(
            msg.starts_with("mesh validation failed for 2 of the scene's leaves:"),
            "msg: {msg}",
        );
        assert!(
            msg.contains(&format!(
                "leaf #{} (\"cracked\"): mesh face validation failed: 2 error(s)",
                a.get(),
            )),
            "msg: {msg}",
        );
        assert!(msg.contains("2x ZeroLengthEdge"), "msg: {msg}");
        assert!(
            msg.contains(&format!(
                "leaf #{}: mesh manifold validation failed: 1 error(s)",
                b.get(),
            )),
            "unnamed leaf must render without quotes; msg: {msg}",
        );
    }

    #[test]
    fn source_returns_inner_only_for_leaf_slicing_failed() {
        let id = mint_node_id();
        let failed = SliceSceneError::LeafSlicingFailed {
            leaf: id,
            leaf_name: None,
            error: SliceError::EmptyScene,
        };
        assert!(failed.source().is_some());
        assert!(SliceSceneError::EmptyScene.source().is_none());
        assert!(
            SliceSceneError::LeavesValidationFailed(vec![])
                .source()
                .is_none(),
        );
    }

    #[test]
    fn invalid_leaf_geometry_converts_into_the_matching_slice_error_variant() {
        let vertices: SliceError =
            InvalidLeafGeometry::Vertices(vec![MeshVertexError::NaNCoordinate {
                vertex_idx: 0,
                coord: CoordAxis::X,
            }])
            .into();
        assert!(matches!(
            vertices,
            SliceError::VerticesValidationFailed(v) if v.len() == 1,
        ));
        let faces: SliceError = InvalidLeafGeometry::Faces(vec![zle(1)]).into();
        assert!(matches!(faces, SliceError::FacesValidationFailed(v) if v.len() == 1));
        let manifold: SliceError =
            InvalidLeafGeometry::Manifold(vec![MeshEdgeManifoldError::EmptyMesh]).into();
        assert!(matches!(
            manifold,
            SliceError::ManifoldCheckFailed(v) if v.len() == 1,
        ));
    }

    #[test]
    fn display_leaves_validation_failed_renders_a_vertices_failure() {
        let leaf = mint_node_id();
        let err = SliceSceneError::LeavesValidationFailed(vec![LeafValidationFailure {
            leaf,
            leaf_name: Some("poisoned".to_string()),
            geometry: InvalidLeafGeometry::Vertices(vec![MeshVertexError::InfiniteCoordinate {
                vertex_idx: 4,
                coord: CoordAxis::Y,
            }]),
        }]);
        let msg = format!("{err}");
        assert!(
            msg.contains(&format!(
                "leaf #{} (\"poisoned\"): mesh vertex validation failed: 1 error(s)",
                leaf.get(),
            )),
            "msg: {msg}",
        );
        assert!(msg.contains("1x InfiniteCoordinate"), "msg: {msg}");
    }
}
