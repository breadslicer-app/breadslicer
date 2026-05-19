/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Hierarchical [`Scene`] aggregate and its mutation / validation
//! API. Arena-backed; per-mesh AABB and bake helpers live in the
//! sibling `scene_aabb` / `scene_bake` modules; typed edits in
//! `scene_edit`.

use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU32;

use crate::indexed_mesh::IndexedMesh;
use crate::scene_node::{
    CsgOp, LeafPayload, MeshResourceId, Node, NodeId, NodeKind, SceneMetadata, SubtreeResourceId,
};
use crate::transform::{Transform3D, TransformError};

/// Recover the [`MeshResourceId`] for the slot at index `i` in the
/// resource arena. Slots are append-only and never reused, so the
/// id at index `i` is always `MeshResourceId(NonZeroU32::new(i + 1))`.
fn mesh_resource_id_at_index(i: usize) -> MeshResourceId {
    let raw = u32::try_from(i + 1).expect("mesh resource index overflow");
    MeshResourceId(NonZeroU32::new(raw).expect("index + 1 is non-zero"))
}

/// Failure modes of [`Scene`] mutators, validators and queries.
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
    /// The expansion graph (children edges plus `Instance -> resource root`)
    /// contains a cycle reachable from this node.
    InstanceCycle { node_id: NodeId },
    /// The children-only structural graph contains a cycle reachable from
    /// this node. Mutators prevent this, so the variant signals a
    /// corrupted graph rather than a routine API misuse.
    StructuralCycle { node_id: NodeId },
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
    /// Instance node.
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
            Self::InstanceCycle { node_id } => {
                write!(f, "instance cycle reachable from node {}", node_id.get())
            }
            Self::StructuralCycle { node_id } => {
                write!(f, "structural cycle reachable from node {}", node_id.get())
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

/// Hierarchical scene aggregate. Arena-based: nodes, mesh resources and
/// subtree resources each live in a `Vec<Option<_>>` indexed by their
/// `NonZeroU32`-backed identifier minus one. Identifiers are never
/// reused, so handles held across mutations stay valid (or surface a
/// dangling-id error rather than silently aliasing a different entity).
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub(crate) meta: Option<SceneMetadata>,
    pub(crate) nodes: Vec<Option<Node>>,
    pub(crate) mesh_resources: Vec<Option<IndexedMesh>>,
    pub(crate) subtree_resources: Vec<Option<NodeId>>,
    pub(crate) roots: Vec<NodeId>,
    pub(crate) parents: Vec<Option<NodeId>>,
    next_node: NonZeroU32,
    next_mesh_resource: NonZeroU32,
    next_subtree_resource: NonZeroU32,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    pub fn new() -> Self {
        Self {
            meta: None,
            nodes: Vec::new(),
            mesh_resources: Vec::new(),
            subtree_resources: Vec::new(),
            roots: Vec::new(),
            parents: Vec::new(),
            next_node: NonZeroU32::new(1).unwrap(),
            next_mesh_resource: NonZeroU32::new(1).unwrap(),
            next_subtree_resource: NonZeroU32::new(1).unwrap(),
        }
    }

    pub fn meta(&self) -> &Option<SceneMetadata> {
        &self.meta
    }

    pub fn set_meta(&mut self, meta: Option<SceneMetadata>) {
        self.meta = meta;
    }

    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    /// Number of slots in the node arena, including tombstones for
    /// deleted nodes. Always `>=` the count of live nodes. Useful as
    /// the size for parallel-`Vec` caches indexed by `NodeId::index()`.
    pub fn nodes_len(&self) -> usize {
        self.nodes.len()
    }

    /// Number of slots in the mesh-resource arena, including
    /// tombstones for deleted resources. Useful as the size for
    /// parallel-`Vec` caches indexed by `MeshResourceId::index()`.
    pub fn mesh_resources_len(&self) -> usize {
        self.mesh_resources.len()
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())?.as_ref()
    }

    pub fn mesh_resource(&self, id: MeshResourceId) -> Option<&IndexedMesh> {
        self.mesh_resources.get(id.index())?.as_ref()
    }

    /// Mutable borrow into the addressed mesh resource. `None` on a
    /// dangling or tombstoned id.
    pub fn mesh_resource_mut(&mut self, id: MeshResourceId) -> Option<&mut IndexedMesh> {
        self.mesh_resources.get_mut(id.index())?.as_mut()
    }

    /// Iterate every live mesh resource as `(id, &mesh)` pairs.
    /// Tombstoned slots are skipped.
    pub fn mesh_resources_iter(&self) -> impl Iterator<Item = (MeshResourceId, &IndexedMesh)> + '_ {
        self.mesh_resources
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                slot.as_ref()
                    .map(|mesh| (mesh_resource_id_at_index(i), mesh))
            })
    }

    /// Mutable variant of [`Self::mesh_resources_iter`].
    pub fn mesh_resources_iter_mut(
        &mut self,
    ) -> impl Iterator<Item = (MeshResourceId, &mut IndexedMesh)> + '_ {
        self.mesh_resources
            .iter_mut()
            .enumerate()
            .filter_map(|(i, slot)| {
                slot.as_mut()
                    .map(|mesh| (mesh_resource_id_at_index(i), mesh))
            })
    }

    /// Count of reachable leaves under `roots()`, traversed in
    /// pre-order. Mirrors what callers see if they enumerate via
    /// repeated `leaf_id_at_index(0..)` calls but does it in a
    /// single O(n) walk.
    pub fn reachable_leaf_count(&self) -> usize {
        let mut count = 0usize;
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = self.node(id) else {
                continue;
            };
            match &node.kind {
                NodeKind::Leaf(_) => count += 1,
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                    for child in children.iter().rev() {
                        stack.push(*child);
                    }
                }
                NodeKind::Instance { .. } => {}
            }
        }
        count
    }

    /// The first reachable leaf in `roots()` pre-order traversal, or
    /// `None` when the graph has no leaves. Format writers that walk
    /// the graph and have already verified `reachable_leaf_count() == 1`
    /// use this to grab the only leaf without re-walking via an index.
    pub fn first_leaf_id(&self) -> Option<NodeId> {
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = self.node(id) else {
                continue;
            };
            match &node.kind {
                NodeKind::Leaf(_) => return Some(id),
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                    for child in children.iter().rev() {
                        stack.push(*child);
                    }
                }
                NodeKind::Instance { .. } => {}
            }
        }
        None
    }

    /// Mutable borrow into the inline mesh of a `Leaf(Mesh(_))` node.
    /// `None` for any other node kind (groups, CSG, instances,
    /// mesh-resource-backed leaves) or a dangling id. Focused
    /// accessor: callers can repair the mesh in place without being
    /// handed `&mut Node` (which would let them change `kind` and
    /// break parent / child invariants).
    pub fn inline_mesh_mut(&mut self, id: NodeId) -> Option<&mut IndexedMesh> {
        let node = self.nodes.get_mut(id.index())?.as_mut()?;
        match &mut node.kind {
            NodeKind::Leaf(LeafPayload::Mesh(mesh)) => Some(mesh),
            _ => None,
        }
    }

    /// Move a `Leaf(Mesh(_))` node's inline mesh into the
    /// mesh-resource arena, rewriting the leaf to reference the new
    /// resource. Idempotent on a `Leaf(MeshResource(_))`: returns the
    /// already-registered id without mutating.
    ///
    /// The leaf's parent / child wiring, [`SceneMetadata`] and
    /// `local_transform` are preserved; only the payload variant
    /// changes.
    ///
    /// Errors:
    /// - [`SceneError::DanglingNodeId`] if no live node lives at
    ///   `leaf_id`.
    /// - [`SceneError::NotALeaf`] if the node is not a `Leaf`.
    ///
    /// On error the scene is unchanged.
    pub fn convert_leaf_inline_to_mesh_resource(
        &mut self,
        leaf_id: NodeId,
    ) -> Result<MeshResourceId, SceneError> {
        let mesh = {
            let node = self
                .nodes
                .get_mut(leaf_id.index())
                .and_then(|slot| slot.as_mut())
                .ok_or(SceneError::DanglingNodeId { id: leaf_id })?;
            match &node.kind {
                NodeKind::Leaf(LeafPayload::MeshResource(rid)) => return Ok(*rid),
                NodeKind::Leaf(LeafPayload::Mesh(_)) => {}
                _ => return Err(SceneError::NotALeaf { node_id: leaf_id }),
            }
            let kind = std::mem::replace(
                &mut node.kind,
                NodeKind::Group {
                    children: Vec::new(),
                },
            );
            let NodeKind::Leaf(LeafPayload::Mesh(mesh)) = kind else {
                unreachable!("kind was just checked to be Leaf(Mesh(_))");
            };
            mesh
        };
        let rid = self.add_mesh_resource(mesh);
        let node = self.nodes[leaf_id.index()]
            .as_mut()
            .expect("node existed at the start of the call");
        node.kind = NodeKind::Leaf(LeafPayload::MeshResource(rid));
        Ok(rid)
    }

    pub fn subtree_resource_root(&self, id: SubtreeResourceId) -> Option<NodeId> {
        self.subtree_resources.get(id.index())?.as_ref().copied()
    }

    pub fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        self.parents.get(id.index()).copied().flatten()
    }

    pub(crate) fn is_subtree_resource_root(&self, id: NodeId) -> bool {
        self.subtree_resources
            .iter()
            .any(|slot| slot.as_ref() == Some(&id))
    }

    fn allocate_node_slot(&mut self) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node = self.next_node.checked_add(1).expect("node id overflow");
        self.nodes.push(None);
        self.parents.push(None);
        id
    }

    fn allocate_mesh_resource_slot(&mut self) -> MeshResourceId {
        let id = MeshResourceId(self.next_mesh_resource);
        self.next_mesh_resource = self
            .next_mesh_resource
            .checked_add(1)
            .expect("mesh resource id overflow");
        self.mesh_resources.push(None);
        id
    }

    fn allocate_subtree_resource_slot(&mut self) -> SubtreeResourceId {
        let id = SubtreeResourceId(self.next_subtree_resource);
        self.next_subtree_resource = self
            .next_subtree_resource
            .checked_add(1)
            .expect("subtree resource id overflow");
        self.subtree_resources.push(None);
        id
    }

    /// Allocate a fresh [`NodeId`] without installing any node. The
    /// returned id's arena slot is `None` and the id is not registered
    /// as a scene root.
    ///
    /// Use this when the id needs to exist before the node it will
    /// reference does, typically so the id can be embedded in a typed
    /// edit (see `sc_mesh_core::scene_edit`) that the undo/redo
    /// machinery replays against the same slot.
    ///
    /// The slot is permanently consumed: dropping the id without
    /// later installing a node at it leaks one arena slot. Slots are
    /// tombstoned but never reused, so this is benign at the scale of
    /// user-driven editing.
    pub fn allocate_unbound_node_id(&mut self) -> NodeId {
        self.allocate_node_slot()
    }

    pub fn add_mesh_resource(&mut self, mesh: IndexedMesh) -> MeshResourceId {
        let id = self.allocate_mesh_resource_slot();
        self.mesh_resources[id.index()] = Some(mesh);
        id
    }

    pub fn add_group(
        &mut self,
        meta: Option<SceneMetadata>,
        local_transform: Transform3D,
    ) -> NodeId {
        let id = self.allocate_node_slot();
        self.nodes[id.index()] = Some(Node {
            meta,
            local_transform,
            kind: NodeKind::Group {
                children: Vec::new(),
            },
        });
        id
    }

    pub fn add_leaf(
        &mut self,
        meta: Option<SceneMetadata>,
        local_transform: Transform3D,
        payload: LeafPayload,
    ) -> Result<NodeId, SceneError> {
        if let LeafPayload::MeshResource(rid) = payload
            && self.mesh_resource(rid).is_none()
        {
            return Err(SceneError::DanglingMeshResource { id: rid });
        }
        let id = self.allocate_node_slot();
        self.nodes[id.index()] = Some(Node {
            meta,
            local_transform,
            kind: NodeKind::Leaf(payload),
        });
        Ok(id)
    }

    pub fn add_csg(
        &mut self,
        meta: Option<SceneMetadata>,
        local_transform: Transform3D,
        op: CsgOp,
        children: Vec<NodeId>,
    ) -> Result<NodeId, SceneError> {
        if op == CsgOp::Intersection && children.is_empty() {
            return Err(SceneError::EmptyIntersection {
                node_id: NodeId(self.next_node),
            });
        }
        let mut seen: HashSet<NodeId> = HashSet::with_capacity(children.len());
        for &cid in &children {
            if self.node(cid).is_none() {
                return Err(SceneError::DanglingNodeId { id: cid });
            }
            if !seen.insert(cid) {
                return Err(SceneError::DuplicateChild {
                    parent: NodeId(self.next_node),
                    child: cid,
                });
            }
            if let Some(p) = self.parent_of(cid) {
                return Err(SceneError::AlreadyHasParent {
                    node_id: cid,
                    parent: p,
                });
            }
            if self.roots.contains(&cid) {
                return Err(SceneError::AlreadyARoot { node_id: cid });
            }
            if self.is_subtree_resource_root(cid) {
                return Err(SceneError::AlreadyASubtreeResourceRoot { node_id: cid });
            }
        }
        let id = self.allocate_node_slot();
        for &cid in &children {
            self.parents[cid.index()] = Some(id);
        }
        self.nodes[id.index()] = Some(Node {
            meta,
            local_transform,
            kind: NodeKind::Csg { op, children },
        });
        Ok(id)
    }

    pub fn add_instance(
        &mut self,
        meta: Option<SceneMetadata>,
        local_transform: Transform3D,
        resource: SubtreeResourceId,
    ) -> Result<NodeId, SceneError> {
        if self.subtree_resource_root(resource).is_none() {
            return Err(SceneError::DanglingSubtreeResource { id: resource });
        }
        let id = self.allocate_node_slot();
        self.nodes[id.index()] = Some(Node {
            meta,
            local_transform,
            kind: NodeKind::Instance { resource },
        });
        Ok(id)
    }

    pub fn register_subtree_resource(
        &mut self,
        root: NodeId,
    ) -> Result<SubtreeResourceId, SceneError> {
        if self.node(root).is_none() {
            return Err(SceneError::DanglingNodeId { id: root });
        }
        if let Some(p) = self.parent_of(root) {
            return Err(SceneError::AlreadyHasParent {
                node_id: root,
                parent: p,
            });
        }
        if self.roots.contains(&root) {
            return Err(SceneError::AlreadyARoot { node_id: root });
        }
        if self.is_subtree_resource_root(root) {
            return Err(SceneError::AlreadyASubtreeResourceRoot { node_id: root });
        }
        let id = self.allocate_subtree_resource_slot();
        self.subtree_resources[id.index()] = Some(root);
        Ok(id)
    }

    pub fn add_root(&mut self, id: NodeId) -> Result<(), SceneError> {
        if self.node(id).is_none() {
            return Err(SceneError::DanglingNodeId { id });
        }
        if let Some(p) = self.parent_of(id) {
            return Err(SceneError::AlreadyHasParent {
                node_id: id,
                parent: p,
            });
        }
        if self.roots.contains(&id) {
            return Err(SceneError::AlreadyARoot { node_id: id });
        }
        if self.is_subtree_resource_root(id) {
            return Err(SceneError::AlreadyASubtreeResourceRoot { node_id: id });
        }
        self.roots.push(id);
        Ok(())
    }

    pub fn add_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), SceneError> {
        if self.node(parent).is_none() {
            return Err(SceneError::DanglingNodeId { id: parent });
        }
        if self.node(child).is_none() {
            return Err(SceneError::DanglingNodeId { id: child });
        }
        match self.node(parent).map(|n| &n.kind) {
            Some(NodeKind::Group { .. }) | Some(NodeKind::Csg { .. }) => {}
            _ => return Err(SceneError::NotABranch { node_id: parent }),
        }
        if let Some(p) = self.parent_of(child) {
            return Err(SceneError::AlreadyHasParent {
                node_id: child,
                parent: p,
            });
        }
        if self.roots.contains(&child) {
            return Err(SceneError::AlreadyARoot { node_id: child });
        }
        if self.is_subtree_resource_root(child) {
            return Err(SceneError::AlreadyASubtreeResourceRoot { node_id: child });
        }
        let parent_node = self.nodes[parent.index()].as_mut().expect("checked above");
        match &mut parent_node.kind {
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                children.push(child);
            }
            _ => unreachable!("NotABranch was checked above"),
        }
        self.parents[child.index()] = Some(parent);
        if let Some(cycle_id) = self.find_cycle_in_expansion() {
            let parent_node = self.nodes[parent.index()].as_mut().expect("just mutated");
            match &mut parent_node.kind {
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                    children.pop();
                }
                _ => unreachable!(),
            }
            self.parents[child.index()] = None;
            return Err(SceneError::InstanceCycle { node_id: cycle_id });
        }
        Ok(())
    }

    pub fn set_local_transform(
        &mut self,
        id: NodeId,
        local_transform: Transform3D,
    ) -> Result<(), SceneError> {
        let node = self
            .nodes
            .get_mut(id.index())
            .and_then(|slot| slot.as_mut())
            .ok_or(SceneError::DanglingNodeId { id })?;
        node.local_transform = local_transform;
        Ok(())
    }

    /// Install a mesh-resource-backed leaf at a previously-allocated,
    /// currently-empty node slot, and register it as a scene root.
    ///
    /// Paired with [`Self::remove_root_leaf`] so a typed edit (see
    /// `sc_mesh_core::scene_edit`) can round-trip a paste insertion
    /// through the undo/redo machinery while keeping the same
    /// [`NodeId`] across applies.
    ///
    /// Errors:
    /// - [`SceneError::DanglingNodeId`] if `id` is past the arena's
    ///   high-water mark.
    /// - [`SceneError::NodeSlotOccupied`] if a node already lives at
    ///   that id.
    /// - [`SceneError::DanglingMeshResource`] if `mesh` is not a
    ///   registered resource id.
    ///
    /// On error the scene is unchanged. The installed leaf has no
    /// metadata; callers that need [`SceneMetadata`] on the new root
    /// should go through [`Self::add_leaf`] + [`Self::add_root`].
    pub fn install_root_leaf(
        &mut self,
        id: NodeId,
        mesh: MeshResourceId,
        local_transform: Transform3D,
    ) -> Result<(), SceneError> {
        if id.index() >= self.nodes.len() {
            return Err(SceneError::DanglingNodeId { id });
        }
        if self.nodes[id.index()].is_some() {
            return Err(SceneError::NodeSlotOccupied { id });
        }
        if self.mesh_resource(mesh).is_none() {
            return Err(SceneError::DanglingMeshResource { id: mesh });
        }
        self.nodes[id.index()] = Some(Node {
            meta: None,
            local_transform,
            kind: NodeKind::Leaf(LeafPayload::MeshResource(mesh)),
        });
        self.roots.push(id);
        Ok(())
    }

    /// Tombstone a mesh-resource-backed root leaf: drop it from the
    /// roots list and clear its arena slot. Returns the
    /// [`MeshResourceId`] and [`Transform3D`] that were attached, so
    /// the paired [`Self::install_root_leaf`] can re-install the same
    /// payload at the same id.
    ///
    /// Any [`SceneMetadata`] on the removed leaf is dropped; callers
    /// that need to round-trip metadata must snapshot it before
    /// calling.
    ///
    /// Errors:
    /// - [`SceneError::DanglingNodeId`] if no live node lives at `id`.
    /// - [`SceneError::NotALeaf`] if the node is not a Leaf.
    /// - [`SceneError::NotAMeshResourceLeaf`] if the leaf's payload is
    ///   an inline mesh rather than a [`LeafPayload::MeshResource`].
    /// - [`SceneError::NotARoot`] if the leaf is parented rather than
    ///   a scene root.
    ///
    /// On error the scene is unchanged.
    pub fn remove_root_leaf(
        &mut self,
        id: NodeId,
    ) -> Result<(MeshResourceId, Transform3D), SceneError> {
        let node = self.node(id).ok_or(SceneError::DanglingNodeId { id })?;
        let (mesh, local_transform) = match &node.kind {
            NodeKind::Leaf(LeafPayload::MeshResource(rid)) => (*rid, node.local_transform),
            NodeKind::Leaf(LeafPayload::Mesh(_)) => {
                return Err(SceneError::NotAMeshResourceLeaf { node_id: id });
            }
            _ => return Err(SceneError::NotALeaf { node_id: id }),
        };
        let root_pos = self
            .roots
            .iter()
            .position(|&r| r == id)
            .ok_or(SceneError::NotARoot { node_id: id })?;
        self.roots.remove(root_pos);
        self.nodes[id.index()] = None;
        Ok((mesh, local_transform))
    }

    pub(crate) fn expansion_successor(&self, node_idx: usize, child_i: usize) -> Option<NodeId> {
        let node = self.nodes[node_idx].as_ref()?;
        match &node.kind {
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                children.get(child_i).copied()
            }
            NodeKind::Instance { resource } => {
                if child_i == 0 {
                    self.subtree_resource_root(*resource)
                } else {
                    None
                }
            }
            NodeKind::Leaf(_) => None,
        }
    }

    pub(crate) fn find_cycle_in_expansion(&self) -> Option<NodeId> {
        // Three-color iterative DFS over the expansion graph (children
        // edges plus `Instance -> resource root` edges). A back edge to
        // a gray vertex is the cycle witness; we return the id of the
        // vertex the back edge points to.
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

    /// Re-check every structural invariant of this graph: live ids in
    /// every reference, intersection non-empty, parent/child agreement,
    /// expansion-graph acyclicity.
    pub fn validate(&self) -> Result<(), SceneError> {
        // (1) Every NodeId referenced exists. (2)(3) Likewise for
        // resource ids. (5) Intersection has >= 1 child.
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
                NodeKind::Instance { resource } => {
                    if self.subtree_resource_root(*resource).is_none() {
                        return Err(SceneError::DanglingSubtreeResource { id: *resource });
                    }
                }
            }
        }

        // Validate subtree-resource roots point to live nodes.
        for slot in &self.subtree_resources {
            if let Some(root) = slot
                && self.node(*root).is_none()
            {
                return Err(SceneError::DanglingNodeId { id: *root });
            }
        }

        // Validate scene roots point to live nodes and are not also
        // subtree-resource roots.
        for &root in &self.roots {
            if self.node(root).is_none() {
                return Err(SceneError::DanglingNodeId { id: root });
            }
            if self.is_subtree_resource_root(root) {
                return Err(SceneError::AlreadyASubtreeResourceRoot { node_id: root });
            }
        }

        // (4) Forest: rebuild expected parents from children lists and
        // compare to self.parents. Also catches duplicate-child within a
        // single parent.
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

        // (8) Expansion-graph acyclicity.
        if let Some(witness) = self.find_cycle_in_expansion() {
            // The children-only graph cannot cycle under the parent
            // bookkeeping above, so any cycle witnessed here must
            // involve an Instance edge.
            return Err(SceneError::InstanceCycle { node_id: witness });
        }

        Ok(())
    }

    fn node_id_at(&self, slot_idx: usize) -> NodeId {
        let raw = u32::try_from(slot_idx + 1).expect("arena index fits in u32");
        NodeId(NonZeroU32::new(raw).expect("index +1 is non-zero"))
    }
}

impl From<IndexedMesh> for Scene {
    /// Single-leaf graph carrying `mesh` under identity transform.
    /// Used by format readers whose on-disk model is a single mesh
    /// (STL) and by test fixtures that need a one-object scene.
    fn from(mesh: IndexedMesh) -> Self {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh))
            .expect("freshly allocated graph has no resources to dangle");
        g.add_root(leaf)
            .expect("freshly allocated leaf has no parent and is not a root");
        g
    }
}

#[cfg(test)]
mod scene_construction_tests {
    use super::*;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::transform::AffineTransform3D;

    fn unit_triangle() -> IndexedMesh {
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

    #[test]
    fn new_is_empty() {
        let g = Scene::new();
        assert!(g.roots().is_empty());
        assert!(g.meta().is_none());
    }

    #[test]
    fn node_ids_strictly_increase() {
        let mut g = Scene::new();
        let a = g.add_group(None, Transform3D::Identity);
        let b = g.add_group(None, Transform3D::Identity);
        let c = g.add_group(None, Transform3D::Identity);
        assert_eq!(a.get(), 1);
        assert_eq!(b.get(), 2);
        assert_eq!(c.get(), 3);
    }

    #[test]
    fn allocate_unbound_node_id_returns_fresh_ids() {
        let mut g = Scene::new();
        let a = g.allocate_unbound_node_id();
        let b = g.allocate_unbound_node_id();
        assert_ne!(a, b);
        assert_eq!(a.get(), 1);
        assert_eq!(b.get(), 2);
    }

    #[test]
    fn allocate_unbound_node_id_leaves_slot_empty() {
        let mut g = Scene::new();
        let id = g.allocate_unbound_node_id();
        assert!(g.node(id).is_none());
        assert!(g.parent_of(id).is_none());
        assert!(!g.roots().contains(&id));
    }

    #[test]
    fn allocate_unbound_node_id_grows_arena_by_one_slot() {
        let mut g = Scene::new();
        let before = g.nodes_len();
        let _ = g.allocate_unbound_node_id();
        assert_eq!(g.nodes_len(), before + 1);
    }

    #[test]
    fn allocate_unbound_node_id_shares_counter_with_installers() {
        // Mixing the unbound allocator with installer paths must not
        // collide: each call bumps the same monotonic counter.
        let mut g = Scene::new();
        let a = g.add_group(None, Transform3D::Identity);
        let b = g.allocate_unbound_node_id();
        let c = g.add_group(None, Transform3D::Identity);
        assert_eq!(a.get(), 1);
        assert_eq!(b.get(), 2);
        assert_eq!(c.get(), 3);
        assert!(g.node(a).is_some());
        assert!(g.node(b).is_none());
        assert!(g.node(c).is_some());
    }

    #[test]
    fn add_leaf_with_inline_mesh_succeeds() {
        let mut g = Scene::new();
        let id = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        assert!(matches!(
            g.node(id).map(|n| &n.kind),
            Some(NodeKind::Leaf(_))
        ));
    }

    #[test]
    fn add_leaf_with_valid_mesh_resource_succeeds() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let id = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        assert_eq!(g.mesh_resource(rid).unwrap().vertices.len(), 3);
        assert!(g.node(id).is_some());
    }

    #[test]
    fn add_leaf_with_dangling_mesh_resource_errors() {
        let mut g = Scene::new();
        let bogus = MeshResourceId(NonZeroU32::new(99).unwrap());
        let result = g.add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::MeshResource(bogus),
        );
        assert_eq!(result, Err(SceneError::DanglingMeshResource { id: bogus }));
    }

    #[test]
    fn add_csg_union_with_no_children_succeeds() {
        let mut g = Scene::new();
        let id = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, Vec::new())
            .unwrap();
        assert!(matches!(
            g.node(id).map(|n| &n.kind),
            Some(NodeKind::Csg { op: CsgOp::Union, children }) if children.is_empty()
        ));
    }

    #[test]
    fn add_csg_intersection_with_no_children_errors() {
        let mut g = Scene::new();
        let result = g.add_csg(None, Transform3D::Identity, CsgOp::Intersection, Vec::new());
        assert!(matches!(result, Err(SceneError::EmptyIntersection { .. })));
    }

    #[test]
    fn add_csg_intersection_with_one_child_succeeds() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Intersection, vec![leaf])
            .unwrap();
        assert_eq!(g.parent_of(leaf), Some(csg));
    }

    #[test]
    fn add_csg_with_duplicate_child_errors() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        let result = g.add_csg(None, Transform3D::Identity, CsgOp::Union, vec![leaf, leaf]);
        assert!(matches!(result, Err(SceneError::DuplicateChild { .. })));
        assert_eq!(g.parent_of(leaf), None);
    }

    #[test]
    fn add_csg_with_dangling_child_errors() {
        let mut g = Scene::new();
        let bogus = NodeId(NonZeroU32::new(99).unwrap());
        let result = g.add_csg(None, Transform3D::Identity, CsgOp::Union, vec![bogus]);
        assert_eq!(result, Err(SceneError::DanglingNodeId { id: bogus }));
    }

    #[test]
    fn add_csg_with_already_parented_child_errors() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_child(group, leaf).unwrap();
        let result = g.add_csg(None, Transform3D::Identity, CsgOp::Union, vec![leaf]);
        assert!(matches!(result, Err(SceneError::AlreadyHasParent { .. })));
    }

    #[test]
    fn add_root_happy_path() {
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_root(group).unwrap();
        assert_eq!(g.roots(), &[group]);
    }

    #[test]
    fn add_root_twice_on_same_node_errors() {
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_root(group).unwrap();
        let result = g.add_root(group);
        assert_eq!(result, Err(SceneError::AlreadyARoot { node_id: group }));
    }

    #[test]
    fn add_root_on_parented_node_errors() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        let child = g.add_group(None, Transform3D::Identity);
        g.add_child(parent, child).unwrap();
        let result = g.add_root(child);
        assert!(matches!(result, Err(SceneError::AlreadyHasParent { .. })));
    }

    #[test]
    fn add_root_on_subtree_resource_root_errors() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.register_subtree_resource(root).unwrap();
        let result = g.add_root(root);
        assert_eq!(
            result,
            Err(SceneError::AlreadyASubtreeResourceRoot { node_id: root })
        );
    }

    #[test]
    fn add_child_under_group() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        let child = g.add_group(None, Transform3D::Identity);
        g.add_child(parent, child).unwrap();
        assert_eq!(g.parent_of(child), Some(parent));
        let kind = &g.node(parent).unwrap().kind;
        assert!(matches!(
            kind,
            NodeKind::Group { children } if children == &vec![child]
        ));
    }

    #[test]
    fn add_child_under_csg() {
        let mut g = Scene::new();
        let parent = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, Vec::new())
            .unwrap();
        let child = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        g.add_child(parent, child).unwrap();
        assert_eq!(g.parent_of(child), Some(parent));
    }

    #[test]
    fn add_child_under_leaf_errors() {
        let mut g = Scene::new();
        let parent = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        let child = g.add_group(None, Transform3D::Identity);
        let result = g.add_child(parent, child);
        assert_eq!(result, Err(SceneError::NotABranch { node_id: parent }));
    }

    #[test]
    fn add_child_under_instance_errors() {
        let mut g = Scene::new();
        let res_root = g.add_group(None, Transform3D::Identity);
        let rid = g.register_subtree_resource(res_root).unwrap();
        let parent = g.add_instance(None, Transform3D::Identity, rid).unwrap();
        let child = g.add_group(None, Transform3D::Identity);
        let result = g.add_child(parent, child);
        assert_eq!(result, Err(SceneError::NotABranch { node_id: parent }));
    }

    #[test]
    fn add_child_with_already_parented_child_errors() {
        let mut g = Scene::new();
        let p1 = g.add_group(None, Transform3D::Identity);
        let p2 = g.add_group(None, Transform3D::Identity);
        let child = g.add_group(None, Transform3D::Identity);
        g.add_child(p1, child).unwrap();
        let result = g.add_child(p2, child);
        assert!(matches!(result, Err(SceneError::AlreadyHasParent { .. })));
    }

    #[test]
    fn add_child_with_scene_root_child_errors() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        let child = g.add_group(None, Transform3D::Identity);
        g.add_root(child).unwrap();
        let result = g.add_child(parent, child);
        assert_eq!(result, Err(SceneError::AlreadyARoot { node_id: child }));
    }

    #[test]
    fn add_child_direct_instance_cycle_rejected() {
        let mut g = Scene::new();
        let res_root = g.add_group(None, Transform3D::Identity);
        let rid = g.register_subtree_resource(res_root).unwrap();
        let inst = g.add_instance(None, Transform3D::Identity, rid).unwrap();
        // Try to put the Instance inside its own subtree.
        let result = g.add_child(res_root, inst);
        assert!(matches!(result, Err(SceneError::InstanceCycle { .. })));
        // Rollback worked: instance still parent-less, res_root has no children.
        assert_eq!(g.parent_of(inst), None);
        assert!(matches!(
            &g.node(res_root).unwrap().kind,
            NodeKind::Group { children } if children.is_empty()
        ));
    }

    #[test]
    fn add_child_indirect_instance_cycle_rejected() {
        // S1 root contains instance to S2; S2 root would then need to
        // contain instance to S1 to close the loop.
        let mut g = Scene::new();
        let s1_root = g.add_group(None, Transform3D::Identity);
        let s2_root = g.add_group(None, Transform3D::Identity);
        let s1_id = g.register_subtree_resource(s1_root).unwrap();
        let s2_id = g.register_subtree_resource(s2_root).unwrap();
        let inst_to_s2 = g.add_instance(None, Transform3D::Identity, s2_id).unwrap();
        g.add_child(s1_root, inst_to_s2).unwrap();
        let inst_to_s1 = g.add_instance(None, Transform3D::Identity, s1_id).unwrap();
        let result = g.add_child(s2_root, inst_to_s1);
        assert!(matches!(result, Err(SceneError::InstanceCycle { .. })));
    }

    #[test]
    fn register_subtree_resource_happy_path() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        let rid = g.register_subtree_resource(root).unwrap();
        assert_eq!(g.subtree_resource_root(rid), Some(root));
    }

    #[test]
    fn register_subtree_resource_for_scene_root_errors() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let result = g.register_subtree_resource(root);
        assert_eq!(result, Err(SceneError::AlreadyARoot { node_id: root }));
    }

    #[test]
    fn register_subtree_resource_for_parented_node_errors() {
        let mut g = Scene::new();
        let parent = g.add_group(None, Transform3D::Identity);
        let child = g.add_group(None, Transform3D::Identity);
        g.add_child(parent, child).unwrap();
        let result = g.register_subtree_resource(child);
        assert!(matches!(result, Err(SceneError::AlreadyHasParent { .. })));
    }

    #[test]
    fn register_subtree_resource_twice_errors() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.register_subtree_resource(root).unwrap();
        let result = g.register_subtree_resource(root);
        assert_eq!(
            result,
            Err(SceneError::AlreadyASubtreeResourceRoot { node_id: root })
        );
    }

    #[test]
    fn add_instance_with_dangling_resource_errors() {
        let mut g = Scene::new();
        let bogus = SubtreeResourceId(NonZeroU32::new(7).unwrap());
        let result = g.add_instance(None, Transform3D::Identity, bogus);
        assert_eq!(
            result,
            Err(SceneError::DanglingSubtreeResource { id: bogus })
        );
    }

    #[test]
    fn set_local_transform_changes_node_transform() {
        let mut g = Scene::new();
        let id = g.add_group(None, Transform3D::Identity);
        let new_t = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        g.set_local_transform(id, new_t).unwrap();
        assert_eq!(g.node(id).unwrap().local_transform, new_t);
    }

    #[test]
    fn set_local_transform_on_dangling_id_errors() {
        let mut g = Scene::new();
        let bogus = NodeId(NonZeroU32::new(9).unwrap());
        let result = g.set_local_transform(bogus, Transform3D::Identity);
        assert_eq!(result, Err(SceneError::DanglingNodeId { id: bogus }));
    }

    #[test]
    fn mesh_resource_mut_returns_writable_borrow() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let mesh = g.mesh_resource_mut(rid).expect("resource lives");
        mesh.vertices.push(Vertex::new([2.0, 2.0, 2.0]));
        assert_eq!(g.mesh_resource(rid).unwrap().vertices.len(), 4);
    }

    #[test]
    fn mesh_resource_mut_on_dangling_id_returns_none() {
        let mut g = Scene::new();
        let bogus = MeshResourceId(NonZeroU32::new(42).unwrap());
        assert!(g.mesh_resource_mut(bogus).is_none());
    }

    #[test]
    fn mesh_resources_iter_yields_every_live_slot() {
        let mut g = Scene::new();
        let a = g.add_mesh_resource(unit_triangle());
        let b = g.add_mesh_resource(unit_triangle());
        let collected: Vec<MeshResourceId> = g.mesh_resources_iter().map(|(id, _)| id).collect();
        assert_eq!(collected, vec![a, b]);
    }

    #[test]
    fn mesh_resources_iter_mut_allows_in_place_edit() {
        let mut g = Scene::new();
        let _ = g.add_mesh_resource(unit_triangle());
        let _ = g.add_mesh_resource(unit_triangle());
        for (_id, mesh) in g.mesh_resources_iter_mut() {
            mesh.vertices.clear();
        }
        for (_, mesh) in g.mesh_resources_iter() {
            assert!(mesh.vertices.is_empty());
        }
    }

    #[test]
    fn inline_mesh_mut_returns_writable_leaf_payload() {
        let mut g = Scene::new();
        let id = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        let mesh = g.inline_mesh_mut(id).expect("inline-mesh leaf is writable");
        mesh.vertices.push(Vertex::new([3.0, 3.0, 3.0]));
        match &g.node(id).unwrap().kind {
            NodeKind::Leaf(LeafPayload::Mesh(m)) => assert_eq!(m.vertices.len(), 4),
            _ => panic!("expected inline-mesh leaf"),
        }
    }

    #[test]
    fn inline_mesh_mut_returns_none_for_group() {
        let mut g = Scene::new();
        let id = g.add_group(None, Transform3D::Identity);
        assert!(g.inline_mesh_mut(id).is_none());
    }

    #[test]
    fn inline_mesh_mut_returns_none_for_resource_backed_leaf() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let id = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        assert!(g.inline_mesh_mut(id).is_none());
    }

    #[test]
    fn inline_mesh_mut_on_dangling_id_returns_none() {
        let mut g = Scene::new();
        let bogus = NodeId(NonZeroU32::new(99).unwrap());
        assert!(g.inline_mesh_mut(bogus).is_none());
    }

    #[test]
    fn reachable_leaf_count_matches_repeated_leaf_id_at_index() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        for _ in 0..3 {
            let leaf = g
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(unit_triangle()),
                )
                .unwrap();
            g.add_child(root, leaf).unwrap();
        }
        // Groups are not leaves; only the three leaves count.
        assert_eq!(g.reachable_leaf_count(), 3);
    }

    #[test]
    fn reachable_leaf_count_is_zero_for_empty_graph() {
        let g = Scene::new();
        assert_eq!(g.reachable_leaf_count(), 0);
    }

    #[test]
    fn first_leaf_id_returns_pre_order_first_leaf() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let first_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        g.add_child(root, first_leaf).unwrap();
        let second_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        g.add_child(root, second_leaf).unwrap();
        assert_eq!(g.first_leaf_id(), Some(first_leaf));
    }

    #[test]
    fn first_leaf_id_is_none_for_empty_graph() {
        let g = Scene::new();
        assert_eq!(g.first_leaf_id(), None);
    }

    #[test]
    fn first_leaf_id_is_none_when_only_groups_reachable() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let child_group = g.add_group(None, Transform3D::Identity);
        g.add_child(root, child_group).unwrap();
        assert_eq!(g.first_leaf_id(), None);
    }

    #[test]
    fn install_root_leaf_at_freshly_allocated_id_succeeds() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let id = g.allocate_unbound_node_id();
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        g.install_root_leaf(id, rid, xform).unwrap();
        assert!(g.roots().contains(&id));
        let node = g.node(id).expect("installed node visible");
        assert_eq!(node.local_transform, xform);
        assert!(matches!(
            node.kind,
            NodeKind::Leaf(LeafPayload::MeshResource(r)) if r == rid
        ));
    }

    #[test]
    fn install_root_leaf_on_out_of_range_id_errors() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let bogus = NodeId(NonZeroU32::new(42).unwrap());
        let result = g.install_root_leaf(bogus, rid, Transform3D::Identity);
        assert_eq!(result, Err(SceneError::DanglingNodeId { id: bogus }));
        assert!(g.roots().is_empty());
    }

    #[test]
    fn install_root_leaf_on_occupied_slot_errors() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let occupied = g.add_group(None, Transform3D::Identity);
        let result = g.install_root_leaf(occupied, rid, Transform3D::Identity);
        assert_eq!(result, Err(SceneError::NodeSlotOccupied { id: occupied }));
        assert!(!g.roots().contains(&occupied));
    }

    #[test]
    fn install_root_leaf_with_dangling_mesh_resource_errors() {
        let mut g = Scene::new();
        let id = g.allocate_unbound_node_id();
        let bogus = MeshResourceId(NonZeroU32::new(99).unwrap());
        let result = g.install_root_leaf(id, bogus, Transform3D::Identity);
        assert_eq!(result, Err(SceneError::DanglingMeshResource { id: bogus }));
        assert!(g.node(id).is_none());
        assert!(g.roots().is_empty());
    }

    #[test]
    fn remove_root_leaf_returns_payload_and_clears_slot() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let id = g.allocate_unbound_node_id();
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([4.0, 5.0, 6.0]));
        g.install_root_leaf(id, rid, xform).unwrap();
        let (out_rid, out_xform) = g.remove_root_leaf(id).unwrap();
        assert_eq!(out_rid, rid);
        assert_eq!(out_xform, xform);
        assert!(g.node(id).is_none());
        assert!(!g.roots().contains(&id));
    }

    #[test]
    fn remove_root_leaf_on_empty_slot_errors() {
        let mut g = Scene::new();
        let id = g.allocate_unbound_node_id();
        let result = g.remove_root_leaf(id);
        assert_eq!(result, Err(SceneError::DanglingNodeId { id }));
    }

    #[test]
    fn remove_root_leaf_on_group_errors() {
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_root(group).unwrap();
        let result = g.remove_root_leaf(group);
        assert_eq!(result, Err(SceneError::NotALeaf { node_id: group }));
        assert!(g.roots().contains(&group));
    }

    #[test]
    fn remove_root_leaf_on_inline_mesh_leaf_errors() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let result = g.remove_root_leaf(leaf);
        assert_eq!(
            result,
            Err(SceneError::NotAMeshResourceLeaf { node_id: leaf })
        );
        assert!(g.roots().contains(&leaf));
        assert!(g.node(leaf).is_some());
    }

    #[test]
    fn remove_root_leaf_on_parented_leaf_errors() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let parent = g.add_group(None, Transform3D::Identity);
        g.add_root(parent).unwrap();
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        g.add_child(parent, leaf).unwrap();
        let result = g.remove_root_leaf(leaf);
        assert_eq!(result, Err(SceneError::NotARoot { node_id: leaf }));
        assert_eq!(g.parent_of(leaf), Some(parent));
    }

    #[test]
    fn install_remove_install_round_trips_at_same_id() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let id = g.allocate_unbound_node_id();
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([7.0, 8.0, 9.0]));
        g.install_root_leaf(id, rid, xform).unwrap();
        let snapshot_after_first = g.clone();
        let (out_rid, out_xform) = g.remove_root_leaf(id).unwrap();
        g.install_root_leaf(id, out_rid, out_xform).unwrap();
        assert_eq!(g, snapshot_after_first);
    }

    #[test]
    fn convert_inline_leaf_to_resource_returns_fresh_rid_and_rewrites_kind() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        assert_eq!(g.mesh_resources_len(), 0);
        let rid = g.convert_leaf_inline_to_mesh_resource(leaf).unwrap();
        assert_eq!(g.mesh_resources_len(), 1);
        let node = g.node(leaf).expect("leaf still lives");
        assert!(matches!(
            node.kind,
            NodeKind::Leaf(LeafPayload::MeshResource(r)) if r == rid
        ));
        assert_eq!(g.mesh_resource(rid).unwrap().vertices.len(), 3);
    }

    #[test]
    fn convert_inline_leaf_to_resource_preserves_meta_and_transform() {
        let mut g = Scene::new();
        let meta = SceneMetadata {
            name: Some("source".to_owned()),
            comment: None,
        };
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([2.0, 0.0, 0.0]));
        let leaf = g
            .add_leaf(
                Some(meta.clone()),
                xform,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        let _ = g.convert_leaf_inline_to_mesh_resource(leaf).unwrap();
        let node = g.node(leaf).unwrap();
        assert_eq!(node.meta.as_ref(), Some(&meta));
        assert_eq!(node.local_transform, xform);
    }

    #[test]
    fn convert_leaf_inline_to_resource_is_idempotent_for_resource_leaves() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        let resources_before = g.mesh_resources_len();
        let returned = g.convert_leaf_inline_to_mesh_resource(leaf).unwrap();
        assert_eq!(returned, rid);
        assert_eq!(g.mesh_resources_len(), resources_before);
        assert!(matches!(
            g.node(leaf).unwrap().kind,
            NodeKind::Leaf(LeafPayload::MeshResource(r)) if r == rid
        ));
    }

    #[test]
    fn convert_leaf_inline_to_resource_on_dangling_id_errors() {
        let mut g = Scene::new();
        let bogus = NodeId(NonZeroU32::new(99).unwrap());
        assert_eq!(
            g.convert_leaf_inline_to_mesh_resource(bogus),
            Err(SceneError::DanglingNodeId { id: bogus })
        );
        assert_eq!(g.mesh_resources_len(), 0);
    }

    #[test]
    fn convert_leaf_inline_to_resource_on_group_errors() {
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        assert_eq!(
            g.convert_leaf_inline_to_mesh_resource(group),
            Err(SceneError::NotALeaf { node_id: group })
        );
        assert!(matches!(
            g.node(group).unwrap().kind,
            NodeKind::Group { .. }
        ));
    }
}

#[cfg(test)]
mod scene_validate_tests {
    use super::*;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};

    fn unit_triangle() -> IndexedMesh {
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
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        g.add_child(root, leaf).unwrap();
        g.add_root(root).unwrap();
        assert_eq!(g.validate(), Ok(()));
    }

    #[test]
    fn validate_passes_on_subtree_instance_graph() {
        let mut g = Scene::new();
        let res_root = g.add_group(None, Transform3D::Identity);
        let res_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(unit_triangle()),
            )
            .unwrap();
        g.add_child(res_root, res_leaf).unwrap();
        let rid = g.register_subtree_resource(res_root).unwrap();
        let r1 = g.add_group(None, Transform3D::Identity);
        let r2 = g.add_group(None, Transform3D::Identity);
        let i1 = g.add_instance(None, Transform3D::Identity, rid).unwrap();
        let i2 = g.add_instance(None, Transform3D::Identity, rid).unwrap();
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
                LeafPayload::Mesh(unit_triangle()),
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
        // Forge: wipe parents[child], leaving the child in parent's
        // children list but with no recorded parent.
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
        g.add_instance(None, Transform3D::Identity, rid).unwrap();
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
    fn validate_detects_instance_cycle() {
        let mut g = Scene::new();
        let res_root = g.add_group(None, Transform3D::Identity);
        let rid = g.register_subtree_resource(res_root).unwrap();
        let inst = g.add_instance(None, Transform3D::Identity, rid).unwrap();
        // Forge: insert the instance as a child of res_root, bypassing
        // the cycle check that `add_child` performs.
        if let Some(node) = g.nodes[res_root.index()].as_mut()
            && let NodeKind::Group { children } = &mut node.kind
        {
            children.push(inst);
        }
        g.parents[inst.index()] = Some(res_root);
        assert!(matches!(
            g.validate(),
            Err(SceneError::InstanceCycle { .. })
        ));
    }
}

#[cfg(test)]
mod from_indexed_mesh_tests {
    use super::*;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};

    fn unit_triangle() -> IndexedMesh {
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

    #[test]
    fn from_indexed_mesh_produces_single_leaf_root() {
        let g = Scene::from(unit_triangle());
        assert_eq!(g.roots().len(), 1);
        assert_eq!(g.reachable_leaf_count(), 1);
        let id = g.first_leaf_id().unwrap();
        let node = g.node(id).unwrap();
        assert_eq!(node.local_transform, Transform3D::Identity);
        assert!(node.meta.is_none());
    }
}
