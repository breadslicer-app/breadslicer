/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Shared-vertex detection for [`Scene`].

use std::num::NonZeroU32;

use rayon::slice::ParallelSliceMut;

use crate::Scene;
use crate::node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_core::primitives::{CanonicalCoords, Vertex};

impl Scene {
    /// Returns the ids of the leaves that have a vertex at the same position as
    /// a vertex of another leaf. The ids are in ascending order, with no
    /// duplicates. A vertex with a NaN or infinite coordinate matches no other;
    /// [`Scene::diagnose_meshes`] reports such a vertex.
    ///
    /// Runs in O(n log n) time and O(n) space, for n vertices in all the
    /// leaves.
    ///
    /// TODO: the detection ignores the transforms of the leaves. It reports
    /// leaves that are placed apart but store equal coordinates, such as 2 uses
    /// of 1 mesh resource, and misses leaves whose vertices meet in the scene.
    pub fn detect_shared_vertices(&self) -> Vec<NodeId> {
        let leaves = self.leaves_with_vertices();
        if leaves.len() < 2 {
            return Vec::new();
        }

        // 1 record per finite vertex. The tuple order sorts by coordinate first
        // and by leaf second, so every equal-coordinate run is itself grouped
        // by leaf.
        let total: usize = leaves.iter().map(|(_, vertices)| vertices.len()).sum();
        let mut records: Vec<(CanonicalCoords, NodeId)> = Vec::with_capacity(total);
        for (leaf_id, vertices) in &leaves {
            records.extend(
                vertices.iter().filter_map(|vertex| {
                    vertex.canonical_coords().map(|coords| (coords, *leaf_id))
                }),
            );
        }
        records.par_sort_unstable();

        let mut shared: Vec<NodeId> = Vec::new();
        for run in records.chunk_by(|a, b| a.0 == b.0) {
            // Sorted by leaf within the run, so the same leaf at both ends
            // means 1 leaf throughout: a coordinate repeated inside a single
            // mesh, which is not sharing.
            if run[0].1 == run[run.len() - 1].1 {
                continue;
            }
            let mut previous = None;
            for &(_, leaf_id) in run {
                if previous != Some(leaf_id) {
                    shared.push(leaf_id);
                    previous = Some(leaf_id);
                }
            }
        }
        shared.sort_unstable();
        shared.dedup();
        shared
    }

    /// Returns every leaf that carries at least 1 vertex, with its mesh. Empty
    /// leaves are dropped: they cannot share a coordinate, and without them the
    /// 2-leaf early exit of [`Scene::detect_shared_vertices`] applies to more
    /// scenes.
    fn leaves_with_vertices(&self) -> Vec<(NodeId, &[Vertex])> {
        let mut leaves = Vec::new();
        for (slot_idx, slot) in self.nodes.iter().enumerate() {
            let Some(node) = slot else { continue };
            let NodeKind::Leaf(payload) = &node.kind else {
                continue;
            };
            let vertices: &[Vertex] = match payload {
                LeafPayload::Mesh(arc) => arc.vertices(),
                LeafPayload::MeshResource(rid) => match self.mesh_resource(*rid) {
                    Some(mesh) => mesh.vertices(),
                    None => continue,
                },
            };
            if !vertices.is_empty() {
                leaves.push((leaf_id_at(slot_idx), vertices));
            }
        }
        leaves
    }
}

fn leaf_id_at(slot_idx: usize) -> NodeId {
    let raw = u32::try_from(slot_idx + 1).expect("arena slot fits in u32");
    NodeId(NonZeroU32::new(raw).expect("slot index + 1 is non-zero"))
}

#[cfg(test)]
mod shared_vertices_tests {
    use std::sync::Arc;

    use proptest::prelude::*;

    use super::*;
    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;

    fn triangle_mesh(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new(v0), Vertex::new(v1), Vertex::new(v2)],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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
        // A leaf that contains a duplicate vertex coordinate within its own
        // mesh does not trigger a cross-leaf warning.
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

    #[test]
    fn an_empty_leaf_beside_a_populated_one_reports_nothing() {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        add_leaf(
            &mut graph,
            root,
            triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        );
        let empty = IndexedMesh {
            meta: None,
            vertices: Vec::new(),
            faces: Vec::new(),
        };
        add_leaf(&mut graph, root, empty);
        assert!(graph.detect_shared_vertices().is_empty());
    }

    #[test]
    fn a_coordinate_held_by_three_leaves_reports_each_once() {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let corner = [4.0, 4.0, 4.0];
        let ids: Vec<NodeId> = (0..3)
            .map(|i| {
                let offset = i as f32;
                add_leaf(
                    &mut graph,
                    root,
                    triangle_mesh(corner, [offset, 0.0, 0.0], [0.0, offset, 0.0]),
                )
            })
            .collect();
        let mut expected = ids;
        expected.sort_by_key(|id| id.get());
        assert_eq!(graph.detect_shared_vertices(), expected);
    }

    #[test]
    fn leaves_sharing_many_coordinates_are_reported_once_each() {
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
            triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        );
        let mut expected = vec![a, b];
        expected.sort_by_key(|id| id.get());
        assert_eq!(graph.detect_shared_vertices(), expected);
    }

    #[test]
    fn leaves_that_share_only_a_non_finite_vertex_are_not_reported() {
        for bad in [f32::NAN, f32::INFINITY] {
            let mut graph = Scene::new();
            let root = graph.add_group(None, Transform3D::Identity);
            graph.add_root(root).unwrap();
            add_leaf(
                &mut graph,
                root,
                triangle_mesh([bad, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            );
            add_leaf(
                &mut graph,
                root,
                triangle_mesh([bad, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
            );
            assert!(graph.detect_shared_vertices().is_empty(), "{bad}");
        }
    }

    #[test]
    fn a_non_finite_vertex_does_not_hide_a_shared_one() {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let a = add_leaf(
            &mut graph,
            root,
            triangle_mesh([f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        );
        let b = add_leaf(
            &mut graph,
            root,
            triangle_mesh([5.0, 5.0, 5.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0]),
        );
        let mut expected = vec![a, b];
        expected.sort_by_key(|id| id.get());
        assert_eq!(graph.detect_shared_vertices(), expected);
    }

    /// Detects shared vertices with a first-seen map keyed on the coordinate:
    /// the earlier detection, kept as the reference that the sort must agree
    /// with.
    fn reference_detect(graph: &Scene) -> Vec<NodeId> {
        use std::collections::HashMap;
        use std::collections::hash_map::Entry;

        let mut first_seen: HashMap<CanonicalCoords, NodeId> = HashMap::new();
        let mut shared: HashMap<NodeId, ()> = HashMap::new();
        for (slot_idx, slot) in graph.nodes.iter().enumerate() {
            let Some(node) = slot else { continue };
            let NodeKind::Leaf(payload) = &node.kind else {
                continue;
            };
            let vertices: &[Vertex] = match payload {
                LeafPayload::Mesh(arc) => arc.vertices(),
                LeafPayload::MeshResource(rid) => match graph.mesh_resource(*rid) {
                    Some(mesh) => mesh.vertices(),
                    None => continue,
                },
            };
            let leaf_id = leaf_id_at(slot_idx);
            for v in vertices {
                let Some(coords) = v.canonical_coords() else {
                    continue;
                };
                match first_seen.entry(coords) {
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

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        /// Pins that the sort agrees with the reference detection. Coordinates
        /// come from a pool small enough that collisions across leaves are the
        /// common case.
        #[test]
        fn matches_the_reference_detection(
            meshes in prop::collection::vec(
                prop::collection::vec(
                    prop::collection::vec(-1_i8..2, 3..4),
                    3..9,
                ),
                1..5,
            ),
        ) {
            let mut graph = Scene::new();
            let root = graph.add_group(None, Transform3D::Identity);
            graph.add_root(root).unwrap();
            for coordinates in &meshes {
                let vertices: Vec<Vertex> = coordinates
                    .iter()
                    .map(|axes| Vertex::new([axes[0] as f32, axes[1] as f32, axes[2] as f32]))
                    .collect();
                let faces = vec![IndexedTriangle::from_raw_indices([0, 1, 2])];
                add_leaf(
                    &mut graph,
                    root,
                    IndexedMesh {
                        meta: None,
                        vertices,
                        faces,
                    },
                );
            }
            prop_assert_eq!(graph.detect_shared_vertices(), reference_detect(&graph));
        }
    }
}
