/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Expansion-aware leaf traversal for [`Scene`].
//!
//! "Expansion" means following `Instance -> subtree resource` edges,
//! so a subtree instanced N times is visited N times, each under the
//! composed transform of its call site. This is the walk the baking
//! and export paths share; walks that deliberately skip instances
//! (cache layers, selection indexing) do not belong here.

use crate::indexed_mesh::IndexedMesh;
use crate::scene::Scene;
use crate::scene::error::SceneError;
use crate::scene::node::{LeafPayload, NodeId, NodeKind};
use crate::transform::Transform3D;

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
    /// the scene roots, expanding `Instance` edges.
    ///
    /// `visit` receives the leaf's stable [`NodeId`], the composed
    /// world transform of this occurrence (root applied last,
    /// instance call-site chains included), and the resolved mesh.
    /// A leaf inside a subtree instanced N times is visited N
    /// times, with the same `NodeId` and N different transforms.
    ///
    /// The number of occurrences can grow exponentially in the node
    /// count when instanced subtrees nest (each level multiplies).
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
                NodeKind::Instance { resource } => {
                    let root = self
                        .subtree_resource_root(*resource)
                        .ok_or(SceneError::DanglingSubtreeResource { id: *resource })?;
                    stack.push((root, world));
                }
            }
        }
        Ok(())
    }
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
    fn instance_occurrence_composes_the_call_site_chain() {
        let mut g = Scene::new();
        let rid = register_triangle_subtree(&mut g, translation(1.0, 0.0, 0.0));
        let outer = g.add_group(None, translation(0.0, 50.0, 0.0));
        let inst = g
            .add_instance(None, translation(10.0, 0.0, 0.0), rid)
            .unwrap();
        g.add_child(outer, inst).unwrap();
        g.add_root(outer).unwrap();

        let seen = collect_occurrences(&g, CsgPolicy::Refuse);
        assert_eq!(seen.len(), 1);
        // outer (0,50,0) + instance (10,0,0) + subtree leaf (1,0,0).
        assert_approx_eq!(f64, seen[0].1[0], 11.0);
        assert_approx_eq!(f64, seen[0].1[1], 50.0);
    }

    #[test]
    fn nested_instances_multiply_and_compose() {
        let mut g = Scene::new();
        let inner = register_triangle_subtree(&mut g, Transform3D::Identity);
        // Outer subtree: a group holding 2 instances of the inner
        // subtree at x = 0 and x = 1.
        let i1 = g.add_instance(None, Transform3D::Identity, inner).unwrap();
        let i2 = g
            .add_instance(None, translation(1.0, 0.0, 0.0), inner)
            .unwrap();
        let outer_root = g.add_group(None, Transform3D::Identity);
        g.add_child(outer_root, i1).unwrap();
        g.add_child(outer_root, i2).unwrap();
        let outer = g.register_subtree_resource(outer_root).unwrap();

        // Scene: 2 instances of the outer subtree at x = 0 and x = 10.
        let root = g.add_group(None, Transform3D::Identity);
        let o1 = g.add_instance(None, Transform3D::Identity, outer).unwrap();
        let o2 = g
            .add_instance(None, translation(10.0, 0.0, 0.0), outer)
            .unwrap();
        g.add_child(root, o1).unwrap();
        g.add_child(root, o2).unwrap();
        g.add_root(root).unwrap();

        let seen = collect_occurrences(&g, CsgPolicy::Refuse);
        let xs: Vec<f64> = seen.iter().map(|(_, t)| t[0]).collect();
        assert_eq!(xs.len(), 4, "2 x 2 nested instances yield 4 occurrences");
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
        let i1 = g.add_instance(None, Transform3D::Identity, rid).unwrap();
        let i2 = g
            .add_instance(None, translation(5.0, 0.0, 0.0), rid)
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
