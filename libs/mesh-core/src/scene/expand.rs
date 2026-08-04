/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Expansion-aware leaf traversal for [`Scene`].
//!
//! "Expansion" means following `Reference -> subtree resource` edges,
//! so a subtree referenced N times is visited N times, each under the
//! composed transform of its call site. This is the walk the baking
//! and export paths share; walks that deliberately skip references
//! (cache layers, selection indexing) do not belong here.

use std::collections::HashMap;

use crate::indexed_mesh::IndexedMesh;
use crate::scene::Scene;
use crate::scene::error::SceneError;
use crate::scene::node::{LeafPayload, NodeId, NodeKind, SubtreeResourceId};
use crate::transform::Transform3D;

/// Hard ceiling shared by consumers that expand subtree references
/// into leaf occurrences (format readers, scene writers). Expansion
/// graphs are acyclic but can multiply: a subtree holding 2
/// references to its predecessor doubles the occurrence count per
/// level, so a small scene or document can describe 2^n placements.
/// Real plates hold thousands of occurrences at most; the cap bounds
/// the work a crafted or accidental construction can demand.
/// Consumers compare [`Scene::expanded_leaf_count`] against this
/// before walking with [`Scene::for_each_expanded_leaf`].
pub const MAX_EXPANDED_LEAF_COUNT: usize = 100_000;

/// What an expanding walk does when it reaches an unevaluated
/// [`NodeKind::Csg`] node. The stances differ per consumer and both
/// predate this walk: merging/baking refuses (there is no evaluator,
/// so flattening operands would misrepresent the intended solid),
/// while the OBJ writer descends and serialises the operands as
/// plain objects. Callers state their stance explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsgPolicy {
    /// Abort the walk with [`SceneError::UnevaluatedCsg`].
    Refuse,
    /// Treat the CSG node as a plain container and visit its
    /// operand subtrees.
    Descend,
}

impl Scene {
    /// Pre-order traversal of every leaf occurrence reachable from
    /// the scene roots, expanding `Reference` edges.
    ///
    /// `visit` receives the leaf's stable [`NodeId`], the composed
    /// world transform of this occurrence (root applied last,
    /// call-site chains included), and the resolved mesh.
    /// A leaf inside a subtree referenced N times is visited N
    /// times, with the same `NodeId` and N different transforms.
    ///
    /// The number of occurrences can grow exponentially in the node
    /// count when referenced subtrees nest (each level multiplies).
    /// Callers that accept untrusted or unbounded scenes must check
    /// [`Scene::expanded_leaf_count`] against their budget before
    /// walking.
    ///
    /// Errors with the relevant dangling-id [`SceneError`] on a
    /// broken reference, with [`SceneError::UnevaluatedCsg`] under
    /// [`CsgPolicy::Refuse`], and with whatever `visit` returns;
    /// the first error aborts the walk.
    pub fn for_each_expanded_leaf<E, F>(&self, csg: CsgPolicy, mut visit: F) -> Result<(), E>
    where
        E: From<SceneError>,
        F: FnMut(NodeId, &Transform3D, &IndexedMesh) -> Result<(), E>,
    {
        let mut stack: Vec<(NodeId, Transform3D)> = self
            .roots()
            .iter()
            .rev()
            .map(|&root| (root, Transform3D::Identity))
            .collect();
        while let Some((id, parent_world)) = stack.pop() {
            let node = self.node(id).ok_or(SceneError::DanglingNodeId { id })?;
            let world = parent_world.compose(&node.local_transform);
            match &node.kind {
                NodeKind::Group { children } => {
                    for &child in children.iter().rev() {
                        stack.push((child, world));
                    }
                }
                NodeKind::Csg { children, .. } => match csg {
                    CsgPolicy::Refuse => {
                        return Err(SceneError::UnevaluatedCsg { node_id: id }.into());
                    }
                    CsgPolicy::Descend => {
                        for &child in children.iter().rev() {
                            stack.push((child, world));
                        }
                    }
                },
                NodeKind::Leaf(payload) => {
                    let mesh = match payload {
                        LeafPayload::Mesh(m) => m.as_ref(),
                        LeafPayload::MeshResource(rid) => self
                            .mesh_resource(*rid)
                            .ok_or(SceneError::DanglingMeshResource { id: *rid })?,
                    };
                    visit(id, &world, mesh)?;
                }
                NodeKind::Reference { resource } => {
                    let root = self
                        .subtree_resource_root(*resource)
                        .ok_or(SceneError::DanglingSubtreeResource { id: *resource })?;
                    stack.push((root, world));
                }
            }
        }
        Ok(())
    }

    /// The number of leaf occurrences [`Scene::for_each_expanded_leaf`]
    /// would visit, computed WITHOUT walking occurrences: each
    /// reachable subtree resource is tallied once and its total is
    /// reused per reference, so a scene describing 2^n placements is
    /// counted in time proportional to its node count, not its
    /// occurrence count. Totals saturate at [`usize::MAX`].
    ///
    /// This is the budget pre-pass for the expanding walk;
    /// [`MAX_EXPANDED_LEAF_COUNT`] is the shared default budget.
    ///
    /// Leaf payloads are not resolved; the count needs only the
    /// graph structure. `csg` follows the same semantics as the
    /// walk; dangling node ids and dangling subtree resources
    /// error, and scopes not reachable from the scene roots
    /// (unreferenced registered subtrees) are never examined.
    pub fn expanded_leaf_count(&self, csg: CsgPolicy) -> Result<usize, SceneError> {
        let main = self.tally_scope(self.roots().iter().copied(), csg)?;

        // Tally every reachable resource scope exactly once.
        let mut tally_of: HashMap<SubtreeResourceId, ScopeTally> = HashMap::new();
        let mut queue: Vec<SubtreeResourceId> = main.refs.clone();
        while let Some(rid) = queue.pop() {
            if tally_of.contains_key(&rid) {
                continue;
            }
            let root = self
                .subtree_resource_root(rid)
                .ok_or(SceneError::DanglingSubtreeResource { id: rid })?;
            let tally = self.tally_scope(std::iter::once(root), csg)?;
            queue.extend(tally.refs.iter().copied());
            tally_of.insert(rid, tally);
        }

        // Resolve totals bottom-up. The expansion graph is acyclic
        // (enforced at every mutation), so each pass resolves at
        // least one scope whose references are all resolved; a pass
        // without progress would mean that invariant broke, and the
        // loop reports it as the cycle it is instead of spinning.
        let mut totals: HashMap<SubtreeResourceId, usize> = HashMap::new();
        let mut unresolved: Vec<SubtreeResourceId> = tally_of.keys().copied().collect();
        while !unresolved.is_empty() {
            let before = unresolved.len();
            unresolved.retain(|&rid| {
                let tally = &tally_of[&rid];
                if tally.refs.iter().all(|r| totals.contains_key(r)) {
                    let total = tally
                        .refs
                        .iter()
                        .fold(tally.local_leaves, |acc, r| acc.saturating_add(totals[r]));
                    totals.insert(rid, total);
                    false
                } else {
                    true
                }
            });
            if unresolved.len() == before {
                let stuck = unresolved[0];
                let node_id = self
                    .subtree_resource_root(stuck)
                    .ok_or(SceneError::DanglingSubtreeResource { id: stuck })?;
                return Err(SceneError::ExpansionCycle { node_id });
            }
        }

        Ok(main
            .refs
            .iter()
            .fold(main.local_leaves, |acc, r| acc.saturating_add(totals[r])))
    }

    /// Counts the leaves directly inside one expansion scope (the
    /// main scene, or one subtree resource's interior) and records
    /// each subtree reference with multiplicity, without
    /// descending through it.
    fn tally_scope(
        &self,
        starts: impl IntoIterator<Item = NodeId>,
        csg: CsgPolicy,
    ) -> Result<ScopeTally, SceneError> {
        let mut stack: Vec<NodeId> = starts.into_iter().collect();
        let mut local_leaves = 0usize;
        let mut refs: Vec<SubtreeResourceId> = Vec::new();
        while let Some(id) = stack.pop() {
            let node = self.node(id).ok_or(SceneError::DanglingNodeId { id })?;
            match &node.kind {
                NodeKind::Group { children } => stack.extend(children.iter().copied()),
                NodeKind::Csg { children, .. } => match csg {
                    CsgPolicy::Refuse => {
                        return Err(SceneError::UnevaluatedCsg { node_id: id });
                    }
                    CsgPolicy::Descend => stack.extend(children.iter().copied()),
                },
                NodeKind::Leaf(_) => local_leaves += 1,
                NodeKind::Reference { resource } => {
                    if self.subtree_resource_root(*resource).is_none() {
                        return Err(SceneError::DanglingSubtreeResource { id: *resource });
                    }
                    refs.push(*resource);
                }
            }
        }
        Ok(ScopeTally { local_leaves, refs })
    }
}

/// Leaves and subtree references found directly inside one
/// expansion scope; see [`Scene::tally_scope`].
struct ScopeTally {
    local_leaves: usize,
    refs: Vec<SubtreeResourceId>,
}

#[cfg(test)]
mod expand_tests {
    use super::*;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::scene::node::{CsgOp, SubtreeResourceId};
    use crate::transform::AffineTransform3D;
    use float_cmp::assert_approx_eq;
    use std::sync::Arc;

    fn tri_mesh() -> IndexedMesh {
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

    fn translation(x: f64, y: f64, z: f64) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation([x, y, z]))
    }

    /// Registers a subtree holding one triangle leaf under
    /// `leaf_transform` and returns the resource id.
    fn register_triangle_subtree(g: &mut Scene, leaf_transform: Transform3D) -> SubtreeResourceId {
        let leaf = g
            .add_leaf(
                None,
                leaf_transform,
                LeafPayload::Mesh(Arc::new(tri_mesh())),
            )
            .unwrap();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_child(root, leaf).unwrap();
        g.register_subtree_resource(root).unwrap()
    }

    /// Collects `(leaf id, translation of the occurrence)` pairs.
    fn collect_occurrences(g: &Scene, csg: CsgPolicy) -> Vec<(NodeId, [f64; 3])> {
        let mut seen: Vec<(NodeId, [f64; 3])> = Vec::new();
        g.for_each_expanded_leaf::<SceneError, _>(csg, |id, world, _mesh| {
            let t = match world {
                Transform3D::Identity => [0.0, 0.0, 0.0],
                Transform3D::Affine(a) => *a.translation(),
            };
            seen.push((id, t));
            Ok(())
        })
        .unwrap();
        seen
    }

    #[test]
    fn plain_scene_visits_leaves_in_pre_order_with_composed_transforms() {
        let mut g = Scene::new();
        let root = g.add_group(None, translation(100.0, 0.0, 0.0));
        let a = g
            .add_leaf(
                None,
                translation(1.0, 0.0, 0.0),
                LeafPayload::Mesh(Arc::new(tri_mesh())),
            )
            .unwrap();
        let b = g
            .add_leaf(
                None,
                translation(2.0, 0.0, 0.0),
                LeafPayload::Mesh(Arc::new(tri_mesh())),
            )
            .unwrap();
        g.add_child(root, a).unwrap();
        g.add_child(root, b).unwrap();
        g.add_root(root).unwrap();

        let seen = collect_occurrences(&g, CsgPolicy::Refuse);
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, a);
        assert_approx_eq!(f64, seen[0].1[0], 101.0);
        assert_eq!(seen[1].0, b);
        assert_approx_eq!(f64, seen[1].1[0], 102.0);
    }

    #[test]
    fn occurrence_composes_the_call_site_chain() {
        let mut g = Scene::new();
        let rid = register_triangle_subtree(&mut g, translation(1.0, 0.0, 0.0));
        let outer = g.add_group(None, translation(0.0, 50.0, 0.0));
        let reference = g
            .add_reference(None, translation(10.0, 0.0, 0.0), rid)
            .unwrap();
        g.add_child(outer, reference).unwrap();
        g.add_root(outer).unwrap();

        let seen = collect_occurrences(&g, CsgPolicy::Refuse);
        assert_eq!(seen.len(), 1);
        // outer (0,50,0) + reference (10,0,0) + subtree leaf (1,0,0).
        assert_approx_eq!(f64, seen[0].1[0], 11.0);
        assert_approx_eq!(f64, seen[0].1[1], 50.0);
    }

    #[test]
    fn nested_references_multiply_and_compose() {
        let mut g = Scene::new();
        let inner = register_triangle_subtree(&mut g, Transform3D::Identity);
        // Outer subtree: a group holding 2 references to the inner
        // subtree at x = 0 and x = 1.
        let i1 = g.add_reference(None, Transform3D::Identity, inner).unwrap();
        let i2 = g
            .add_reference(None, translation(1.0, 0.0, 0.0), inner)
            .unwrap();
        let outer_root = g.add_group(None, Transform3D::Identity);
        g.add_child(outer_root, i1).unwrap();
        g.add_child(outer_root, i2).unwrap();
        let outer = g.register_subtree_resource(outer_root).unwrap();

        // Scene: 2 references to the outer subtree at x = 0 and x = 10.
        let root = g.add_group(None, Transform3D::Identity);
        let o1 = g.add_reference(None, Transform3D::Identity, outer).unwrap();
        let o2 = g
            .add_reference(None, translation(10.0, 0.0, 0.0), outer)
            .unwrap();
        g.add_child(root, o1).unwrap();
        g.add_child(root, o2).unwrap();
        g.add_root(root).unwrap();

        let seen = collect_occurrences(&g, CsgPolicy::Refuse);
        let xs: Vec<f64> = seen.iter().map(|(_, t)| t[0]).collect();
        assert_eq!(xs.len(), 4, "2 x 2 nested references yield 4 occurrences");
        assert_approx_eq!(f64, xs[0], 0.0);
        assert_approx_eq!(f64, xs[1], 1.0);
        assert_approx_eq!(f64, xs[2], 10.0);
        assert_approx_eq!(f64, xs[3], 11.0);
    }

    #[test]
    fn diamond_sharing_visits_the_same_leaf_id_per_occurrence() {
        let mut g = Scene::new();
        let rid = register_triangle_subtree(&mut g, Transform3D::Identity);
        let root = g.add_group(None, Transform3D::Identity);
        let i1 = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        let i2 = g
            .add_reference(None, translation(5.0, 0.0, 0.0), rid)
            .unwrap();
        g.add_child(root, i1).unwrap();
        g.add_child(root, i2).unwrap();
        g.add_root(root).unwrap();

        let seen = collect_occurrences(&g, CsgPolicy::Refuse);
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0].0, seen[1].0,
            "both occurrences share the leaf's NodeId"
        );
        assert_approx_eq!(f64, seen[0].1[0], 0.0);
        assert_approx_eq!(f64, seen[1].1[0], 5.0);
    }

    #[test]
    fn csg_refuse_aborts_naming_the_csg_node() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh())),
            )
            .unwrap();
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![leaf])
            .unwrap();
        g.add_root(csg).unwrap();

        let result = g.for_each_expanded_leaf::<SceneError, _>(CsgPolicy::Refuse, |_, _, _| Ok(()));
        assert_eq!(result, Err(SceneError::UnevaluatedCsg { node_id: csg }));
    }

    #[test]
    fn csg_descend_visits_the_operands() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                translation(3.0, 0.0, 0.0),
                LeafPayload::Mesh(Arc::new(tri_mesh())),
            )
            .unwrap();
        let csg = g
            .add_csg(None, translation(1.0, 0.0, 0.0), CsgOp::Union, vec![leaf])
            .unwrap();
        g.add_root(csg).unwrap();

        let seen = collect_occurrences(&g, CsgPolicy::Descend);
        assert_eq!(seen.len(), 1);
        assert_approx_eq!(f64, seen[0].1[0], 4.0);
    }

    #[test]
    fn callback_error_aborts_the_walk() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        for _ in 0..3 {
            let leaf = g
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(tri_mesh())),
                )
                .unwrap();
            g.add_child(root, leaf).unwrap();
        }
        g.add_root(root).unwrap();

        let mut visits = 0usize;
        let result = g.for_each_expanded_leaf::<SceneError, _>(CsgPolicy::Refuse, |id, _, _| {
            visits += 1;
            Err(SceneError::DanglingNodeId { id })
        });
        assert!(result.is_err());
        assert_eq!(visits, 1, "the first callback error stops the walk");
    }

    /// Builds a multiplicative chain: level 0 is a single-leaf
    /// subtree, each further level is a subtree holding 2 references
    /// to the previous one, and the scene holds 1 reference to the
    /// top level. Expanded count = 2^levels.
    fn doubling_chain(levels: usize) -> Scene {
        let mut g = Scene::new();
        let mut current = register_triangle_subtree(&mut g, Transform3D::Identity);
        for _ in 0..levels {
            let i1 = g
                .add_reference(None, Transform3D::Identity, current)
                .unwrap();
            let i2 = g
                .add_reference(None, Transform3D::Identity, current)
                .unwrap();
            let root = g.add_group(None, Transform3D::Identity);
            g.add_child(root, i1).unwrap();
            g.add_child(root, i2).unwrap();
            current = g.register_subtree_resource(root).unwrap();
        }
        let top = g
            .add_reference(None, Transform3D::Identity, current)
            .unwrap();
        let scene_root = g.add_group(None, Transform3D::Identity);
        g.add_child(scene_root, top).unwrap();
        g.add_root(scene_root).unwrap();
        g
    }

    fn count(g: &Scene, csg: CsgPolicy) -> Result<usize, SceneError> {
        g.expanded_leaf_count(csg)
    }

    #[test]
    fn count_matches_the_walk_on_every_shape() {
        // Plain, shared, nested, diamond and csg-descend scenes:
        // the count must equal the number of occurrences the walk
        // visits.
        let builders: Vec<fn() -> Scene> = vec![
            || {
                let mut g = Scene::new();
                let root = g.add_group(None, Transform3D::Identity);
                let leaf = g
                    .add_leaf(
                        None,
                        Transform3D::Identity,
                        LeafPayload::Mesh(Arc::new(tri_mesh())),
                    )
                    .unwrap();
                g.add_child(root, leaf).unwrap();
                g.add_root(root).unwrap();
                g
            },
            || {
                let mut g = Scene::new();
                let rid = register_triangle_subtree(&mut g, Transform3D::Identity);
                let root = g.add_group(None, Transform3D::Identity);
                let i1 = g.add_reference(None, Transform3D::Identity, rid).unwrap();
                let i2 = g.add_reference(None, Transform3D::Identity, rid).unwrap();
                g.add_child(root, i1).unwrap();
                g.add_child(root, i2).unwrap();
                g.add_root(root).unwrap();
                g
            },
            || doubling_chain(3),
        ];
        for (i, build) in builders.iter().enumerate() {
            let g = build();
            let walked = collect_occurrences(&g, CsgPolicy::Refuse).len();
            assert_eq!(
                count(&g, CsgPolicy::Refuse).unwrap(),
                walked,
                "shape {i}: count diverged from the walk"
            );
        }
    }

    #[test]
    fn count_handles_exponential_scenes_without_walking_them() {
        // 2^20 occurrences from ~65 nodes; the count must be exact
        // and fast, and it exceeds the shared budget.
        let g = doubling_chain(20);
        let n = count(&g, CsgPolicy::Refuse).unwrap();
        assert_eq!(n, 1 << 20);
        assert!(n > MAX_EXPANDED_LEAF_COUNT);
    }

    #[test]
    fn count_saturates_instead_of_overflowing() {
        // 2^70 exceeds usize on 64-bit targets; the fold must
        // saturate, not wrap.
        let g = doubling_chain(70);
        assert_eq!(count(&g, CsgPolicy::Refuse).unwrap(), usize::MAX);
    }

    #[test]
    fn count_csg_semantics_match_the_walk() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh())),
            )
            .unwrap();
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![leaf])
            .unwrap();
        g.add_root(csg).unwrap();

        assert_eq!(
            count(&g, CsgPolicy::Refuse),
            Err(SceneError::UnevaluatedCsg { node_id: csg })
        );
        assert_eq!(count(&g, CsgPolicy::Descend).unwrap(), 1);
    }

    #[test]
    fn count_ignores_unreachable_registered_subtrees() {
        // An unreferenced subtree containing a CSG node must not
        // affect the count, even under Refuse: it is outside the
        // scene's expansion.
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh())),
            )
            .unwrap();
        g.add_child(root, leaf).unwrap();
        g.add_root(root).unwrap();

        let orphan_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh())),
            )
            .unwrap();
        let orphan_csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![orphan_leaf])
            .unwrap();
        let orphan_root = g.add_group(None, Transform3D::Identity);
        g.add_child(orphan_root, orphan_csg).unwrap();
        g.register_subtree_resource(orphan_root).unwrap();

        assert_eq!(count(&g, CsgPolicy::Refuse).unwrap(), 1);
    }

    #[test]
    fn count_matches_the_walk_for_resource_backed_leaves() {
        let mut g = Scene::new();
        let mid = g.add_mesh_resource(tri_mesh());
        let root = g.add_group(None, Transform3D::Identity);
        for _ in 0..2 {
            let leaf = g
                .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(mid))
                .unwrap();
            g.add_child(root, leaf).unwrap();
        }
        g.add_root(root).unwrap();

        assert_eq!(count(&g, CsgPolicy::Refuse).unwrap(), 2);
        assert_eq!(collect_occurrences(&g, CsgPolicy::Refuse).len(), 2);
    }

    #[test]
    fn resource_backed_leaves_resolve_through_the_arena() {
        let mut g = Scene::new();
        let mid = g.add_mesh_resource(tri_mesh());
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(mid))
            .unwrap();
        g.add_root(leaf).unwrap();

        let mut faces = 0usize;
        g.for_each_expanded_leaf::<SceneError, _>(CsgPolicy::Refuse, |_, _, mesh| {
            faces += mesh.faces.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(faces, 1);
    }
}
