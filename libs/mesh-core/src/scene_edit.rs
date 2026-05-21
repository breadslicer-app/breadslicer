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

use std::sync::Arc;

use crate::indexed_mesh::IndexedMesh;
use crate::scene::Scene;
use crate::scene_node::{MeshResourceId, NodeId};
use crate::transform::Transform3D;

/// Typed scene mutation. The shared edit shape across the workspace:
/// the GUI's undo/redo stack, the headless apply paths, and any
/// cache layer's `apply_edit` all speak this enum.
///
/// Every variant is keyed by a stable [`NodeId`] or
/// [`MeshResourceId`] on a [`Scene`]. Variants come in `Add` / `Remove`
/// pairs that carry the same fields, so each is the
/// [`Self::inverse`] of the other; [`Self::Compound`] bundles several
/// edits as a single undo / redo unit (used for paste, which adds
/// both a mesh resource and a root leaf atomically).
///
/// `Clone` rather than `Copy`: variants like
/// [`Self::AddMeshResource`] carry `Arc<IndexedMesh>` so the
/// underlying mesh storage can be shared across undo / redo states
/// and across scenes without deep-cloning vertex / index buffers; an
/// `Arc` is `Clone` but never `Copy`. Existing call sites pass edits
/// by value and clone them explicitly when needed.
#[derive(Clone, Debug, PartialEq)]
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
    /// Install a mesh resource at `rid` carrying the given `Arc`.
    /// The id must come from
    /// [`Scene::allocate_unbound_mesh_resource_id`] so the arena
    /// slot exists and is empty when the edit is applied.
    AddMeshResource {
        rid: MeshResourceId,
        mesh: Arc<IndexedMesh>,
    },
    /// Tombstone the mesh resource at `rid`. `mesh` snapshots the
    /// `Arc` so the inverse [`Self::AddMeshResource`] re-installs the
    /// same storage.
    RemoveMeshResource {
        rid: MeshResourceId,
        mesh: Arc<IndexedMesh>,
    },
    /// Apply a sequence of edits as a single undo / redo unit. The
    /// inverse is the reversed sequence of inverses, so e.g. a paste
    /// that emits `Compound([AddMeshResource, AddRootLeaf])` undoes
    /// as `Compound([RemoveRootLeaf, RemoveMeshResource])` (which
    /// is the order that satisfies the
    /// [`SceneError::MeshResourceInUse`] precondition).
    Compound(Vec<SceneEdit>),
}

impl SceneEdit {
    /// True when applying this edit would not change the target.
    /// Useful for filtering no-op gestures from the undo stack.
    pub fn is_noop(&self) -> bool {
        match self {
            Self::SetLocalTransform { before, after, .. } => before == after,
            Self::AddRootLeaf { .. }
            | Self::RemoveRootLeaf { .. }
            | Self::AddMeshResource { .. }
            | Self::RemoveMeshResource { .. } => false,
            Self::Compound(edits) => edits.iter().all(Self::is_noop),
        }
    }

    /// True when applying this edit changes the node or
    /// mesh-resource arena membership (a leaf appears or
    /// disappears, or a mesh resource is registered or removed).
    /// `SetLocalTransform` is non-structural: it perturbs an
    /// existing leaf's transform without touching arena slots.
    ///
    /// Downstream caches that mirror the arena (the GPU scene's
    /// per-leaf model buffer, the spatial caches' parallel vecs)
    /// use this to decide whether they need a re-upload after the
    /// edit lands.
    pub fn is_structural(&self) -> bool {
        match self {
            Self::SetLocalTransform { .. } => false,
            Self::AddRootLeaf { .. }
            | Self::RemoveRootLeaf { .. }
            | Self::AddMeshResource { .. }
            | Self::RemoveMeshResource { .. } => true,
            Self::Compound(edits) => edits.iter().any(Self::is_structural),
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
            Self::AddMeshResource { rid, mesh } => Self::RemoveMeshResource {
                rid: *rid,
                mesh: Arc::clone(mesh),
            },
            Self::RemoveMeshResource { rid, mesh } => Self::AddMeshResource {
                rid: *rid,
                mesh: Arc::clone(mesh),
            },
            Self::Compound(edits) => {
                Self::Compound(edits.iter().rev().map(Self::inverse).collect())
            }
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
            Self::AddMeshResource { rid, mesh } => {
                let _ = graph.install_mesh_resource_at(*rid, Arc::clone(mesh));
            }
            Self::RemoveMeshResource { rid, .. } => {
                let _ = graph.remove_mesh_resource(*rid);
            }
            Self::Compound(edits) => {
                for edit in edits {
                    edit.apply_to_scene(graph);
                }
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
    fn set_local_transform_is_not_structural() {
        let mut g = Scene::new();
        let id = g.add_group(None, Transform3D::Identity);
        let edit = SceneEdit::SetLocalTransform {
            node_id: id,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        };
        assert!(!edit.is_structural());
    }

    #[test]
    fn add_root_leaf_is_structural() {
        use std::num::NonZeroU32;
        let edit = SceneEdit::AddRootLeaf {
            node_id: crate::scene_node::NodeId(NonZeroU32::new(1).unwrap()),
            mesh: crate::scene_node::MeshResourceId(NonZeroU32::new(1).unwrap()),
            local_transform: Transform3D::Identity,
        };
        assert!(edit.is_structural());
    }

    #[test]
    fn compound_is_structural_iff_any_inner_is() {
        let nid = crate::scene_node::NodeId(std::num::NonZeroU32::new(1).unwrap());
        let only_set = SceneEdit::Compound(vec![SceneEdit::SetLocalTransform {
            node_id: nid,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        }]);
        assert!(!only_set.is_structural());
        let mixed = SceneEdit::Compound(vec![
            SceneEdit::SetLocalTransform {
                node_id: nid,
                before: Transform3D::Identity,
                after: Transform3D::Identity,
            },
            SceneEdit::AddRootLeaf {
                node_id: nid,
                mesh: crate::scene_node::MeshResourceId(std::num::NonZeroU32::new(1).unwrap()),
                local_transform: Transform3D::Identity,
            },
        ]);
        assert!(mixed.is_structural());
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

    fn fresh_scene_with_unbound_mesh_slot() -> (Scene, MeshResourceId, Arc<IndexedMesh>) {
        use crate::primitives::{IndexedTriangle, Normal, Vertex};
        let mut g = Scene::new();
        let mesh = Arc::new(IndexedMesh {
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
        });
        let rid = g.allocate_unbound_mesh_resource_id();
        (g, rid, mesh)
    }

    #[test]
    fn add_mesh_resource_is_never_noop() {
        let (_, rid, mesh) = fresh_scene_with_unbound_mesh_slot();
        let edit = SceneEdit::AddMeshResource { rid, mesh };
        assert!(!edit.is_noop());
    }

    #[test]
    fn remove_mesh_resource_is_never_noop() {
        let (_, rid, mesh) = fresh_scene_with_unbound_mesh_slot();
        let edit = SceneEdit::RemoveMeshResource { rid, mesh };
        assert!(!edit.is_noop());
    }

    #[test]
    fn add_mesh_resource_inverse_is_remove_with_same_arc() {
        let (_, rid, mesh) = fresh_scene_with_unbound_mesh_slot();
        let add = SceneEdit::AddMeshResource {
            rid,
            mesh: Arc::clone(&mesh),
        };
        let inverse = add.inverse();
        let SceneEdit::RemoveMeshResource {
            rid: out_rid,
            mesh: out_mesh,
        } = inverse
        else {
            panic!("inverse of AddMeshResource must be RemoveMeshResource");
        };
        assert_eq!(out_rid, rid);
        assert!(std::ptr::eq(Arc::as_ptr(&mesh), Arc::as_ptr(&out_mesh)));
    }

    #[test]
    fn apply_add_mesh_resource_installs_at_the_allocated_slot() {
        let (mut g, rid, mesh) = fresh_scene_with_unbound_mesh_slot();
        SceneEdit::AddMeshResource {
            rid,
            mesh: Arc::clone(&mesh),
        }
        .apply_to_scene(&mut g);
        assert!(std::ptr::eq(
            Arc::as_ptr(&mesh),
            Arc::as_ptr(g.mesh_resource_arc(rid).unwrap())
        ));
    }

    #[test]
    fn apply_add_mesh_then_inverse_round_trips_scene() {
        let (mut g, rid, mesh) = fresh_scene_with_unbound_mesh_slot();
        let snapshot_before = g.clone();
        let edit = SceneEdit::AddMeshResource {
            rid,
            mesh: Arc::clone(&mesh),
        };
        edit.apply_to_scene(&mut g);
        edit.inverse().apply_to_scene(&mut g);
        assert_eq!(g, snapshot_before);
    }

    #[test]
    fn compound_is_noop_iff_all_inner_are_noop() {
        // Identity-only SetLocalTransform is the only noop variant
        // in the enum's vocabulary; everything else is unconditionally
        // mutating.
        use crate::primitives::{IndexedTriangle, Normal, Vertex};
        let mut g = Scene::new();
        let nid = g.add_group(None, Transform3D::Identity);
        let noop = SceneEdit::SetLocalTransform {
            node_id: nid,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        };
        assert!(SceneEdit::Compound(vec![noop.clone(), noop.clone()]).is_noop());
        let mesh = Arc::new(IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 0, 0],
            )],
        });
        let rid = g.allocate_unbound_mesh_resource_id();
        let mutating = SceneEdit::AddMeshResource { rid, mesh };
        assert!(!SceneEdit::Compound(vec![noop, mutating]).is_noop());
    }

    #[test]
    fn compound_inverse_reverses_and_inverts_each_inner_edit() {
        let (_, rid, mesh) = fresh_scene_with_unbound_mesh_slot();
        let mut g = Scene::new();
        let nid = g.allocate_unbound_node_id();
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let compound = SceneEdit::Compound(vec![
            SceneEdit::AddMeshResource {
                rid,
                mesh: Arc::clone(&mesh),
            },
            SceneEdit::AddRootLeaf {
                node_id: nid,
                mesh: rid,
                local_transform: xform,
            },
        ]);
        let inverse = compound.inverse();
        let SceneEdit::Compound(parts) = inverse else {
            panic!("inverse of Compound must be Compound");
        };
        assert!(matches!(
            parts[0],
            SceneEdit::RemoveRootLeaf {
                node_id,
                mesh: m,
                ..
            } if node_id == nid && m == rid
        ));
        assert!(matches!(
            parts[1],
            SceneEdit::RemoveMeshResource {
                rid: r,
                ..
            } if r == rid
        ));
    }

    #[test]
    fn compound_apply_runs_edits_in_order_and_round_trips_via_inverse() {
        // The paste shape: AddMeshResource then AddRootLeaf. The
        // first edit's id allocations are visible to the second.
        let (mut g, rid, mesh) = fresh_scene_with_unbound_mesh_slot();
        let nid = g.allocate_unbound_node_id();
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([4.0, 5.0, 6.0]));
        let compound = SceneEdit::Compound(vec![
            SceneEdit::AddMeshResource {
                rid,
                mesh: Arc::clone(&mesh),
            },
            SceneEdit::AddRootLeaf {
                node_id: nid,
                mesh: rid,
                local_transform: xform,
            },
        ]);
        let snapshot_before = g.clone();
        compound.apply_to_scene(&mut g);
        assert!(g.roots().contains(&nid));
        assert!(g.mesh_resource(rid).is_some());
        compound.inverse().apply_to_scene(&mut g);
        assert_eq!(g, snapshot_before);
    }
}
