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

use crate::scene::Scene;
use crate::scene_node::{MeshResourceId, NodeId};
use crate::transform::Transform3D;

/// Typed scene mutation. The shared edit shape across the workspace:
/// the GUI's undo/redo stack, the headless apply paths, and any
/// cache layer's `apply_edit` all speak this enum.
///
/// Every variant is keyed by a stable [`NodeId`] on a [`Scene`].
/// `AddRootLeaf` and `RemoveRootLeaf` carry the same fields so each
/// is the [`Self::inverse`] of the other.
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
    /// Install a mesh-resource-backed leaf at `node_id` and register
    /// it as a scene root. The id must come from
    /// [`Scene::allocate_unbound_node_id`] so the arena slot exists
    /// and is empty when the edit is applied.
    AddRootLeaf {
        node_id: NodeId,
        mesh: MeshResourceId,
        local_transform: Transform3D,
    },
    /// Tombstone the mesh-resource-backed root leaf at `node_id`.
    /// `mesh` and `local_transform` snapshot the payload so the
    /// inverse [`Self::AddRootLeaf`] re-installs it identically.
    RemoveRootLeaf {
        node_id: NodeId,
        mesh: MeshResourceId,
        local_transform: Transform3D,
    },
}

impl SceneEdit {
    /// True when applying this edit would not change the target.
    /// Useful for filtering no-op gestures from the undo stack.
    pub fn is_noop(&self) -> bool {
        match self {
            Self::SetLocalTransform { before, after, .. } => before == after,
            Self::AddRootLeaf { .. } | Self::RemoveRootLeaf { .. } => false,
        }
    }

    /// Inverse edit: applying `self` then `self.inverse()` returns
    /// the target to its pre-edit state.
    pub fn inverse(&self) -> Self {
        match self {
            Self::SetLocalTransform {
                node_id,
                before,
                after,
            } => Self::SetLocalTransform {
                node_id: *node_id,
                before: *after,
                after: *before,
            },
            Self::AddRootLeaf {
                node_id,
                mesh,
                local_transform,
            } => Self::RemoveRootLeaf {
                node_id: *node_id,
                mesh: *mesh,
                local_transform: *local_transform,
            },
            Self::RemoveRootLeaf {
                node_id,
                mesh,
                local_transform,
            } => Self::AddRootLeaf {
                node_id: *node_id,
                mesh: *mesh,
                local_transform: *local_transform,
            },
        }
    }

    /// Apply this edit to a [`Scene`]. Each variant calls the
    /// matching scene mutator and silently swallows precondition
    /// errors (dangling id, occupied slot, kind mismatch); the only
    /// way to reach a precondition violation here is a programming
    /// bug, and a downstream cache layer (see
    /// `sc_mesh_spatial::SceneSpatial::apply_edit`) is the next gate.
    pub fn apply_to_scene(&self, graph: &mut Scene) {
        match self {
            Self::SetLocalTransform { node_id, after, .. } => {
                let _ = graph.set_local_transform(*node_id, *after);
            }
            Self::AddRootLeaf {
                node_id,
                mesh,
                local_transform,
            } => {
                let _ = graph.install_root_leaf(*node_id, *mesh, *local_transform);
            }
            Self::RemoveRootLeaf { node_id, .. } => {
                let _ = graph.remove_root_leaf(*node_id);
            }
        }
    }
}

#[cfg(test)]
mod scene_edit_tests {
    use super::*;
    use crate::transform::AffineTransform3D;

    #[test]
    fn set_local_transform_is_noop_when_before_equals_after() {
        let mut g = Scene::new();
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
        let mut g = Scene::new();
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
        let mut g = Scene::new();
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
        let mut g = Scene::new();
        let id = g.add_group(None, Transform3D::Identity);
        let after = Transform3D::Affine(AffineTransform3D::from_translation([5.0, 6.0, 7.0]));
        SceneEdit::SetLocalTransform {
            node_id: id,
            before: Transform3D::Identity,
            after,
        }
        .apply_to_scene(&mut g);
        assert_eq!(g.node(id).unwrap().local_transform, after);
    }

    #[test]
    fn apply_then_inverse_round_trips_graph() {
        let mut g = Scene::new();
        let id = g.add_group(None, Transform3D::Identity);
        let before = Transform3D::Identity;
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let edit = SceneEdit::SetLocalTransform {
            node_id: id,
            before,
            after,
        };
        edit.apply_to_scene(&mut g);
        edit.inverse().apply_to_scene(&mut g);
        assert_eq!(g.node(id).unwrap().local_transform, before);
    }

    #[test]
    fn set_local_transform_on_dangling_node_id_is_silent_noop() {
        use std::num::NonZeroU32;
        let mut g = Scene::new();
        let bogus = NodeId(NonZeroU32::new(99).unwrap());
        // Should not panic; the underlying graph mutator returns an
        // error and we discard it.
        SceneEdit::SetLocalTransform {
            node_id: bogus,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        }
        .apply_to_scene(&mut g);
        // No nodes existed before and none exist after.
        assert!(g.node(bogus).is_none());
    }

    fn fresh_scene_with_one_resource() -> (Scene, NodeId, MeshResourceId, Transform3D) {
        use crate::indexed_mesh::IndexedMesh;
        use crate::primitives::{IndexedTriangle, Normal, Vertex};
        let mut g = Scene::new();
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        };
        let rid = g.add_mesh_resource(mesh);
        let id = g.allocate_unbound_node_id();
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        (g, id, rid, xform)
    }

    #[test]
    fn add_root_leaf_is_never_noop() {
        let (_, id, rid, xform) = fresh_scene_with_one_resource();
        let edit = SceneEdit::AddRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        };
        assert!(!edit.is_noop());
    }

    #[test]
    fn remove_root_leaf_is_never_noop() {
        let (_, id, rid, xform) = fresh_scene_with_one_resource();
        let edit = SceneEdit::RemoveRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        };
        assert!(!edit.is_noop());
    }

    #[test]
    fn add_root_leaf_inverse_is_remove_root_leaf_with_same_fields() {
        let (_, id, rid, xform) = fresh_scene_with_one_resource();
        let add = SceneEdit::AddRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        };
        assert_eq!(
            add.inverse(),
            SceneEdit::RemoveRootLeaf {
                node_id: id,
                mesh: rid,
                local_transform: xform,
            }
        );
    }

    #[test]
    fn remove_root_leaf_inverse_is_add_root_leaf_with_same_fields() {
        let (_, id, rid, xform) = fresh_scene_with_one_resource();
        let remove = SceneEdit::RemoveRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        };
        assert_eq!(
            remove.inverse(),
            SceneEdit::AddRootLeaf {
                node_id: id,
                mesh: rid,
                local_transform: xform,
            }
        );
    }

    #[test]
    fn apply_add_root_leaf_installs_a_root() {
        let (mut g, id, rid, xform) = fresh_scene_with_one_resource();
        let edit = SceneEdit::AddRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        };
        edit.apply_to_scene(&mut g);
        assert!(g.roots().contains(&id));
        assert!(g.node(id).is_some());
    }

    #[test]
    fn apply_add_then_inverse_round_trips_graph() {
        let (mut g, id, rid, xform) = fresh_scene_with_one_resource();
        let snapshot_before = g.clone();
        let edit = SceneEdit::AddRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        };
        edit.apply_to_scene(&mut g);
        edit.inverse().apply_to_scene(&mut g);
        assert_eq!(g, snapshot_before);
    }

    #[test]
    fn apply_remove_then_inverse_round_trips_graph() {
        let (mut g, id, rid, xform) = fresh_scene_with_one_resource();
        g.install_root_leaf(id, rid, xform).unwrap();
        let snapshot_after_install = g.clone();
        let edit = SceneEdit::RemoveRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        };
        edit.apply_to_scene(&mut g);
        edit.inverse().apply_to_scene(&mut g);
        assert_eq!(g, snapshot_after_install);
    }

    #[test]
    fn add_root_leaf_with_out_of_range_id_is_silent_noop() {
        use std::num::NonZeroU32;
        let mut g = Scene::new();
        let bogus = NodeId(NonZeroU32::new(99).unwrap());
        let bogus_rid = MeshResourceId(NonZeroU32::new(99).unwrap());
        SceneEdit::AddRootLeaf {
            node_id: bogus,
            mesh: bogus_rid,
            local_transform: Transform3D::Identity,
        }
        .apply_to_scene(&mut g);
        assert!(g.node(bogus).is_none());
        assert!(g.roots().is_empty());
    }

    #[test]
    fn remove_root_leaf_on_empty_slot_is_silent_noop() {
        let (mut g, id, rid, xform) = fresh_scene_with_one_resource();
        SceneEdit::RemoveRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        }
        .apply_to_scene(&mut g);
        assert!(g.node(id).is_none());
        assert!(!g.roots().contains(&id));
    }
}
