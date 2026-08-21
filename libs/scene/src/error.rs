/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! [`SceneError`]: failure variants surfaced by [`Scene`](Scene)
//! mutators, validators and queries, plus its `Display` and `Error`
//! conformances.

use std::fmt;

use crate::node::{MeshResourceId, NodeId, SubtreeResourceId};
use sc_mesh_core::transform::TransformError;

/// Failure modes of [`Scene`](Scene) mutators,
/// validators and queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneError {
    /// A `NodeId` references a slot that is empty or out of range.
    DanglingNodeId { id: NodeId },
    /// A `MeshResourceId` references a slot that is empty or out of range.
    DanglingMeshResource { id: MeshResourceId },
    /// A `SubtreeResourceId` references a slot that is empty or out of range.
    DanglingSubtreeResource { id: SubtreeResourceId },
    /// A `NodeKind::Csg { op: Intersection, children }` has zero children.
    EmptyIntersection { node_id: NodeId },
    /// The expansion graph (children edges plus `Reference -> resource root`)
    /// contains a cycle reachable from this node. Covers children-only
    /// cycles (floating subtrees, self-loops) and cycles closed through
    /// a Reference edge alike.
    ExpansionCycle { node_id: NodeId },
    /// A child is listed under two different parents, or recorded with a
    /// parent that does not list it as a child.
    ParentMismatch {
        node_id: NodeId,
        recorded: Option<NodeId>,
    },
    /// A mutator was asked to take ownership of a child node that
    /// already has a parent.
    AlreadyHasParent { node_id: NodeId, parent: NodeId },
    /// A mutator was asked to add a node as a scene root, but it is
    /// already a scene root.
    AlreadyARoot { node_id: NodeId },
    /// A mutator was asked to register a node as a subtree-resource root,
    /// but it is already registered (under the same or a different id).
    AlreadyASubtreeResourceRoot { node_id: NodeId },
    /// A child appears more than once in a single parent's children list.
    DuplicateChild { parent: NodeId, child: NodeId },
    /// A mutator that requires a Group or Csg parent was given a Leaf or
    /// Reference node.
    NotABranch { node_id: NodeId },
    /// A query that expects a leaf was given a non-leaf node.
    NotALeaf { node_id: NodeId },
    /// A mutator that expects to fill an empty arena slot found it
    /// already populated.
    NodeSlotOccupied { id: NodeId },
    /// A mutator that expects a scene root was given a non-root node.
    NotARoot { node_id: NodeId },
    /// A mutator that expects a `Leaf(MeshResource(_))` was given a
    /// leaf carrying an inline mesh instead.
    NotAMeshResourceLeaf { node_id: NodeId },
    /// A mutator that expects a `Leaf(Mesh(_))` was given a leaf
    /// carrying its mesh by resource id instead.
    NotAnInlineMeshLeaf { node_id: NodeId },
    /// A mutator that expects to fill an empty mesh-resource slot
    /// found it already populated.
    MeshResourceSlotOccupied { id: MeshResourceId },
    /// A mutator that was asked to remove a mesh resource that at
    /// least one live leaf still references.
    MeshResourceInUse { id: MeshResourceId },
    /// `merge_meshes` was asked to bake a graph that still contains an
    /// unevaluated CSG node.
    UnevaluatedCsg { node_id: NodeId },
    /// A transform operation in the bake path failed.
    Transform(TransformError),
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DanglingNodeId { id } => write!(f, "dangling node id {}", id.get()),
            Self::DanglingMeshResource { id } => {
                write!(f, "dangling mesh resource id {}", id.get())
            }
            Self::DanglingSubtreeResource { id } => {
                write!(f, "dangling subtree resource id {}", id.get())
            }
            Self::EmptyIntersection { node_id } => {
                write!(f, "empty intersection at node {}", node_id.get())
            }
            Self::ExpansionCycle { node_id } => {
                write!(
                    f,
                    "cycle in the expansion graph reachable from node {}",
                    node_id.get()
                )
            }
            Self::ParentMismatch { node_id, recorded } => {
                let r = recorded.map(NodeId::get);
                write!(
                    f,
                    "parent mismatch at node {}: recorded {:?}",
                    node_id.get(),
                    r
                )
            }
            Self::AlreadyHasParent { node_id, parent } => {
                write!(
                    f,
                    "node {} already has parent {}",
                    node_id.get(),
                    parent.get()
                )
            }
            Self::AlreadyARoot { node_id } => {
                write!(f, "node {} is already a scene root", node_id.get())
            }
            Self::AlreadyASubtreeResourceRoot { node_id } => write!(
                f,
                "node {} is already a subtree resource root",
                node_id.get()
            ),
            Self::DuplicateChild { parent, child } => write!(
                f,
                "duplicate child {} under parent {}",
                child.get(),
                parent.get()
            ),
            Self::NotABranch { node_id } => {
                write!(f, "node {} is not a Group or Csg branch", node_id.get())
            }
            Self::NotALeaf { node_id } => write!(f, "node {} is not a Leaf", node_id.get()),
            Self::NodeSlotOccupied { id } => {
                write!(f, "node slot {} is already occupied", id.get())
            }
            Self::NotARoot { node_id } => {
                write!(f, "node {} is not a scene root", node_id.get())
            }
            Self::NotAMeshResourceLeaf { node_id } => write!(
                f,
                "node {} is a leaf but not a mesh-resource leaf",
                node_id.get()
            ),
            Self::NotAnInlineMeshLeaf { node_id } => write!(
                f,
                "node {} is a leaf but not an inline-mesh leaf",
                node_id.get()
            ),
            Self::MeshResourceSlotOccupied { id } => {
                write!(f, "mesh resource slot {} is already occupied", id.get())
            }
            Self::MeshResourceInUse { id } => write!(
                f,
                "mesh resource {} is still referenced by at least one leaf",
                id.get()
            ),
            Self::UnevaluatedCsg { node_id } => {
                write!(f, "unevaluated CSG node {}", node_id.get())
            }
            Self::Transform(err) => write!(f, "transform error: {err}"),
        }
    }
}

impl std::error::Error for SceneError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transform(err) => Some(err),
            _ => None,
        }
    }
}

impl From<TransformError> for SceneError {
    fn from(err: TransformError) -> Self {
        Self::Transform(err)
    }
}
