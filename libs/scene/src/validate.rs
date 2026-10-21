/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Structural invariant re-check for [`Scene`] plus the expansion-graph cycle
//! detector it relies on.

use std::collections::HashSet;
use std::num::NonZeroU32;

use crate::Scene;
use crate::error::SceneError;
use crate::node::{CsgOp, LeafPayload, NodeId, NodeKind};

impl Scene {
    pub(crate) fn expansion_successor(&self, node_idx: usize, child_i: usize) -> Option<NodeId> {
        let node = self.nodes[node_idx].as_ref()?;
        match &node.kind {
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                children.get(child_i).copied()
            }
            NodeKind::Reference { resource } => {
                if child_i == 0 {
                    self.subtree_resource_root(*resource)
                } else {
                    None
                }
            }
            NodeKind::Leaf(_) => None,
        }
    }

    /// Returns whether `goal` is reachable from `from` in the expansion graph
    /// (children edges plus `Reference -> resource root` edges). A node reaches
    /// itself.
    ///
    /// Visits only the subgraph reachable from `from`, so the cost is bounded
    /// by that subgraph and not by the size of the arena.
    pub(crate) fn expansion_reaches(&self, from: NodeId, goal: NodeId) -> bool {
        let goal_idx = goal.index();
        let mut seen: HashSet<usize> = HashSet::new();
        let mut stack = vec![from.index()];
        while let Some(curr) = stack.pop() {
            if curr == goal_idx {
                return true;
            }
            if !seen.insert(curr) {
                continue;
            }
            let mut child_i = 0;
            while let Some(next) = self.expansion_successor(curr, child_i) {
                stack.push(next.index());
                child_i += 1;
            }
        }
        false
    }

    pub(crate) fn find_cycle_in_expansion(&self) -> Option<NodeId> {
        // 3-color iterative DFS over the expansion graph (children edges plus
        // `Reference -> resource root` edges). A back edge to a gray vertex
        // proves a cycle; the result is the id of the vertex that the back edge
        // points to.
        const WHITE: u8 = 0;
        const GRAY: u8 = 1;
        const BLACK: u8 = 2;

        let n = self.nodes.len();
        let mut color = vec![WHITE; n];
        let mut stack: Vec<(usize, usize)> = Vec::new();

        for start in 0..n {
            if self.nodes[start].is_none() || color[start] != WHITE {
                continue;
            }
            color[start] = GRAY;
            stack.push((start, 0));
            while let Some(&(curr, child_i)) = stack.last() {
                if let Some(next_id) = self.expansion_successor(curr, child_i) {
                    let next_idx = next_id.index();
                    let last = stack.last_mut().expect("stack non-empty");
                    last.1 += 1;
                    match color.get(next_idx).copied() {
                        Some(GRAY) => return Some(next_id),
                        Some(WHITE) => {
                            color[next_idx] = GRAY;
                            stack.push((next_idx, 0));
                        }
                        _ => {}
                    }
                } else {
                    color[curr] = BLACK;
                    stack.pop();
                }
            }
        }
        None
    }

    /// Re-checks every structural invariant of this graph: live ids in every
    /// reference, non-empty intersections, parent and child agreement,
    /// expansion-graph acyclicity.
    pub fn validate(&self) -> Result<(), SceneError> {
        // Every referenced node id, mesh resource and subtree resource exists,
        // and every intersection has at least 1 child.
        for (slot_idx, slot) in self.nodes.iter().enumerate() {
            let Some(node) = slot else {
                continue;
            };
            let id = self.node_id_at(slot_idx);
            match &node.kind {
                NodeKind::Group { children } => {
                    for &cid in children {
                        if self.node(cid).is_none() {
                            return Err(SceneError::DanglingNodeId { id: cid });
                        }
                    }
                }
                NodeKind::Csg { op, children } => {
                    if *op == CsgOp::Intersection && children.is_empty() {
                        return Err(SceneError::EmptyIntersection { node_id: id });
                    }
                    for &cid in children {
                        if self.node(cid).is_none() {
                            return Err(SceneError::DanglingNodeId { id: cid });
                        }
                    }
                }
                NodeKind::Leaf(payload) => match payload {
                    LeafPayload::Mesh(_) => {}
                    LeafPayload::MeshResource(rid) => {
                        if self.mesh_resource(*rid).is_none() {
                            return Err(SceneError::DanglingMeshResource { id: *rid });
                        }
                    }
                },
                NodeKind::Reference { resource } => {
                    if self.subtree_resource_root(*resource).is_none() {
                        return Err(SceneError::DanglingSubtreeResource { id: *resource });
                    }
                }
            }
        }

        // Subtree-resource roots must point to live nodes.
        for slot in &self.subtree_resources {
            if let Some(root) = slot
                && self.node(*root).is_none()
            {
                return Err(SceneError::DanglingNodeId { id: *root });
            }
        }

        // Scene roots must point to live nodes and must not also be
        // subtree-resource roots.
        for &root in &self.roots {
            if self.node(root).is_none() {
                return Err(SceneError::DanglingNodeId { id: root });
            }
            if self.is_subtree_resource_root(root) {
                return Err(SceneError::AlreadyASubtreeResourceRoot { node_id: root });
            }
        }

        // Forest: rebuild the expected parents from the children lists and
        // compare them to `self.parents`. This also catches a duplicate child
        // within a single parent.
        let mut expected: Vec<Option<NodeId>> = vec![None; self.nodes.len()];
        for (slot_idx, slot) in self.nodes.iter().enumerate() {
            let Some(node) = slot else {
                continue;
            };
            let pid = self.node_id_at(slot_idx);
            let mut seen: HashSet<NodeId> = HashSet::new();
            for &cid in node.kind.children() {
                if !seen.insert(cid) {
                    return Err(SceneError::DuplicateChild {
                        parent: pid,
                        child: cid,
                    });
                }
                if expected[cid.index()].is_some() {
                    return Err(SceneError::ParentMismatch {
                        node_id: cid,
                        recorded: self.parents.get(cid.index()).copied().flatten(),
                    });
                }
                expected[cid.index()] = Some(pid);
            }
        }
        for (idx, exp) in expected.iter().enumerate() {
            if self.nodes[idx].is_none() {
                continue;
            }
            let actual = self.parents[idx];
            if actual != *exp {
                return Err(SceneError::ParentMismatch {
                    node_id: self.node_id_at(idx),
                    recorded: actual,
                });
            }
        }

        // Expansion-graph acyclicity.
        if let Some(witness) = self.find_cycle_in_expansion() {
            // A cycle found here involves either a Reference edge or forged or
            // floating children edges (a detached 2-cycle with consistent
            // parent records passes the checks above). The guard for mutators
            // against both is in `add_child`.
            return Err(SceneError::ExpansionCycle { node_id: witness });
        }

        Ok(())
    }

    fn node_id_at(&self, slot_idx: usize) -> NodeId {
        let raw = u32::try_from(slot_idx + 1).expect("arena index fits in u32");
        NodeId(NonZeroU32::new(raw).expect("index +1 is non-zero"))
    }
}

#[cfg(test)]
mod scene_validate_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;

    fn unit_triangle() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        }
    }

    #[test]
    fn validate_passes_on_empty_graph() {
        let g = Scene::new();
        assert_eq!(g.validate(), Ok(()));
    }

    #[test]
    fn validate_passes_on_simple_tree() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_child(root, leaf).unwrap();
        g.add_root(root).unwrap();
        assert_eq!(g.validate(), Ok(()));
    }

    #[test]
    fn validate_passes_on_subtree_reference_graph() {
        let mut g = Scene::new();
        let res_root = g.add_group(None, Transform3D::Identity);
        let res_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_child(res_root, res_leaf).unwrap();
        let rid = g.register_subtree_resource(res_root).unwrap();
        let r1 = g.add_group(None, Transform3D::Identity);
        let r2 = g.add_group(None, Transform3D::Identity);
        let i1 = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        let i2 = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        g.add_child(r1, i1).unwrap();
        g.add_child(r2, i2).unwrap();
        g.add_root(r1).unwrap();
        g.add_root(r2).unwrap();
        assert_eq!(g.validate(), Ok(()));
    }

    #[test]
    fn validate_detects_dangling_child() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        let child = g.add_group(None, Transform3D::Identity);
        g.add_child(parent, child).unwrap();
        // Forge: clear the child slot.
        let cidx = child.index();
        g.nodes[cidx] = None;
        g.parents[cidx] = None;
        assert_eq!(g.validate(), Err(SceneError::DanglingNodeId { id: child }));
    }

    #[test]
    fn validate_detects_empty_intersection() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Intersection, vec![leaf])
            .unwrap();
        // Forge: empty the intersection's children list and clear parent.
        if let Some(node) = g.nodes[csg.index()].as_mut()
            && let NodeKind::Csg { children, .. } = &mut node.kind
        {
            children.clear();
        }
        g.parents[leaf.index()] = None;
        assert_eq!(
            g.validate(),
            Err(SceneError::EmptyIntersection { node_id: csg })
        );
    }

    #[test]
    fn validate_detects_parent_mismatch() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        let child = g.add_group(None, Transform3D::Identity);
        g.add_child(parent, child).unwrap();
        // Forge: wipe parents[child], leaving the child in parent's children
        // list but with no recorded parent.
        g.parents[child.index()] = None;
        assert!(matches!(
            g.validate(),
            Err(SceneError::ParentMismatch { node_id: cid, .. }) if cid == child
        ));
    }

    #[test]
    fn validate_detects_dangling_mesh_resource() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        g.add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        // Forge: clear the resource slot.
        g.mesh_resources[rid.index()] = None;
        assert_eq!(
            g.validate(),
            Err(SceneError::DanglingMeshResource { id: rid })
        );
    }

    #[test]
    fn validate_detects_dangling_subtree_resource() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        let rid = g.register_subtree_resource(root).unwrap();
        g.add_reference(None, Transform3D::Identity, rid).unwrap();
        // Forge: clear the resource slot.
        g.subtree_resources[rid.index()] = None;
        assert_eq!(
            g.validate(),
            Err(SceneError::DanglingSubtreeResource { id: rid })
        );
    }

    #[test]
    fn validate_detects_duplicate_child() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        let child = g.add_group(None, Transform3D::Identity);
        g.add_child(parent, child).unwrap();
        // Forge: append child a second time to parent's children list.
        if let Some(node) = g.nodes[parent.index()].as_mut()
            && let NodeKind::Group { children } = &mut node.kind
        {
            children.push(child);
        }
        assert!(matches!(
            g.validate(),
            Err(SceneError::DuplicateChild { .. })
        ));
    }

    #[test]
    fn validate_detects_reference_cycle() {
        let mut g = Scene::new();
        let res_root = g.add_group(None, Transform3D::Identity);
        let rid = g.register_subtree_resource(res_root).unwrap();
        let inst = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        // Forge: insert the reference as a child of res_root, bypassing the
        // cycle check that `add_child` performs.
        if let Some(node) = g.nodes[res_root.index()].as_mut()
            && let NodeKind::Group { children } = &mut node.kind
        {
            children.push(inst);
        }
        g.parents[inst.index()] = Some(res_root);
        assert!(matches!(
            g.validate(),
            Err(SceneError::ExpansionCycle { .. })
        ));
    }
}
