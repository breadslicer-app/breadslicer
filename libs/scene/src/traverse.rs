/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Structural traversal and world-transform queries on [`Scene`].
//!
//! Structural means the walks follow children edges only: `Reference`
//! nodes are never expanded here. Occurrence-level walks (one visit
//! per expanded occurrence, call-site transforms included) live in
//! [`crate::expand`].

use sc_mesh_core::transform::Transform3D;

use crate::Scene;
use crate::error::SceneError;
use crate::expand::CsgPolicy;
use crate::node::{Node, NodeId, NodeKind};

/// Stance of a structural walk that meets a `Reference` node.
///
/// Mirrors [`CsgPolicy`]: call sites state what a reference means to
/// them instead of inheriting a walk's hard-coded choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferencePolicy {
    /// Pass over the reference without following it. The referenced
    /// subtree's leaves are not visited.
    Skip,
    /// Abort the walk with [`SceneError::UnexpandedReference`].
    Refuse,
}

impl Scene {
    /// Visit every node reachable from the scene roots through
    /// children edges, in pre-order. `Csg` operands are entered;
    /// `Reference` nodes are visited themselves but never followed.
    /// Ids whose slot is tombstoned are passed over silently, so the
    /// walk is safe on scenes mid-undo.
    pub fn for_each_reachable_node<F>(&self, mut visit: F)
    where
        F: FnMut(NodeId, &Node),
    {
        let mut stack: Vec<NodeId> = self.roots().iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = self.node(id) else {
                continue;
            };
            visit(id, node);
            match &node.kind {
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                    for child in children.iter().rev() {
                        stack.push(*child);
                    }
                }
                NodeKind::Leaf(_) | NodeKind::Reference { .. } => {}
            }
        }
    }

    /// Visit every leaf reachable from the scene roots through
    /// children edges, in pre-order, under the stated `Csg` and
    /// `Reference` stances. Ids whose slot is tombstoned are passed
    /// over silently, so the walk is safe on scenes mid-undo.
    ///
    /// The first refusal or closure error aborts the walk.
    pub fn for_each_reachable_leaf<E, F>(
        &self,
        csg: CsgPolicy,
        refs: ReferencePolicy,
        mut visit: F,
    ) -> Result<(), E>
    where
        E: From<SceneError>,
        F: FnMut(NodeId) -> Result<(), E>,
    {
        let mut stack: Vec<NodeId> = self.roots().iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = self.node(id) else {
                continue;
            };
            match &node.kind {
                NodeKind::Leaf(_) => visit(id)?,
                NodeKind::Group { children } => {
                    for child in children.iter().rev() {
                        stack.push(*child);
                    }
                }
                NodeKind::Csg { children, .. } => match csg {
                    CsgPolicy::Descend => {
                        for child in children.iter().rev() {
                            stack.push(*child);
                        }
                    }
                    CsgPolicy::Refuse => {
                        return Err(SceneError::UnevaluatedCsg { node_id: id }.into());
                    }
                },
                NodeKind::Reference { .. } => match refs {
                    ReferencePolicy::Skip => {}
                    ReferencePolicy::Refuse => {
                        return Err(SceneError::UnexpandedReference { node_id: id }.into());
                    }
                },
            }
        }
        Ok(())
    }

    /// The reachable leaves in pre-order, as a list. Convenience over
    /// [`Self::for_each_reachable_leaf`] with the same stances and
    /// refusal errors.
    pub fn reachable_leaves(
        &self,
        csg: CsgPolicy,
        refs: ReferencePolicy,
    ) -> Result<Vec<NodeId>, SceneError> {
        let mut out = Vec::new();
        self.for_each_reachable_leaf::<SceneError, _>(csg, refs, |id| {
            out.push(id);
            Ok(())
        })?;
        Ok(out)
    }

    /// World transform of the node at `id`: its local transform
    /// composed under the ancestor chain, root applied last. The
    /// chain ends at the nearest parentless ancestor, so for a node
    /// inside a registered subtree the result is relative to that
    /// subtree's root frame.
    ///
    /// This is a structural query. A leaf referenced N times has N
    /// occurrence transforms, which only
    /// [`Scene::for_each_expanded_leaf`] can hand out.
    ///
    /// Errors with [`SceneError::DanglingNodeId`] when `id` or any
    /// ancestor link does not resolve to a live node.
    pub fn world_transform_of(&self, id: NodeId) -> Result<Transform3D, SceneError> {
        let node = self.node(id).ok_or(SceneError::DanglingNodeId { id })?;
        let mut chain: Vec<Transform3D> = vec![node.local_transform];
        let mut current = id;
        while let Some(p) = self.parent_of(current) {
            let pnode = self.node(p).ok_or(SceneError::DanglingNodeId { id: p })?;
            chain.push(pnode.local_transform);
            current = p;
        }
        let mut world = Transform3D::Identity;
        for t in chain.iter().rev() {
            world = world.compose(t);
        }
        Ok(world)
    }
}
