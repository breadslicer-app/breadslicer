/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Node-level types of [`Scene`].

use std::num::NonZeroU32;
use std::sync::Arc;

use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::transform::Transform3D;

/// Optional human-readable name and free-form comment carried by a
/// graph node and by the graph's top-level metadata. Loaders attach
/// `name` from on-disk object directives (e.g. OBJ's `o`), and
/// `comment` from leading header comments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneMetadata {
    pub name: Option<String>,
    pub comment: Option<String>,
}

/// Stable, never-reused identifier of a node in a `Scene`.
///
/// `NonZeroU32` so `Option<NodeId>` is one machine word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct NodeId(pub(crate) NonZeroU32);

impl NodeId {
    /// Raw value of the identifier. Mostly useful for display and tests.
    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// Zero-based index into the arena that backs the parent
    /// `Scene`. Stable for the lifetime of the id. Useful for
    /// parallel-`Vec` caches indexed by `NodeId`.
    pub fn index(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// Stable, never-reused identifier of a shared mesh resource.
///
/// Distinct type from [`SubtreeResourceId`] so a leaf cannot
/// accidentally reference a subtree resource, and a `Reference`
/// cannot reference a mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct MeshResourceId(pub(crate) NonZeroU32);

impl MeshResourceId {
    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// Zero-based index into the arena that backs the parent
    /// `Scene`'s `mesh_resources`. Stable for the lifetime of
    /// the id. Useful for parallel-`Vec` caches.
    pub fn index(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// Stable, never-reused identifier of a shared subtree resource.
///
/// Resolves to the `NodeId` of the subtree's root inside the parent
/// `Scene::nodes` arena. The subtree root and its descendants live
/// in the same arena as the scene's own nodes but are unreachable from
/// `Scene::roots`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct SubtreeResourceId(pub(crate) NonZeroU32);

impl SubtreeResourceId {
    pub fn get(self) -> u32 {
        self.0.get()
    }

    pub(crate) fn index(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// Where a leaf sits in the scene graph: at a position in the scene's
/// roots list, or at a position in another node's children list.
/// Recorded by [`Scene::remove_leaf`] alongside the leaf's payload and
/// transform so the inverse [`Scene::install_leaf_at`] can re-attach
/// the leaf at the exact same place, preserving root and sibling order.
///
/// A position is measured against the list as it stood when the
/// attachment was recorded, so redeeming one against a scene that has
/// since shrunk that list is refused rather than clamped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeafAttachment {
    Root { position: usize },
    Parent { parent: NodeId, position: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub meta: Option<SceneMetadata>,
    /// Transform from this node's local frame to its parent's frame.
    /// For a root node (scene root or subtree-resource root) the parent
    /// frame is world space.
    pub local_transform: Transform3D,
    pub kind: NodeKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    /// Logical container. World geometry is the union of children's
    /// world geometry, no Boolean combinator implied.
    Group { children: Vec<NodeId> },
    /// Carrier of concrete geometry.
    Leaf(LeafPayload),
    /// CSG combinator over children. Operator semantics live on
    /// [`CsgOp`]. Invariant: for `op == CsgOp::Intersection`,
    /// `children.len() >= 1`.
    Csg { op: CsgOp, children: Vec<NodeId> },
    /// References a stored subtree by id. Evaluation expands the
    /// referenced subtree under this node's `local_transform`, one
    /// occurrence per reference. All `Reference` nodes naming the same
    /// resource share identity: an edit to a node inside the resource
    /// subtree is observed at every reference site.
    Reference { resource: SubtreeResourceId },
}

#[derive(Clone, Debug, PartialEq)]
pub enum LeafPayload {
    /// Mesh stored inline on the node. The `Arc` lets the same mesh
    /// bytes flow across scene boundaries (notably copy/paste) and be
    /// shared between an inline leaf and a [`MeshResource`] entry
    /// without deep-cloning the vertex / index buffers; the leaf still
    /// "owns" the storage in the sense that no other leaf references
    /// this `Arc` via a [`MeshResourceId`] handle.
    Mesh(Arc<IndexedMesh>),
    /// Reference to a shared mesh in `Scene::mesh_resources`.
    MeshResource(MeshResourceId),
}

/// CSG combinator over children.
///
/// `Difference`: first child MINUS the union of the rest. 0 children is
/// the empty set, 1 child is identity.
/// `Union`: union of children. 0 children is the empty set.
/// `Intersection`: intersection of children. Empty intersection is
/// rejected at construction (invariant 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsgOp {
    Union,
    Difference,
    Intersection,
}

impl NodeKind {
    /// Number of children references this kind exposes through the
    /// children-only structural graph (Group + Csg). `Leaf` and
    /// `Reference` carry no children edges in that graph; see
    /// `expansion_successor` for the graph that includes Reference
    /// edges.
    pub(crate) fn children(&self) -> &[NodeId] {
        match self {
            Self::Group { children } | Self::Csg { children, .. } => children.as_slice(),
            Self::Leaf(_) | Self::Reference { .. } => &[],
        }
    }
}

#[cfg(test)]
mod scene_node_tests {
    use super::*;

    #[test]
    fn node_id_get_returns_raw_value() {
        let id = NodeId(NonZeroU32::new(7).unwrap());
        assert_eq!(id.get(), 7);
        assert_eq!(id.index(), 6);
    }

    #[test]
    fn mesh_resource_id_get_returns_raw_value() {
        let id = MeshResourceId(NonZeroU32::new(3).unwrap());
        assert_eq!(id.get(), 3);
        assert_eq!(id.index(), 2);
    }

    #[test]
    fn subtree_resource_id_get_returns_raw_value() {
        let id = SubtreeResourceId(NonZeroU32::new(5).unwrap());
        assert_eq!(id.get(), 5);
        assert_eq!(id.index(), 4);
    }

    #[test]
    fn group_kind_exposes_children() {
        let a = NodeId(NonZeroU32::new(1).unwrap());
        let b = NodeId(NonZeroU32::new(2).unwrap());
        let kind = NodeKind::Group {
            children: vec![a, b],
        };
        assert_eq!(kind.children(), &[a, b]);
    }

    #[test]
    fn csg_kind_exposes_children() {
        let a = NodeId(NonZeroU32::new(1).unwrap());
        let kind = NodeKind::Csg {
            op: CsgOp::Union,
            children: vec![a],
        };
        assert_eq!(kind.children(), &[a]);
    }

    #[test]
    fn leaf_kind_exposes_no_children() {
        let kind = NodeKind::Leaf(LeafPayload::MeshResource(MeshResourceId(
            NonZeroU32::new(1).unwrap(),
        )));
        assert!(kind.children().is_empty());
    }

    #[test]
    fn reference_kind_exposes_no_children_through_structural_graph() {
        let kind = NodeKind::Reference {
            resource: SubtreeResourceId(NonZeroU32::new(1).unwrap()),
        };
        assert!(kind.children().is_empty());
    }

    #[test]
    fn nonzero_u32_makes_option_node_id_one_word() {
        assert_eq!(size_of::<Option<NodeId>>(), size_of::<u32>());
        assert_eq!(size_of::<Option<MeshResourceId>>(), size_of::<u32>());
        assert_eq!(size_of::<Option<SubtreeResourceId>>(), size_of::<u32>());
    }
}
