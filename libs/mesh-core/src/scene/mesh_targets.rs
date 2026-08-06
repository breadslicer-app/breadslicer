/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The scene-wide mesh traversal: one definition of "which meshes
//! does this scene store", shared by the health sweep and the
//! scene-level repair / diagnosis drivers so they can never disagree
//! about the target set. Arena-wide: inline leaves inside registered
//! subtrees (and any other detached node) count; meshes held only by
//! external `Arc`s (undo history, clipboard) do not.

use crate::indexed_mesh::IndexedMesh;
use crate::scene::node::{LeafPayload, MeshResourceId, NodeId, NodeKind};
use crate::scene::{Scene, node_id_at_index};

/// One place a [`Scene`] stores a mesh: a slot in the mesh-resource
/// arena, or an inline-mesh leaf in the node arena. A resource shared
/// by several leaves is a single target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeshTarget {
    /// A live slot in the mesh-resource arena.
    Resource(MeshResourceId),
    /// A leaf node carrying its mesh inline.
    InlineLeaf(NodeId),
}

impl Scene {
    /// Every place this scene stores a mesh, in deterministic order:
    /// live mesh-resource slots in arena order, then inline-mesh
    /// leaves in node-arena slot order. Resource-backed leaves
    /// contribute through their resource, so nothing is listed twice.
    pub fn mesh_targets(&self) -> Vec<MeshTarget> {
        let mut targets: Vec<MeshTarget> = self
            .mesh_resources_iter()
            .map(|(rid, _)| MeshTarget::Resource(rid))
            .collect();
        for (i, slot) in self.nodes.iter().enumerate() {
            let Some(node) = slot else { continue };
            if matches!(&node.kind, NodeKind::Leaf(LeafPayload::Mesh(_))) {
                targets.push(MeshTarget::InlineLeaf(node_id_at_index(i)));
            }
        }
        targets
    }

    /// The storage target behind the leaf at `id`: an inline-mesh
    /// leaf maps to itself, a resource-backed leaf to its resource
    /// id. `None` for non-leaf nodes and dangling ids. A leaf
    /// referencing a tombstoned resource slot still maps to
    /// `Resource(rid)`; what a dead target means is the caller's
    /// decision.
    pub fn leaf_mesh_target(&self, id: NodeId) -> Option<MeshTarget> {
        match &self.node(id)?.kind {
            NodeKind::Leaf(LeafPayload::Mesh(_)) => Some(MeshTarget::InlineLeaf(id)),
            NodeKind::Leaf(LeafPayload::MeshResource(rid)) => Some(MeshTarget::Resource(*rid)),
            NodeKind::Group { .. } | NodeKind::Csg { .. } | NodeKind::Reference { .. } => None,
        }
    }

    /// Every leaf whose mesh is stored at `target`, in node-arena
    /// slot order. An inline target lists exactly its own leaf; a
    /// resource target lists each referencing leaf.
    pub fn leaves_referencing(&self, target: MeshTarget) -> Vec<NodeId> {
        let mut out = Vec::new();
        for (i, slot) in self.nodes.iter().enumerate() {
            let Some(node) = slot else { continue };
            let NodeKind::Leaf(payload) = &node.kind else {
                continue;
            };
            let id = node_id_at_index(i);
            let references = match (payload, target) {
                (LeafPayload::Mesh(_), MeshTarget::InlineLeaf(target_id)) => id == target_id,
                (LeafPayload::MeshResource(rid), MeshTarget::Resource(target_rid)) => {
                    *rid == target_rid
                }
                (LeafPayload::Mesh(_), MeshTarget::Resource(_))
                | (LeafPayload::MeshResource(_), MeshTarget::InlineLeaf(_)) => false,
            };
            if references {
                out.push(id);
            }
        }
        out
    }

    /// Borrow the mesh stored at `target`. `None` when the target is
    /// dead: a tombstoned resource slot, or an id that is not a live
    /// inline-mesh leaf.
    pub fn target_mesh(&self, target: MeshTarget) -> Option<&IndexedMesh> {
        match target {
            MeshTarget::Resource(rid) => self.mesh_resource(rid),
            MeshTarget::InlineLeaf(id) => match &self.node(id)?.kind {
                NodeKind::Leaf(LeafPayload::Mesh(arc)) => Some(arc.as_ref()),
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod mesh_targets_tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use super::*;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::transform::Transform3D;

    fn triangle_mesh() -> IndexedMesh {
        IndexedMesh {
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
        }
    }

    /// A scene exercising every storage shape at once: 1 resource
    /// shared by 2 leaves, 1 reachable inline leaf, and 1 inline leaf
    /// inside a registered subtree.
    fn combined_scene() -> (Scene, MeshResourceId, NodeId, NodeId, NodeId, NodeId) {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let inline = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(triangle_mesh())),
            )
            .unwrap();
        graph.add_child(root, inline).unwrap();
        let rid = graph.add_mesh_resource(triangle_mesh());
        let shared_a = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, shared_a).unwrap();
        let shared_b = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, shared_b).unwrap();
        let sub_root = graph.add_group(None, Transform3D::Identity);
        let sub_leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(triangle_mesh())),
            )
            .unwrap();
        graph.add_child(sub_root, sub_leaf).unwrap();
        graph.register_subtree_resource(sub_root).unwrap();
        (graph, rid, inline, shared_a, shared_b, sub_leaf)
    }

    #[test]
    fn empty_scene_has_no_targets() {
        assert!(Scene::new().mesh_targets().is_empty());
    }

    #[test]
    fn targets_list_resources_then_inline_leaves_in_slot_order() {
        let (graph, rid, inline, _, _, sub_leaf) = combined_scene();
        assert_eq!(
            graph.mesh_targets(),
            vec![
                MeshTarget::Resource(rid),
                MeshTarget::InlineLeaf(inline),
                MeshTarget::InlineLeaf(sub_leaf),
            ]
        );
    }

    #[test]
    fn leaf_mesh_target_resolves_every_leaf_shape() {
        let (graph, rid, inline, shared_a, _, sub_leaf) = combined_scene();
        assert_eq!(
            graph.leaf_mesh_target(inline),
            Some(MeshTarget::InlineLeaf(inline))
        );
        assert_eq!(
            graph.leaf_mesh_target(shared_a),
            Some(MeshTarget::Resource(rid))
        );
        assert_eq!(
            graph.leaf_mesh_target(sub_leaf),
            Some(MeshTarget::InlineLeaf(sub_leaf))
        );
    }

    #[test]
    fn leaf_mesh_target_is_none_for_groups_and_dangling_ids() {
        let mut graph = Scene::new();
        let group = graph.add_group(None, Transform3D::Identity);
        assert_eq!(graph.leaf_mesh_target(group), None);
        let dangling = NodeId(NonZeroU32::new(99).unwrap());
        assert_eq!(graph.leaf_mesh_target(dangling), None);
    }

    #[test]
    fn leaves_referencing_maps_a_shared_resource_to_every_leaf() {
        let (graph, rid, inline, shared_a, shared_b, _) = combined_scene();
        assert_eq!(
            graph.leaves_referencing(MeshTarget::Resource(rid)),
            vec![shared_a, shared_b]
        );
        assert_eq!(
            graph.leaves_referencing(MeshTarget::InlineLeaf(inline)),
            vec![inline]
        );
    }

    #[test]
    fn leaves_referencing_an_unreferenced_resource_is_empty() {
        let mut graph = Scene::new();
        let rid = graph.add_mesh_resource(triangle_mesh());
        assert!(
            graph
                .leaves_referencing(MeshTarget::Resource(rid))
                .is_empty()
        );
    }

    #[test]
    fn target_mesh_resolves_live_targets_and_rejects_dead_ones() {
        let (graph, rid, inline, shared_a, _, _) = combined_scene();
        assert!(graph.target_mesh(MeshTarget::Resource(rid)).is_some());
        assert!(graph.target_mesh(MeshTarget::InlineLeaf(inline)).is_some());
        // A resource-backed leaf's id is not an inline target.
        assert!(
            graph
                .target_mesh(MeshTarget::InlineLeaf(shared_a))
                .is_none()
        );
        let dangling = NodeId(NonZeroU32::new(99).unwrap());
        assert!(
            graph
                .target_mesh(MeshTarget::InlineLeaf(dangling))
                .is_none()
        );
    }
}
