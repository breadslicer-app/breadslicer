/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Detects when two or more scene leaves carry coincident vertex
//! coordinates. The check is bit-exact on the f32 vertex tuple
//! (with -0.0 normalised to +0.0) so it surfaces the OBJ-style
//! "different SceneObjects share a global vertex" pattern. Surface
//! coincidence under floating noise is out of scope here; that would
//! be a spatial-index epsilon match.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroU32;

use crate::primitives::Vertex;
use crate::scene::Scene;
use crate::scene::node::{LeafPayload, NodeId, NodeKind};

impl Scene {
    /// Returns the ids of every leaf that shares at least one vertex
    /// (by bit-exact coordinate, with -0.0 normalised) with another
    /// leaf elsewhere in the scene. The result is sorted by id and
    /// contains no duplicates.
    ///
    /// Both inline-mesh leaves and mesh-resource leaves are walked.
    /// Mesh-resource leaves that reference a dangling slot are
    /// skipped; the structural validator at
    /// [`Scene::validate`](crate::scene::Scene::validate) handles
    /// that variant separately.
    ///
    /// Complexity: O(total vertex count) time, O(distinct vertex
    /// coordinate count) space.
    pub fn detect_shared_vertices(&self) -> Vec<NodeId> {
        let mut first_seen: HashMap<[u32; 3], NodeId> = HashMap::new();
        let mut shared: HashMap<NodeId, ()> = HashMap::new();

        for (slot_idx, slot) in self.nodes.iter().enumerate() {
            let Some(node) = slot else { continue };
            let NodeKind::Leaf(payload) = &node.kind else {
                continue;
            };
            let mesh_vertices: &[Vertex] = match payload {
                LeafPayload::Mesh(arc) => arc.vertices(),
                LeafPayload::MeshResource(rid) => match self.mesh_resource(*rid) {
                    Some(mesh) => mesh.vertices(),
                    None => continue,
                },
            };
            let leaf_id = leaf_id_at(slot_idx);
            for v in mesh_vertices {
                let bp = normalised_bit_pattern(v);
                match first_seen.entry(bp) {
                    Entry::Vacant(e) => {
                        e.insert(leaf_id);
                    }
                    Entry::Occupied(e) => {
                        let other = *e.get();
                        if other != leaf_id {
                            shared.insert(other, ());
                            shared.insert(leaf_id, ());
                        }
                    }
                }
            }
        }

        let mut out: Vec<NodeId> = shared.into_keys().collect();
        out.sort_by_key(|id| id.get());
        out
    }
}

fn normalised_bit_pattern(v: &Vertex) -> [u32; 3] {
    v.coords().map(|f| {
        if f == -0.0 {
            0.0_f32.to_bits()
        } else {
            f.to_bits()
        }
    })
}

fn leaf_id_at(slot_idx: usize) -> NodeId {
    let raw = u32::try_from(slot_idx + 1).expect("arena slot fits in u32");
    NodeId(NonZeroU32::new(raw).expect("slot index + 1 is non-zero"))
}

#[cfg(test)]
mod shared_vertices_tests {
    use std::sync::Arc;

    use super::*;
    use crate::indexed_mesh::IndexedMesh;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::transform::Transform3D;

    fn triangle_mesh(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new(v0), Vertex::new(v1), Vertex::new(v2)],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        }
    }

    fn add_leaf(graph: &mut Scene, parent: NodeId, mesh: IndexedMesh) -> NodeId {
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

    #[test]
    fn empty_scene_has_no_shared_vertices() {
        let graph = Scene::new();
        assert!(graph.detect_shared_vertices().is_empty());
    }

    #[test]
    fn single_leaf_with_repeated_local_vertex_is_not_shared() {
        // A leaf that contains a duplicate vertex coordinate within
        // its own mesh does not trigger a cross-leaf warning.
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let mut mesh = triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        mesh.vertices.push(Vertex::new([0.0, 0.0, 0.0])); // duplicate coord, same leaf
        add_leaf(&mut graph, root, mesh);
        assert!(graph.detect_shared_vertices().is_empty());
    }

    #[test]
    fn two_disjoint_leaves_are_not_shared() {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        add_leaf(
            &mut graph,
            root,
            triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        );
        add_leaf(
            &mut graph,
            root,
            triangle_mesh([10.0, 0.0, 0.0], [11.0, 0.0, 0.0], [10.0, 1.0, 0.0]),
        );
        assert!(graph.detect_shared_vertices().is_empty());
    }

    #[test]
    fn two_leaves_sharing_one_vertex_are_both_reported() {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let a = add_leaf(
            &mut graph,
            root,
            triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        );
        let b = add_leaf(
            &mut graph,
            root,
            triangle_mesh([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
        );
        let mut expected = vec![a, b];
        expected.sort_by_key(|id| id.get());
        assert_eq!(graph.detect_shared_vertices(), expected);
    }

    #[test]
    fn three_leaves_with_pairwise_sharing_all_reported() {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let a = add_leaf(
            &mut graph,
            root,
            triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        );
        let b = add_leaf(
            &mut graph,
            root,
            triangle_mesh([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
        );
        let c = add_leaf(
            &mut graph,
            root,
            triangle_mesh([5.0, 5.0, 5.0], [1.0, 0.0, 0.0], [9.0, 9.0, 9.0]),
        );
        let mut expected = vec![a, b, c];
        expected.sort_by_key(|id| id.get());
        assert_eq!(graph.detect_shared_vertices(), expected);
    }

    #[test]
    fn negative_zero_collides_with_positive_zero() {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let a = add_leaf(
            &mut graph,
            root,
            triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        );
        let b = add_leaf(
            &mut graph,
            root,
            triangle_mesh([-0.0, -0.0, -0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
        );
        let mut expected = vec![a, b];
        expected.sort_by_key(|id| id.get());
        assert_eq!(graph.detect_shared_vertices(), expected);
    }

    #[test]
    fn mesh_resource_leaves_are_walked_too() {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let rid = graph.add_mesh_resource(triangle_mesh(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ));
        let a = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, a).unwrap();
        let b = add_leaf(
            &mut graph,
            root,
            triangle_mesh([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
        );
        let mut expected = vec![a, b];
        expected.sort_by_key(|id| id.get());
        assert_eq!(graph.detect_shared_vertices(), expected);
    }
}
