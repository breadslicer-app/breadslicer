/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed scene mutation vocabulary.
//!
//! Every observable change to a [`SceneGraph`] is one variant of
//! [`SceneEdit`]. Headless callers apply edits directly with
//! [`SceneEdit::apply_to_graph`]; cache layers (notably
//! `sc_mesh_spatial::SceneSpatial`) dispatch the same enum and add
//! invalidation hooks for the cached state that the variant affects.

use crate::scene_graph::SceneGraph;
use crate::scene_graph_node::NodeId;
use crate::transform::Transform3D;

/// Typed scene mutation. The shared edit shape across the workspace:
/// the GUI's undo/redo stack, the headless apply paths, and any
/// cache layer's `apply_edit` all speak this enum.
///
/// `SetLocalTransform` is keyed by a stable [`NodeId`] on a
/// [`SceneGraph`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneEdit {
    /// `node.local_transform` for the node at `node_id` was changed
    /// from `before` to `after`. Stable id, so the edit stays valid
    /// across intervening edits.
    SetLocalTransform {
        node_id: NodeId,
        before: Transform3D,
        after: Transform3D,
    },
}

impl SceneEdit {
    /// True when applying this edit would not change the target.
    /// Useful for filtering no-op gestures from the undo stack.
    pub fn is_noop(&self) -> bool {
        let Self::SetLocalTransform { before, after, .. } = self;
        before == after
    }

    /// Inverse edit: applying `self` then `self.inverse()` returns
    /// the target to its pre-edit state.
    pub fn inverse(&self) -> Self {
        let Self::SetLocalTransform {
            node_id,
            before,
            after,
        } = self;
        Self::SetLocalTransform {
            node_id: *node_id,
            before: *after,
            after: *before,
        }
    }

    /// Apply this edit to a [`SceneGraph`]. `SetLocalTransform`
    /// writes the new transform onto the addressed node, silently
    /// no-op on a dangling `NodeId`. Cache-aware callers should go
    /// through `sc_mesh_spatial::SceneSpatial::apply_edit` instead,
    /// which dispatches the same enum and updates spatial caches
    /// before delegating here.
    pub fn apply_to_graph(&self, graph: &mut SceneGraph) {
        let Self::SetLocalTransform { node_id, after, .. } = self;
        let _ = graph.set_local_transform(*node_id, *after);
    }
}

#[cfg(test)]
mod scene_edit_tests {
    use super::*;
    use crate::transform::AffineTransform3D;

    #[test]
    fn set_local_transform_is_noop_when_before_equals_after() {
        let mut g = SceneGraph::new();
        let id = g.add_group(None, Transform3D::Identity);
        let edit = SceneEdit::SetLocalTransform {
            node_id: id,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        };
        assert!(edit.is_noop());
    }

    #[test]
    fn set_local_transform_is_not_noop_when_transforms_differ() {
        let mut g = SceneGraph::new();
        let id = g.add_group(None, Transform3D::Identity);
        let edit = SceneEdit::SetLocalTransform {
            node_id: id,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        };
        assert!(!edit.is_noop());
    }

    #[test]
    fn set_local_transform_inverse_swaps_before_and_after() {
        let mut g = SceneGraph::new();
        let id = g.add_group(None, Transform3D::Identity);
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let edit = SceneEdit::SetLocalTransform {
            node_id: id,
            before: Transform3D::Identity,
            after,
        };
        assert_eq!(
            edit.inverse(),
            SceneEdit::SetLocalTransform {
                node_id: id,
                before: after,
                after: Transform3D::Identity,
            }
        );
    }

    #[test]
    fn apply_set_local_transform_writes_after_to_graph() {
        let mut g = SceneGraph::new();
        let id = g.add_group(None, Transform3D::Identity);
        let after = Transform3D::Affine(AffineTransform3D::from_translation([5.0, 6.0, 7.0]));
        SceneEdit::SetLocalTransform {
            node_id: id,
            before: Transform3D::Identity,
            after,
        }
        .apply_to_graph(&mut g);
        assert_eq!(g.node(id).unwrap().local_transform, after);
    }

    #[test]
    fn apply_then_inverse_round_trips_graph() {
        let mut g = SceneGraph::new();
        let id = g.add_group(None, Transform3D::Identity);
        let before = Transform3D::Identity;
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let edit = SceneEdit::SetLocalTransform {
            node_id: id,
            before,
            after,
        };
        edit.apply_to_graph(&mut g);
        edit.inverse().apply_to_graph(&mut g);
        assert_eq!(g.node(id).unwrap().local_transform, before);
    }

    #[test]
    fn set_local_transform_on_dangling_node_id_is_silent_noop() {
        use std::num::NonZeroU32;
        let mut g = SceneGraph::new();
        let bogus = NodeId(NonZeroU32::new(99).unwrap());
        // Should not panic; the underlying graph mutator returns an
        // error and we discard it.
        SceneEdit::SetLocalTransform {
            node_id: bogus,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        }
        .apply_to_graph(&mut g);
        // No nodes existed before and none exist after.
        assert!(g.node(bogus).is_none());
    }
}
