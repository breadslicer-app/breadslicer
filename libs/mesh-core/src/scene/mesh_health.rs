/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-wide mesh soundness sweep: which leaves carry a mesh that is
//! not a valid closed manifold. Used to decide whether a freshly loaded
//! scene is safe to edit and slice, and to point future repair tooling
//! at the leaves that need attention.

use std::num::NonZeroU32;

use crate::scene::Scene;
use crate::scene::node::{LeafPayload, NodeId, NodeKind};

impl Scene {
    /// Returns the ids of every leaf whose mesh is not a valid closed
    /// manifold. A leaf is flagged when its mesh fails face validation
    /// (out-of-range indices or zero-length edges) or the manifold
    /// check (boundary edges / holes, edges shared by three or more
    /// faces, or inconsistent winding). An empty result means every
    /// leaf is a clean watertight manifold. Face-invalid meshes are
    /// included because a mesh with invalid faces cannot be a manifold.
    ///
    /// Both inline-mesh leaves and mesh-resource leaves are walked;
    /// mesh-resource leaves referencing a dangling slot are skipped
    /// (the structural validator covers that variant). A resource mesh
    /// shared by several leaves is validated once per referencing leaf.
    /// The result is sorted by id and contains no duplicates (leaf ids
    /// are unique per slot).
    ///
    /// Complexity: O(total face count) time; each mesh is a single
    /// half-edge pass.
    pub fn non_manifold_leaves(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        for (slot_idx, slot) in self.nodes.iter().enumerate() {
            let Some(node) = slot else { continue };
            let NodeKind::Leaf(payload) = &node.kind else {
                continue;
            };
            let mesh = match payload {
                LeafPayload::Mesh(arc) => arc.as_ref(),
                LeafPayload::MeshResource(rid) => match self.mesh_resource(*rid) {
                    Some(mesh) => mesh,
                    None => continue,
                },
            };
            // A leaf is sound iff the manifold witness can be minted:
            // in-range faces first, then the manifold upgrade. The two
            // validators carry different error types, so this is an
            // `is_ok_and` rather than a `?`/`and_then` chain.
            let is_valid_manifold = mesh
                .validated_faces()
                .is_ok_and(|faces| faces.validated_manifold().is_ok());
            if !is_valid_manifold {
                out.push(leaf_id_at(slot_idx));
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
    use crate::indexed_mesh::IndexedMesh;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::transform::Transform3D;

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
    fn empty_scene_has_no_non_manifold_leaves() {
        assert!(Scene::new().non_manifold_leaves().is_empty());
    }

    #[test]
    fn a_closed_manifold_leaf_is_not_flagged() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(&mut graph, root, tetrahedron_mesh());
        assert!(graph.non_manifold_leaves().is_empty());
    }

    #[test]
    fn an_open_mesh_leaf_is_flagged() {
        let (mut graph, root) = scene_with_root();
        let leaf = add_inline_leaf(&mut graph, root, open_triangle_mesh());
        assert_eq!(graph.non_manifold_leaves(), vec![leaf]);
    }

    #[test]
    fn only_the_defective_leaf_is_reported() {
        let (mut graph, root) = scene_with_root();
        add_inline_leaf(&mut graph, root, tetrahedron_mesh());
        let bad = add_inline_leaf(&mut graph, root, open_triangle_mesh());
        assert_eq!(graph.non_manifold_leaves(), vec![bad]);
    }

    #[test]
    fn a_mesh_resource_leaf_is_checked_too() {
        let (mut graph, root) = scene_with_root();
        let rid = graph.add_mesh_resource(open_triangle_mesh());
        let leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, leaf).unwrap();
        assert_eq!(graph.non_manifold_leaves(), vec![leaf]);
    }
}
