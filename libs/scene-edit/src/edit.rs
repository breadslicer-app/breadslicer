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

use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_scene::node::{
    LeafAttachment, LeafPayload, MeshResourceId, NodeId, NodeKind, SceneMetadata,
};
use sc_scene::{Scene, SceneError};

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
    /// Install a leaf at `node_id` with the given payload,
    /// attachment, and transform. Generalises [`Self::AddRootLeaf`]
    /// across inline-mesh / mesh-resource payloads and across
    /// root / parented attachments. The id must come from
    /// [`Scene::allocate_unbound_node_id`] so the arena slot exists
    /// and is empty when the edit is applied.
    AddLeaf {
        node_id: NodeId,
        attachment: LeafAttachment,
        payload: LeafPayload,
        local_transform: Transform3D,
        meta: Option<SceneMetadata>,
    },
    /// Tombstone the leaf at `node_id`. `attachment`, `payload`,
    /// `local_transform`, and `meta` snapshot the state so the
    /// inverse [`Self::AddLeaf`] re-installs it identically,
    /// including the position within the parent's children list and
    /// the leaf's name.
    RemoveLeaf {
        node_id: NodeId,
        attachment: LeafAttachment,
        payload: LeafPayload,
        local_transform: Transform3D,
        meta: Option<SceneMetadata>,
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
    /// Swap the mesh stored at the live resource slot `rid` from
    /// `before` to `after`. Both `Arc`s are snapshotted, so the
    /// inverse swap restores the exact previous storage, pointer
    /// identity included.
    ReplaceMeshResource {
        rid: MeshResourceId,
        before: Arc<IndexedMesh>,
        after: Arc<IndexedMesh>,
    },
    /// Swap the inline mesh of the `Leaf(Mesh(_))` node at `node_id`
    /// from `before` to `after`. Both `Arc`s are snapshotted, so the
    /// inverse swap restores the exact previous storage, pointer
    /// identity included.
    ReplaceLeafMesh {
        node_id: NodeId,
        before: Arc<IndexedMesh>,
        after: Arc<IndexedMesh>,
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
            // Pointer identity, not content equality: the stored
            // `Arc` is observable state (`find_mesh_resource_by_arc`,
            // cache keys), so swapping in a content-equal but
            // distinct `Arc` IS a change.
            Self::ReplaceMeshResource { before, after, .. }
            | Self::ReplaceLeafMesh { before, after, .. } => Arc::ptr_eq(before, after),
            Self::AddRootLeaf { .. }
            | Self::RemoveRootLeaf { .. }
            | Self::AddLeaf { .. }
            | Self::RemoveLeaf { .. }
            | Self::AddMeshResource { .. }
            | Self::RemoveMeshResource { .. } => false,
            Self::Compound(edits) => edits.iter().all(Self::is_noop),
        }
    }

    /// True when applying this edit changes the node or
    /// mesh-resource arena membership (a leaf appears or
    /// disappears, or a mesh resource is registered or removed) or
    /// swaps the mesh stored at an existing slot (the vertex data
    /// behind the slot changes). `SetLocalTransform` is
    /// non-structural: it perturbs an existing leaf's transform
    /// without touching arena slots or their meshes.
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
            | Self::AddLeaf { .. }
            | Self::RemoveLeaf { .. }
            | Self::AddMeshResource { .. }
            | Self::RemoveMeshResource { .. }
            | Self::ReplaceMeshResource { .. }
            | Self::ReplaceLeafMesh { .. } => true,
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
            Self::AddLeaf {
                node_id,
                attachment,
                payload,
                local_transform,
                meta,
            } => Self::RemoveLeaf {
                node_id: *node_id,
                attachment: *attachment,
                payload: payload.clone(),
                local_transform: *local_transform,
                meta: meta.clone(),
            },
            Self::RemoveLeaf {
                node_id,
                attachment,
                payload,
                local_transform,
                meta,
            } => Self::AddLeaf {
                node_id: *node_id,
                attachment: *attachment,
                payload: payload.clone(),
                local_transform: *local_transform,
                meta: meta.clone(),
            },
            Self::AddMeshResource { rid, mesh } => Self::RemoveMeshResource {
                rid: *rid,
                mesh: Arc::clone(mesh),
            },
            Self::RemoveMeshResource { rid, mesh } => Self::AddMeshResource {
                rid: *rid,
                mesh: Arc::clone(mesh),
            },
            Self::ReplaceMeshResource { rid, before, after } => Self::ReplaceMeshResource {
                rid: *rid,
                before: Arc::clone(after),
                after: Arc::clone(before),
            },
            Self::ReplaceLeafMesh {
                node_id,
                before,
                after,
            } => Self::ReplaceLeafMesh {
                node_id: *node_id,
                before: Arc::clone(after),
                after: Arc::clone(before),
            },
            Self::Compound(edits) => {
                Self::Compound(edits.iter().rev().map(Self::inverse).collect())
            }
        }
    }

    /// Apply this edit to a [`Scene`]. Each variant calls the
    /// matching scene mutator and propagates its precondition error
    /// (dangling id, occupied slot, kind mismatch); on error the
    /// scene is unchanged. A [`Self::Compound`] is atomic: when an
    /// inner edit fails, the already-applied prefix is undone before
    /// the error is returned, so a failed Compound also leaves the
    /// scene unchanged.
    pub fn apply_to_scene(&self, graph: &mut Scene) -> Result<(), SceneError> {
        match self {
            Self::SetLocalTransform { node_id, after, .. } => {
                graph.set_local_transform(*node_id, *after)
            }
            Self::AddRootLeaf {
                node_id,
                mesh,
                local_transform,
            } => graph.install_root_leaf(*node_id, *mesh, *local_transform),
            Self::RemoveRootLeaf { node_id, .. } => graph.remove_root_leaf(*node_id).map(|_| ()),
            Self::AddLeaf {
                node_id,
                attachment,
                payload,
                local_transform,
                meta,
            } => graph.install_leaf_at(
                *node_id,
                *attachment,
                payload.clone(),
                *local_transform,
                meta.clone(),
            ),
            Self::RemoveLeaf { node_id, .. } => graph.remove_leaf(*node_id).map(|_| ()),
            Self::AddMeshResource { rid, mesh } => {
                graph.install_mesh_resource_at(*rid, Arc::clone(mesh))
            }
            Self::RemoveMeshResource { rid, .. } => graph.remove_mesh_resource(*rid).map(|_| ()),
            Self::ReplaceMeshResource { rid, after, .. } => graph
                .replace_mesh_resource(*rid, Arc::clone(after))
                .map(|_| ()),
            Self::ReplaceLeafMesh { node_id, after, .. } => graph
                .replace_inline_leaf_mesh(*node_id, Arc::clone(after))
                .map(|_| ()),
            Self::Compound(edits) => {
                for (applied, edit) in edits.iter().enumerate() {
                    if let Err(error) = edit.apply_to_scene(graph) {
                        // A prefix inverse applies against exactly the
                        // state its edit produced, so rollback cannot
                        // fail.
                        for done in edits[..applied].iter().rev() {
                            let _ = done.inverse().apply_to_scene(graph);
                        }
                        return Err(error);
                    }
                }
                Ok(())
            }
        }
    }

    /// Build the edit that removes the leaf at `node_id`, or `None`
    /// if the node is not a leaf or its graph attachment cannot be
    /// determined.
    ///
    /// The returned edit is:
    /// - a plain [`Self::RemoveLeaf`] for inline-mesh leaves, or for
    ///   mesh-resource leaves where at least one other live leaf
    ///   still references the same [`MeshResourceId`];
    /// - a `Compound([RemoveLeaf, RemoveMeshResource])` when a
    ///   mesh-resource leaf was the last live reference, so the
    ///   inverse `Compound([AddMeshResource, AddLeaf])` re-installs
    ///   both on undo.
    ///
    /// The compound order matters: `RemoveMeshResource` requires
    /// that no leaf still references the rid, and the preceding
    /// `RemoveLeaf` satisfies that precondition.
    pub fn remove_leaf_with_gc(graph: &Scene, node_id: NodeId) -> Option<SceneEdit> {
        let node = graph.node(node_id)?;
        let local_transform = node.local_transform;
        let meta = node.meta.clone();
        let payload = match &node.kind {
            NodeKind::Leaf(payload) => payload.clone(),
            NodeKind::Group { .. } | NodeKind::Csg { .. } | NodeKind::Reference { .. } => {
                return None;
            }
        };
        let attachment = if let Some(parent) = graph.parent_of(node_id) {
            let parent_node = graph.node(parent)?;
            let children = match &parent_node.kind {
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => children,
                NodeKind::Leaf(_) | NodeKind::Reference { .. } => return None,
            };
            let position = children.iter().position(|c| *c == node_id)?;
            LeafAttachment::Parent { parent, position }
        } else if graph.roots().contains(&node_id) {
            LeafAttachment::Root
        } else {
            return None;
        };
        let remove_leaf = SceneEdit::RemoveLeaf {
            node_id,
            attachment,
            payload: payload.clone(),
            local_transform,
            meta,
        };
        if let LeafPayload::MeshResource(rid) = payload
            && !graph.other_leaf_references_resource(rid, node_id)
        {
            let arc = Arc::clone(graph.mesh_resource_arc(rid)?);
            return Some(SceneEdit::Compound(vec![
                remove_leaf,
                SceneEdit::RemoveMeshResource { rid, mesh: arc },
            ]));
        }
        Some(remove_leaf)
    }
}

#[cfg(test)]
mod scene_edit_tests {
    use super::*;
    use sc_mesh_core::transform::AffineTransform3D;

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
        let mut g = Scene::new();
        let edit = SceneEdit::AddRootLeaf {
            node_id: g.allocate_unbound_node_id(),
            mesh: g.allocate_unbound_mesh_resource_id(),
            local_transform: Transform3D::Identity,
        };
        assert!(edit.is_structural());
    }

    #[test]
    fn compound_is_structural_iff_any_inner_is() {
        let mut g = Scene::new();
        let nid = g.allocate_unbound_node_id();
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
                mesh: g.allocate_unbound_mesh_resource_id(),
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
        .apply_to_scene(&mut g)
        .expect("apply");
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
        edit.apply_to_scene(&mut g).expect("apply");
        edit.inverse().apply_to_scene(&mut g).expect("apply");
        assert_eq!(g.node(id).unwrap().local_transform, before);
    }

    #[test]
    fn set_local_transform_on_dangling_node_id_errors_and_changes_nothing() {
        let mut g = Scene::new();
        // An id allocated by another scene is out of range for `g`,
        // whose arena is empty.
        let bogus = Scene::new().allocate_unbound_node_id();
        let err = SceneEdit::SetLocalTransform {
            node_id: bogus,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        }
        .apply_to_scene(&mut g)
        .unwrap_err();
        assert!(matches!(err, sc_scene::SceneError::DanglingNodeId { id } if id == bogus));
        assert!(g.node(bogus).is_none());
    }

    fn fresh_scene_with_one_resource() -> (Scene, NodeId, MeshResourceId, Transform3D) {
        use sc_mesh_core::indexed_mesh::IndexedMesh;
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
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
        edit.apply_to_scene(&mut g).expect("apply");
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
        edit.apply_to_scene(&mut g).expect("apply");
        edit.inverse().apply_to_scene(&mut g).expect("apply");
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
        edit.apply_to_scene(&mut g).expect("apply");
        edit.inverse().apply_to_scene(&mut g).expect("apply");
        assert_eq!(g, snapshot_after_install);
    }

    #[test]
    fn add_root_leaf_with_out_of_range_id_errors_and_changes_nothing() {
        let mut g = Scene::new();
        // Ids allocated by another scene are out of range for `g`,
        // whose arenas are empty.
        let mut scratch = Scene::new();
        let bogus = scratch.allocate_unbound_node_id();
        let bogus_rid = scratch.allocate_unbound_mesh_resource_id();
        let err = SceneEdit::AddRootLeaf {
            node_id: bogus,
            mesh: bogus_rid,
            local_transform: Transform3D::Identity,
        }
        .apply_to_scene(&mut g)
        .unwrap_err();
        assert!(matches!(err, sc_scene::SceneError::DanglingNodeId { id } if id == bogus));
        assert!(g.node(bogus).is_none());
        assert!(g.roots().is_empty());
    }

    #[test]
    fn remove_root_leaf_on_empty_slot_errors_and_changes_nothing() {
        let (mut g, id, rid, xform) = fresh_scene_with_one_resource();
        let err = SceneEdit::RemoveRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        }
        .apply_to_scene(&mut g)
        .unwrap_err();
        assert!(matches!(err, sc_scene::SceneError::DanglingNodeId { id: e } if e == id));
        assert!(g.node(id).is_none());
        assert!(!g.roots().contains(&id));
    }

    fn fresh_scene_with_unbound_mesh_slot() -> (Scene, MeshResourceId, Arc<IndexedMesh>) {
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
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
        .apply_to_scene(&mut g)
        .expect("apply");
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
        edit.apply_to_scene(&mut g).expect("apply");
        edit.inverse().apply_to_scene(&mut g).expect("apply");
        assert_eq!(g, snapshot_before);
    }

    fn triangle_mesh_arc() -> Arc<IndexedMesh> {
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
        Arc::new(IndexedMesh {
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
        })
    }

    /// A live resource slot and a live inline-mesh leaf, each holding
    /// its own `Arc`: the 2 storage shapes the Replace* edits target.
    fn scene_with_replaceable_targets() -> (
        Scene,
        MeshResourceId,
        Arc<IndexedMesh>,
        NodeId,
        Arc<IndexedMesh>,
    ) {
        let mut g = Scene::new();
        let resource_mesh = triangle_mesh_arc();
        let rid = g.add_mesh_resource_arc(Arc::clone(&resource_mesh));
        let leaf_mesh = triangle_mesh_arc();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::clone(&leaf_mesh)),
            )
            .unwrap();
        (g, rid, resource_mesh, leaf, leaf_mesh)
    }

    #[test]
    fn replace_edits_are_noop_only_on_pointer_identical_arcs() {
        let (_, rid, resource_mesh, leaf, leaf_mesh) = scene_with_replaceable_targets();
        let same_arc = SceneEdit::ReplaceMeshResource {
            rid,
            before: Arc::clone(&resource_mesh),
            after: Arc::clone(&resource_mesh),
        };
        assert!(same_arc.is_noop());
        // Content-equal but pointer-distinct is a real change: the
        // stored Arc's identity is observable state.
        let content_equal = Arc::new((*resource_mesh).clone());
        let distinct = SceneEdit::ReplaceMeshResource {
            rid,
            before: Arc::clone(&resource_mesh),
            after: content_equal,
        };
        assert!(!distinct.is_noop());
        let same_leaf_arc = SceneEdit::ReplaceLeafMesh {
            node_id: leaf,
            before: Arc::clone(&leaf_mesh),
            after: Arc::clone(&leaf_mesh),
        };
        assert!(same_leaf_arc.is_noop());
        let distinct_leaf = SceneEdit::ReplaceLeafMesh {
            node_id: leaf,
            before: Arc::clone(&leaf_mesh),
            after: Arc::new((*leaf_mesh).clone()),
        };
        assert!(!distinct_leaf.is_noop());
    }

    #[test]
    fn replace_edits_are_structural() {
        let (_, rid, resource_mesh, leaf, leaf_mesh) = scene_with_replaceable_targets();
        let resource_edit = SceneEdit::ReplaceMeshResource {
            rid,
            before: resource_mesh,
            after: triangle_mesh_arc(),
        };
        assert!(resource_edit.is_structural());
        let leaf_edit = SceneEdit::ReplaceLeafMesh {
            node_id: leaf,
            before: leaf_mesh,
            after: triangle_mesh_arc(),
        };
        assert!(leaf_edit.is_structural());
    }

    #[test]
    fn replace_inverse_swaps_before_and_after_preserving_arc_identity() {
        let (_, rid, before, leaf, leaf_before) = scene_with_replaceable_targets();
        let after = triangle_mesh_arc();
        let SceneEdit::ReplaceMeshResource {
            rid: out_rid,
            before: out_before,
            after: out_after,
        } = SceneEdit::ReplaceMeshResource {
            rid,
            before: Arc::clone(&before),
            after: Arc::clone(&after),
        }
        .inverse()
        else {
            panic!("inverse of ReplaceMeshResource must be ReplaceMeshResource");
        };
        assert_eq!(out_rid, rid);
        assert!(Arc::ptr_eq(&out_before, &after));
        assert!(Arc::ptr_eq(&out_after, &before));
        let leaf_after = triangle_mesh_arc();
        let SceneEdit::ReplaceLeafMesh {
            node_id: out_id,
            before: out_before,
            after: out_after,
        } = SceneEdit::ReplaceLeafMesh {
            node_id: leaf,
            before: Arc::clone(&leaf_before),
            after: Arc::clone(&leaf_after),
        }
        .inverse()
        else {
            panic!("inverse of ReplaceLeafMesh must be ReplaceLeafMesh");
        };
        assert_eq!(out_id, leaf);
        assert!(Arc::ptr_eq(&out_before, &leaf_after));
        assert!(Arc::ptr_eq(&out_after, &leaf_before));
    }

    #[test]
    fn apply_replace_mesh_resource_then_inverse_restores_arc_identity() {
        let (mut g, rid, before, _, _) = scene_with_replaceable_targets();
        let after = triangle_mesh_arc();
        let edit = SceneEdit::ReplaceMeshResource {
            rid,
            before: Arc::clone(&before),
            after: Arc::clone(&after),
        };
        edit.apply_to_scene(&mut g).expect("apply");
        assert!(Arc::ptr_eq(g.mesh_resource_arc(rid).unwrap(), &after));
        edit.inverse().apply_to_scene(&mut g).expect("apply");
        assert!(Arc::ptr_eq(g.mesh_resource_arc(rid).unwrap(), &before));
    }

    #[test]
    fn apply_replace_leaf_mesh_then_inverse_restores_arc_identity() {
        let (mut g, _, _, leaf, before) = scene_with_replaceable_targets();
        let after = triangle_mesh_arc();
        let edit = SceneEdit::ReplaceLeafMesh {
            node_id: leaf,
            before: Arc::clone(&before),
            after: Arc::clone(&after),
        };
        edit.apply_to_scene(&mut g).expect("apply");
        assert!(Arc::ptr_eq(g.leaf_mesh_arc(leaf).unwrap(), &after));
        edit.inverse().apply_to_scene(&mut g).expect("apply");
        assert!(Arc::ptr_eq(g.leaf_mesh_arc(leaf).unwrap(), &before));
    }

    #[test]
    fn compound_is_noop_iff_all_inner_are_noop() {
        // Identity-only SetLocalTransform and same-Arc Replace*
        // edits are the only noop shapes in the enum's vocabulary;
        // everything else is unconditionally mutating.
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
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
        compound.apply_to_scene(&mut g).expect("apply");
        assert!(g.roots().contains(&nid));
        assert!(g.mesh_resource(rid).is_some());
        compound.inverse().apply_to_scene(&mut g).expect("apply");
        assert_eq!(g, snapshot_before);
    }

    #[test]
    fn compound_failure_rolls_back_the_applied_prefix() {
        // The second edit targets an occupied slot, so it fails after
        // the first has installed the mesh resource. The Compound must
        // return that error with the resource uninstalled again: a
        // failed Compound leaves the scene exactly as it found it.
        let (mut g, rid, mesh) = fresh_scene_with_unbound_mesh_slot();
        let occupied = g.add_group(None, Transform3D::Identity);
        let snapshot_before = g.clone();
        let err = SceneEdit::Compound(vec![
            SceneEdit::AddMeshResource {
                rid,
                mesh: Arc::clone(&mesh),
            },
            SceneEdit::AddRootLeaf {
                node_id: occupied,
                mesh: rid,
                local_transform: Transform3D::Identity,
            },
        ])
        .apply_to_scene(&mut g)
        .unwrap_err();
        assert!(matches!(
            err,
            sc_scene::SceneError::NodeSlotOccupied { id } if id == occupied
        ));
        assert_eq!(g, snapshot_before);
    }

    fn triangle_arc() -> Arc<IndexedMesh> {
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
        Arc::new(IndexedMesh {
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
        })
    }

    #[test]
    fn add_leaf_and_remove_leaf_are_structural_and_not_noop() {
        let mut g = Scene::new();
        let nid = g.allocate_unbound_node_id();
        let add = SceneEdit::AddLeaf {
            node_id: nid,
            attachment: LeafAttachment::Root,
            payload: LeafPayload::Mesh(triangle_arc()),
            local_transform: Transform3D::Identity,
            meta: None,
        };
        let remove = SceneEdit::RemoveLeaf {
            node_id: nid,
            attachment: LeafAttachment::Root,
            payload: LeafPayload::Mesh(triangle_arc()),
            local_transform: Transform3D::Identity,
            meta: None,
        };
        assert!(add.is_structural());
        assert!(!add.is_noop());
        assert!(remove.is_structural());
        assert!(!remove.is_noop());
        let _ = g.allocate_unbound_node_id();
    }

    #[test]
    fn add_leaf_inverse_is_remove_leaf_with_same_fields_and_vice_versa() {
        let mut g = Scene::new();
        let nid = g.allocate_unbound_node_id();
        let attachment = LeafAttachment::Parent {
            parent: g.allocate_unbound_node_id(),
            position: 3,
        };
        let payload = LeafPayload::Mesh(triangle_arc());
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let add = SceneEdit::AddLeaf {
            node_id: nid,
            attachment,
            payload: payload.clone(),
            local_transform: xform,
            meta: None,
        };
        match add.inverse() {
            SceneEdit::RemoveLeaf {
                node_id,
                attachment: a,
                payload: p,
                local_transform,
                meta,
            } => {
                assert_eq!(node_id, nid);
                assert_eq!(a, attachment);
                assert_eq!(p, payload);
                assert_eq!(local_transform, xform);
                assert_eq!(meta, None);
            }
            _ => panic!("AddLeaf inverse must be RemoveLeaf"),
        }
        let remove = SceneEdit::RemoveLeaf {
            node_id: nid,
            attachment,
            payload: payload.clone(),
            local_transform: xform,
            meta: None,
        };
        assert!(matches!(remove.inverse(), SceneEdit::AddLeaf { .. }));
    }

    #[test]
    fn add_leaf_apply_installs_a_root_inline_leaf() {
        let mut g = Scene::new();
        let nid = g.allocate_unbound_node_id();
        let arc = triangle_arc();
        let edit = SceneEdit::AddLeaf {
            node_id: nid,
            attachment: LeafAttachment::Root,
            payload: LeafPayload::Mesh(Arc::clone(&arc)),
            local_transform: Transform3D::Identity,
            meta: None,
        };
        edit.apply_to_scene(&mut g).expect("apply");
        assert!(g.roots().contains(&nid));
        let observed = g.leaf_mesh_arc(nid).map(Arc::as_ptr);
        assert_eq!(observed, Some(Arc::as_ptr(&arc)));
    }

    #[test]
    fn add_leaf_apply_installs_a_parented_inline_leaf_at_the_recorded_position() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        g.add_root(parent).unwrap();
        // Pre-existing sibling so the inserted leaf at position 0
        // shifts it to position 1.
        let sibling = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(triangle_arc()),
            )
            .unwrap();
        g.add_child(parent, sibling).unwrap();
        let new_id = g.allocate_unbound_node_id();
        let edit = SceneEdit::AddLeaf {
            node_id: new_id,
            attachment: LeafAttachment::Parent {
                parent,
                position: 0,
            },
            payload: LeafPayload::Mesh(triangle_arc()),
            local_transform: Transform3D::Identity,
            meta: None,
        };
        edit.apply_to_scene(&mut g).expect("apply");
        let sc_scene::NodeKind::Group { children } = &g.node(parent).unwrap().kind else {
            panic!("parent should still be a group");
        };
        assert_eq!(children.as_slice(), &[new_id, sibling]);
    }

    #[test]
    fn add_leaf_apply_then_inverse_round_trips_for_a_root_leaf() {
        let mut g = Scene::new();
        let nid = g.allocate_unbound_node_id();
        let snapshot_before = g.clone();
        let edit = SceneEdit::AddLeaf {
            node_id: nid,
            attachment: LeafAttachment::Root,
            payload: LeafPayload::Mesh(triangle_arc()),
            local_transform: Transform3D::Identity,
            meta: None,
        };
        edit.apply_to_scene(&mut g).expect("apply");
        edit.inverse().apply_to_scene(&mut g).expect("apply");
        assert_eq!(g, snapshot_before);
    }

    #[test]
    fn remove_leaf_apply_then_inverse_round_trips_for_a_parented_leaf() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        g.add_root(parent).unwrap();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(triangle_arc()),
            )
            .unwrap();
        g.add_child(parent, leaf).unwrap();
        let snapshot_before = g.clone();
        let edit = SceneEdit::RemoveLeaf {
            node_id: leaf,
            attachment: LeafAttachment::Parent {
                parent,
                position: 0,
            },
            payload: LeafPayload::Mesh(triangle_arc()),
            local_transform: Transform3D::Identity,
            meta: None,
        };
        edit.apply_to_scene(&mut g).expect("apply");
        assert!(g.node(leaf).is_none());
        edit.inverse().apply_to_scene(&mut g).expect("apply");
        assert_eq!(g, snapshot_before);
    }

    #[test]
    fn remove_leaf_then_inverse_restores_the_leaf_meta() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let meta = SceneMetadata {
            name: Some("Bracket".to_string()),
            comment: None,
        };
        let leaf = g
            .add_leaf(
                Some(meta.clone()),
                Transform3D::Identity,
                LeafPayload::Mesh(triangle_arc()),
            )
            .unwrap();
        g.add_child(root, leaf).unwrap();
        let snapshot_before = g.clone();
        let edit = SceneEdit::RemoveLeaf {
            node_id: leaf,
            attachment: LeafAttachment::Parent {
                parent: root,
                position: 0,
            },
            payload: LeafPayload::Mesh(triangle_arc()),
            local_transform: Transform3D::Identity,
            meta: Some(meta.clone()),
        };
        edit.apply_to_scene(&mut g).expect("apply");
        assert!(g.node(leaf).is_none());
        edit.inverse().apply_to_scene(&mut g).expect("apply");
        assert_eq!(g.node(leaf).unwrap().meta, Some(meta));
        assert_eq!(g, snapshot_before);
    }

    fn root_resource_leaf(g: &mut Scene, rid: MeshResourceId) -> NodeId {
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        g.add_root(leaf).unwrap();
        leaf
    }

    #[test]
    fn remove_leaf_with_gc_on_an_inline_leaf_is_a_plain_remove() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(triangle_arc()),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let edit = SceneEdit::remove_leaf_with_gc(&g, leaf).unwrap();
        assert!(matches!(
            edit,
            SceneEdit::RemoveLeaf { node_id, .. } if node_id == leaf
        ));
    }

    #[test]
    fn remove_leaf_with_gc_keeps_a_resource_another_leaf_references() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource_arc(triangle_arc());
        let _kept = root_resource_leaf(&mut g, rid);
        let doomed = root_resource_leaf(&mut g, rid);
        let edit = SceneEdit::remove_leaf_with_gc(&g, doomed).unwrap();
        assert!(matches!(
            edit,
            SceneEdit::RemoveLeaf { node_id, .. } if node_id == doomed
        ));
    }

    #[test]
    fn remove_leaf_with_gc_bundles_the_last_reference_into_a_compound() {
        let mut g = Scene::new();
        let arc = triangle_arc();
        let rid = g.add_mesh_resource_arc(Arc::clone(&arc));
        let leaf = g
            .add_leaf(
                Some(SceneMetadata {
                    name: Some("Named".to_string()),
                    comment: None,
                }),
                Transform3D::Identity,
                LeafPayload::MeshResource(rid),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let Some(SceneEdit::Compound(parts)) = SceneEdit::remove_leaf_with_gc(&g, leaf) else {
            panic!("the sole reference must produce a Compound");
        };
        assert_eq!(parts.len(), 2);
        // RemoveLeaf first: RemoveMeshResource requires that no leaf
        // still references the rid.
        assert!(matches!(
            &parts[0],
            SceneEdit::RemoveLeaf { node_id, attachment, meta, .. }
                if *node_id == leaf
                    && *attachment == LeafAttachment::Root
                    && meta.as_ref().and_then(|m| m.name.as_deref()) == Some("Named")
        ));
        let SceneEdit::RemoveMeshResource { rid: out_rid, mesh } = &parts[1] else {
            panic!("the second edit must remove the mesh resource");
        };
        assert_eq!(*out_rid, rid);
        assert!(Arc::ptr_eq(mesh, &arc));
    }

    #[test]
    fn remove_leaf_with_gc_on_a_non_leaf_or_foreign_id_is_none() {
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_root(group).unwrap();
        assert!(SceneEdit::remove_leaf_with_gc(&g, group).is_none());
        let mut scratch = Scene::new();
        for _ in 0..8 {
            scratch.add_group(None, Transform3D::Identity);
        }
        let foreign = scratch.allocate_unbound_node_id();
        assert!(SceneEdit::remove_leaf_with_gc(&g, foreign).is_none());
    }
}
