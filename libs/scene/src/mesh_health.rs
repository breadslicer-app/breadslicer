/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-wide mesh soundness sweep: which leaves carry a mesh that is
//! not closed and edge-manifold. Used to decide whether a freshly
//! loaded scene is safe to edit and slice, and to point future repair
//! tooling at the leaves that need attention.

use std::collections::HashMap;
use std::num::NonZeroU32;

use crate::Scene;
use crate::mesh_targets::MeshTarget;
use crate::node::{NodeId, NodeKind};

impl Scene {
    /// Returns the ids of every leaf whose mesh is not closed and
    /// edge-manifold. A leaf is flagged when its mesh fails face
    /// validation (out-of-range indices or zero-length edges) or the
    /// edge-manifold check (boundary edges / holes, edges shared by
    /// three or more faces, or inconsistent winding). An empty result
    /// means every leaf is watertight and edge-manifold; a
    /// vertex-pinched mesh is NOT flagged, since it bounds a valid
    /// solid and slices fine. Face-invalid meshes are included
    /// because a mesh with invalid faces cannot be edge-manifold.
    ///
    /// The sweep runs over [`Scene::mesh_targets`], so its scope is
    /// the same as the repair and diagnosis drivers': every stored
    /// mesh, arena-wide. Each target is validated once and its
    /// verdict shared by every referencing leaf; mesh-resource leaves
    /// referencing a dangling slot are skipped (the structural
    /// validator covers that variant). The result is sorted by id and
    /// contains no duplicates (leaf ids are unique per slot).
    ///
    /// Complexity: O(total face count) time; each stored mesh is a
    /// single half-edge pass.
    pub fn non_edge_manifold_leaves(&self) -> Vec<NodeId> {
        // A target is sound iff the edge-manifold witness can be
        // minted: in-range faces first, then the edge-manifold
        // upgrade. The two validators carry different error types,
        // so this is an `is_ok_and` rather than a `?`/`and_then`
        // chain.
        let verdicts: HashMap<MeshTarget, bool> = self
            .mesh_targets()
            .into_iter()
            .map(|target| {
                let is_edge_manifold = self.target_mesh(target).is_some_and(|mesh| {
                    mesh.validated_faces()
                        .is_ok_and(|faces| faces.validated_edge_manifold().is_ok())
                });
                (target, is_edge_manifold)
            })
            .collect();

        let mut out = Vec::new();
        for (slot_idx, slot) in self.nodes.iter().enumerate() {
            let Some(node) = slot else { continue };
            if !matches!(&node.kind, NodeKind::Leaf(_)) {
                continue;
            }
            let id = leaf_id_at(slot_idx);
            let Some(target) = self.leaf_mesh_target(id) else {
                continue;
            };
            // A dangling resource target never entered the verdict
            // map; the leaf is skipped, as before.
            if verdicts.get(&target) == Some(&false) {
                out.push(id);
            }
        }
        out
    }
}

fn leaf_id_at(slot_idx: usize) -> NodeId {
    let raw = u32::try_from(slot_idx + 1).expect("arena slot fits in u32");
    NodeId(NonZeroU32::new(raw).expect("slot index + 1 is non-zero"))
}

#[cfg(test)]
mod mesh_health_tests {
    use std::sync::Arc;

    use super::*;
    use crate::node::LeafPayload;
    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::transform::Transform3D;

    /// A closed, consistently wound tetrahedron: a valid manifold. The
    /// stored normal is irrelevant to the manifold check (which looks
    /// only at topology and winding), so a placeholder is used.
    fn tetrahedron_mesh() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
            Vertex::new([-1.0, 1.0, -1.0]),
            Vertex::new([-1.0, -1.0, 1.0]),
        ];
        let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
            .into_iter()
            .map(|indices| IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), indices))
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// A lone triangle: every edge is a boundary edge, so it is not a
    /// closed manifold.
    fn open_triangle_mesh() -> IndexedMesh {
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

    fn add_inline_leaf(graph: &mut Scene, parent: NodeId, mesh: IndexedMesh) -> NodeId {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        graph.add_child(parent, leaf).unwrap();
        leaf
    }

    fn scene_with_root() -> (Scene, NodeId) {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        (graph, root)
    }

    #[test]
    fn empty_scene_has_no_non_edge_manifold_leaves() {
        assert!(Scene::new().non_edge_manifold_leaves().is_empty());
    }

    #[test]
    fn a_closed_manifold_leaf_is_not_flagged() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(&mut graph, root, tetrahedron_mesh());
        assert!(graph.non_edge_manifold_leaves().is_empty());
    }

    #[test]
    fn an_open_mesh_leaf_is_flagged() {
        let (mut graph, root) = scene_with_root();
        let leaf = add_inline_leaf(&mut graph, root, open_triangle_mesh());
        assert_eq!(graph.non_edge_manifold_leaves(), vec![leaf]);
    }

    #[test]
    fn only_the_defective_leaf_is_reported() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(&mut graph, root, tetrahedron_mesh());
        let bad = add_inline_leaf(&mut graph, root, open_triangle_mesh());
        assert_eq!(graph.non_edge_manifold_leaves(), vec![bad]);
    }

    #[test]
    fn a_mesh_resource_leaf_is_checked_too() {
        let (mut graph, root) = scene_with_root();
        let rid = graph.add_mesh_resource(open_triangle_mesh());
        let leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, leaf).unwrap();
        assert_eq!(graph.non_edge_manifold_leaves(), vec![leaf]);
    }

    /// The sweep is arena-wide: a defective inline leaf inside a
    /// registered subtree (detached from the scene roots by
    /// construction) is flagged like any other leaf.
    #[test]
    fn a_registered_subtree_leaf_is_flagged() {
        let mut graph = Scene::new();
        let sub_root = graph.add_group(None, Transform3D::Identity);
        let leaf = add_inline_leaf(&mut graph, sub_root, open_triangle_mesh());
        graph.register_subtree_resource(sub_root).unwrap();
        assert_eq!(graph.non_edge_manifold_leaves(), vec![leaf]);
    }

    /// A defective resource shared by 2 leaves flags both of them, in
    /// node-arena slot order.
    #[test]
    fn a_shared_defective_resource_flags_every_referencing_leaf() {
        let (mut graph, root) = scene_with_root();
        let rid = graph.add_mesh_resource(open_triangle_mesh());
        let first = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, first).unwrap();
        let second = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, second).unwrap();
        assert_eq!(graph.non_edge_manifold_leaves(), vec![first, second]);
    }
}
