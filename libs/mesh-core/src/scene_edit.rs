/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed scene mutation vocabulary.
//!
//! Every observable change to a [`Scene`] is one variant of
//! [`SceneEdit`]. Headless callers apply edits directly with
//! [`SceneEdit::apply_to_scene`]; cache layers (notably
//! `sc_mesh_spatial::SceneSpatial`) dispatch the same enum and add
//! invalidation hooks for the cached state that the variant affects.

use crate::scene::{Scene, Transform3D};
use crate::scene_graph::SceneGraph;
use crate::scene_graph_node::NodeId;

/// Typed scene mutation. The shared edit shape across the workspace:
/// the GUI's undo/redo stack, the headless apply paths, and any cache
/// layer's `apply_edit` all speak this enum.
///
/// Variants address either the flat [`Scene`] (`ObjectTransform`,
/// indexed by `objects[object_idx]`) or the [`SceneGraph`]
/// (`SetLocalTransform`, indexed by stable [`NodeId`]). An edit
/// addressed for one type is a no-op when applied to the other; this
/// lets the undo stack hold a single sequence of edits even while
/// both representations coexist.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneEdit {
    /// `objects[object_idx].transform` was changed from `before` to
    /// `after`. Applying writes `after`; `inverse()` produces the
    /// undo edit.
    ObjectTransform {
        object_idx: usize,
        before: Transform3D,
        after: Transform3D,
    },
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
        match self {
            Self::ObjectTransform { before, after, .. }
            | Self::SetLocalTransform { before, after, .. } => before == after,
        }
    }

    /// Inverse edit: applying `self` then `self.inverse()` returns
    /// the target to its pre-edit state.
    pub fn inverse(&self) -> Self {
        match self {
            Self::ObjectTransform {
                object_idx,
                before,
                after,
            } => Self::ObjectTransform {
                object_idx: *object_idx,
                before: *after,
                after: *before,
            },
            Self::SetLocalTransform {
                node_id,
                before,
                after,
            } => Self::SetLocalTransform {
                node_id: *node_id,
                before: *after,
                after: *before,
            },
        }
    }

    /// Apply this edit to a [`Scene`]. Cache-aware callers should go
    /// through `sc_mesh_spatial::SceneSpatial::apply_edit` instead,
    /// which dispatches the same enum and updates spatial caches
    /// before delegating here.
    ///
    /// `ObjectTransform` writes the new transform onto the addressed
    /// object, silently no-op on an out-of-range index.
    /// `SetLocalTransform` is a no-op on a `Scene`: that variant
    /// addresses by `NodeId`, which has no analogue here.
    pub fn apply_to_scene(&self, scene: &mut Scene) {
        match self {
            Self::ObjectTransform {
                object_idx, after, ..
            } => {
                if let Some(obj) = scene.objects.get_mut(*object_idx) {
                    obj.transform = *after;
                }
            }
            Self::SetLocalTransform { .. } => {}
        }
    }

    /// Apply this edit to a [`SceneGraph`]. `SetLocalTransform`
    /// writes the new transform onto the addressed node, silently
    /// no-op on a dangling `NodeId` (matching `apply_to_scene`'s
    /// silent treatment of an out-of-range `object_idx`).
    /// `ObjectTransform` is a no-op on a `SceneGraph`: that variant
    /// addresses by `objects[object_idx]`, which has no analogue
    /// here.
    pub fn apply_to_graph(&self, graph: &mut SceneGraph) {
        match self {
            Self::SetLocalTransform { node_id, after, .. } => {
                let _ = graph.set_local_transform(*node_id, *after);
            }
            Self::ObjectTransform { .. } => {}
        }
    }
}

#[cfg(test)]
mod scene_edit_tests {
    use super::*;
    use crate::indexed_mesh::IndexedMesh;
    use crate::scene::{SceneObject, SceneObjectContent};
    use crate::transform::AffineTransform3D;

    fn empty_mesh_object(transform: Transform3D) -> SceneObject {
        SceneObject {
            meta: None,
            transform,
            content: SceneObjectContent::Mesh(IndexedMesh::new(None, vec![], vec![])),
        }
    }

    #[test]
    fn is_noop_detects_identical_before_after() {
        let edit = SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        };
        assert!(edit.is_noop());
    }

    #[test]
    fn is_noop_false_when_transforms_differ() {
        let edit = SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        };
        assert!(!edit.is_noop());
    }

    #[test]
    fn inverse_swaps_before_and_after() {
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let edit = SceneEdit::ObjectTransform {
            object_idx: 7,
            before: Transform3D::Identity,
            after,
        };
        let inv = edit.inverse();
        assert_eq!(
            inv,
            SceneEdit::ObjectTransform {
                object_idx: 7,
                before: after,
                after: Transform3D::Identity,
            }
        );
    }

    #[test]
    fn apply_object_transform_writes_after_to_scene() {
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let mut scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![empty_mesh_object(Transform3D::Identity)],
        };
        SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after,
        }
        .apply_to_scene(&mut scene);
        assert_eq!(scene.objects[0].transform, after);
    }

    #[test]
    fn apply_then_inverse_round_trips_scene() {
        let before = Transform3D::Identity;
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let mut scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![empty_mesh_object(before)],
        };
        let edit = SceneEdit::ObjectTransform {
            object_idx: 0,
            before,
            after,
        };
        edit.apply_to_scene(&mut scene);
        edit.inverse().apply_to_scene(&mut scene);
        assert_eq!(scene.objects[0].transform, before);
    }

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

    #[test]
    fn object_transform_on_graph_is_noop() {
        let mut g = SceneGraph::new();
        let id = g.add_group(None, Transform3D::Identity);
        let original = g.node(id).unwrap().local_transform;
        SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0])),
        }
        .apply_to_graph(&mut g);
        assert_eq!(g.node(id).unwrap().local_transform, original);
    }

    #[test]
    fn set_local_transform_on_scene_is_noop() {
        use std::num::NonZeroU32;
        let bogus_node = NodeId(NonZeroU32::new(1).unwrap());
        let mut scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![empty_mesh_object(Transform3D::Identity)],
        };
        let original = scene.objects[0].transform;
        SceneEdit::SetLocalTransform {
            node_id: bogus_node,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        }
        .apply_to_scene(&mut scene);
        assert_eq!(scene.objects[0].transform, original);
    }
}
