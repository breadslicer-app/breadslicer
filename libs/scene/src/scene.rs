/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Hierarchical [`Scene`] aggregate and its mutation / validation
//! API. Arena-backed; per-mesh AABB and bake helpers live in the
//! sibling [`sc_mesh_core::aabb`] / [`crate::bake`] modules.

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::sync::Arc;

use crate::error::SceneError;
use crate::node::{
    CsgOp, LeafAttachment, LeafPayload, MeshResourceId, Node, NodeId, NodeKind, SceneMetadata,
    SubtreeResourceId,
};
use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::transform::Transform3D;

/// Recover the [`NodeId`] for the slot at index `i` in the node
/// arena. Slots are append-only and never reused, so the id at
/// index `i` is always `NodeId(NonZeroU32::new(i + 1))`.
pub(crate) fn node_id_at_index(i: usize) -> NodeId {
    let raw = u32::try_from(i + 1).expect("node index overflow");
    NodeId(NonZeroU32::new(raw).expect("index + 1 is non-zero"))
}

/// Recover the [`MeshResourceId`] for the slot at index `i` in the
/// resource arena. Slots are append-only and never reused, so the
/// id at index `i` is always `MeshResourceId(NonZeroU32::new(i + 1))`.
fn mesh_resource_id_at_index(i: usize) -> MeshResourceId {
    let raw = u32::try_from(i + 1).expect("mesh resource index overflow");
    MeshResourceId(NonZeroU32::new(raw).expect("index + 1 is non-zero"))
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
    pub(crate) mesh_resources: Vec<Option<Arc<IndexedMesh>>>,
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
        self.mesh_resources.get(id.index())?.as_deref()
    }

    /// Borrow the `Arc<IndexedMesh>` itself for the addressed mesh
    /// resource, not the inner mesh. Used by the copy / paste path
    /// when it needs to dedupe by `Arc::as_ptr` identity or pipe the
    /// `Arc` straight into a new scene without deep-cloning.
    pub fn mesh_resource_arc(&self, id: MeshResourceId) -> Option<&Arc<IndexedMesh>> {
        self.mesh_resources.get(id.index())?.as_ref()
    }

    /// Mutable borrow into the addressed mesh resource. `None` on a
    /// dangling or tombstoned id. Uses [`Arc::make_mut`] semantics:
    /// if the `Arc` is shared (refcount > 1, e.g. an inline leaf or
    /// the clipboard also holds it), the call deep-clones the mesh
    /// into a fresh `Arc` first, leaving every other holder pointing
    /// at the original bytes.
    pub fn mesh_resource_mut(&mut self, id: MeshResourceId) -> Option<&mut IndexedMesh> {
        let slot = self.mesh_resources.get_mut(id.index())?.as_mut()?;
        Some(Arc::make_mut(slot))
    }

    /// Iterate every live mesh resource as `(id, &mesh)` pairs.
    /// Tombstoned slots are skipped.
    pub fn mesh_resources_iter(&self) -> impl Iterator<Item = (MeshResourceId, &IndexedMesh)> + '_ {
        self.mesh_resources
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                slot.as_deref()
                    .map(|mesh| (mesh_resource_id_at_index(i), mesh))
            })
    }

    /// Mutable variant of [`Self::mesh_resources_iter`]. Each yielded
    /// `&mut IndexedMesh` is obtained through [`Arc::make_mut`], so a
    /// shared mesh is forked into a unique `Arc` on first mutation.
    pub fn mesh_resources_iter_mut(
        &mut self,
    ) -> impl Iterator<Item = (MeshResourceId, &mut IndexedMesh)> + '_ {
        self.mesh_resources
            .iter_mut()
            .enumerate()
            .filter_map(|(i, slot)| {
                slot.as_mut()
                    .map(|arc| (mesh_resource_id_at_index(i), Arc::make_mut(arc)))
            })
    }

    /// Search the mesh-resource arena for a slot whose `Arc` shares
    /// pointer identity with `arc`. Returns the resource id of the
    /// match, or `None` if no live slot holds the same `Arc`. Used
    /// by the cross-scene paste path to dedupe a mesh that's already
    /// registered in the target scene.
    pub fn find_mesh_resource_by_arc(&self, arc: &Arc<IndexedMesh>) -> Option<MeshResourceId> {
        let target = Arc::as_ptr(arc);
        self.mesh_resources
            .iter()
            .enumerate()
            .find_map(|(i, slot)| {
                slot.as_ref().and_then(|stored| {
                    if Arc::as_ptr(stored) == target {
                        Some(mesh_resource_id_at_index(i))
                    } else {
                        None
                    }
                })
            })
    }

    /// Count of reachable leaves under `roots()`, traversed in
    /// pre-order. Mirrors what callers see if they enumerate via
    /// repeated `leaf_id_at_index(0..)` calls but does it in a
    /// single O(n) walk.
    ///
    /// `Reference` nodes are not descended: leaves inside referenced
    /// subtrees do not count. Contrast with [`Scene::merge_meshes`]
    /// and [`Scene::aabb`], which expand references.
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
                NodeKind::Reference { .. } => {}
            }
        }
        count
    }

    /// The first reachable leaf in `roots()` pre-order traversal, or
    /// `None` when the graph has no leaves. Format writers that walk
    /// the graph and have already verified `reachable_leaf_count() == 1`
    /// use this to grab the only leaf without re-walking via an index.
    ///
    /// `Reference` nodes are not descended, matching
    /// [`Scene::reachable_leaf_count`].
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
                NodeKind::Reference { .. } => {}
            }
        }
        None
    }

    /// The id of the first live `Reference` node in arena slot order,
    /// or `None` when the scene holds none. Scans the whole node
    /// arena, so references detached from `roots()` count too: the
    /// question answered is "does this scene use sharing at all",
    /// not "is a reference reachable". Intended for callers that do
    /// not support subtree references yet and refuse them at their own
    /// boundary; the returned id lets their error name the node.
    pub fn first_reference_node(&self) -> Option<NodeId> {
        self.nodes.iter().enumerate().find_map(|(i, slot)| {
            slot.as_ref().and_then(|node| match &node.kind {
                NodeKind::Reference { .. } => Some(node_id_at_index(i)),
                _ => None,
            })
        })
    }

    /// Mutable borrow into the inline mesh of a `Leaf(Mesh(_))` node.
    /// `None` for any other node kind (groups, CSG, references,
    /// mesh-resource-backed leaves) or a dangling id. Focused
    /// accessor: callers can repair the mesh in place without being
    /// handed `&mut Node` (which would let them change `kind` and
    /// break parent / child invariants).
    ///
    /// Uses [`Arc::make_mut`] semantics: if the inline `Arc` is
    /// shared with another holder (clipboard, mesh-resource arena,
    /// another inline leaf), the first mutation forks a fresh copy
    /// for this leaf so the other holders keep observing the
    /// pre-mutation bytes.
    pub fn inline_mesh_mut(&mut self, id: NodeId) -> Option<&mut IndexedMesh> {
        let node = self.nodes.get_mut(id.index())?.as_mut()?;
        match &mut node.kind {
            NodeKind::Leaf(LeafPayload::Mesh(arc)) => Some(Arc::make_mut(arc)),
            _ => None,
        }
    }

    /// Borrow the `Arc<IndexedMesh>` that backs the leaf at `id`,
    /// regardless of whether the leaf stores its mesh inline or by
    /// resource id. `None` for any non-leaf node, a dangling id, or a
    /// resource-backed leaf whose resource slot was tombstoned (which
    /// would be a graph invariant violation, not a routine case).
    /// Used by the copy primitive: the clipboard needs the live `Arc`
    /// itself, so a later same-scene paste can dedupe by
    /// `Arc::as_ptr` identity instead of registering a duplicate
    /// resource.
    pub fn leaf_mesh_arc(&self, id: NodeId) -> Option<&Arc<IndexedMesh>> {
        let node = self.nodes.get(id.index())?.as_ref()?;
        match &node.kind {
            NodeKind::Leaf(LeafPayload::Mesh(arc)) => Some(arc),
            NodeKind::Leaf(LeafPayload::MeshResource(rid)) => self.mesh_resource_arc(*rid),
            NodeKind::Group { .. } | NodeKind::Csg { .. } | NodeKind::Reference { .. } => None,
        }
    }

    pub fn subtree_resource_root(&self, id: SubtreeResourceId) -> Option<NodeId> {
        self.subtree_resources.get(id.index())?.as_ref().copied()
    }

    /// Whether any subtree resource is currently registered.
    /// Complements [`Scene::first_reference_node`] for callers whose
    /// phase contract excludes the whole sharing vocabulary (subtree resources and references), not
    /// just `Reference` nodes: a registered subtree without a
    /// reference to it still puts leaves outside every
    /// root-based walk.
    pub fn has_subtree_resources(&self) -> bool {
        self.subtree_resources.iter().any(Option::is_some)
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
    /// edit (see `sc_scene_edit`) that the undo/redo
    /// machinery replays against the same slot.
    ///
    /// The slot is permanently consumed: dropping the id without
    /// later installing a node at it leaks one arena slot. Slots are
    /// tombstoned but never reused, so this is benign at the scale of
    /// user-driven editing.
    pub fn allocate_unbound_node_id(&mut self) -> NodeId {
        self.allocate_node_slot()
    }

    /// Register `mesh` as a shared resource and return its handle.
    /// The mesh is wrapped in an `Arc` internally; callers that
    /// already hold an `Arc<IndexedMesh>` and want to share storage
    /// (typically the paste path) should use
    /// [`Self::add_mesh_resource_arc`] instead.
    pub fn add_mesh_resource(&mut self, mesh: IndexedMesh) -> MeshResourceId {
        self.add_mesh_resource_arc(Arc::new(mesh))
    }

    /// `Arc`-handoff variant of [`Self::add_mesh_resource`]. The arena stores
    /// the exact `Arc` the caller passed in (refcount bumped, no mesh-content
    /// clone), so a later [`Self::find_mesh_resource_by_arc`] with the same
    /// `Arc` returns the id allocated here.
    pub fn add_mesh_resource_arc(&mut self, arc: Arc<IndexedMesh>) -> MeshResourceId {
        let id = self.allocate_mesh_resource_slot();
        self.mesh_resources[id.index()] = Some(arc);
        id
    }

    /// Allocate a fresh [`MeshResourceId`] without populating its slot. The
    /// returned id's arena slot is `None`.
    ///
    /// Use this when the id needs to exist before the resource it will
    /// reference does, typically so the id can be embedded in a typed edit (see
    /// `sc_scene_edit`) that the undo/redo machinery replays
    /// against the same slot. Mirrors [`Self::allocate_unbound_node_id`] for
    /// mesh-resource ids.
    ///
    /// The slot is permanently consumed: dropping the id without later
    /// installing a resource at it leaks one arena slot. Slots are tombstoned
    /// but never reused, so this is benign at the scale of user-driven editing.
    pub fn allocate_unbound_mesh_resource_id(&mut self) -> MeshResourceId {
        self.allocate_mesh_resource_slot()
    }

    /// Install a mesh resource at a previously-allocated, currently-empty slot.
    /// Pairs with [`Self::remove_mesh_resource`] so a typed edit can round-trip
    /// a paste's mesh-resource creation through undo/redo while keeping the
    /// same [`MeshResourceId`].
    ///
    /// Errors:
    /// - [`SceneError::DanglingMeshResource`] if `rid` is past the arena's
    ///   high-water mark.
    /// - [`SceneError::MeshResourceSlotOccupied`] if a resource already lives
    ///   at that id.
    ///
    /// On error the scene is unchanged.
    pub fn install_mesh_resource_at(
        &mut self,
        rid: MeshResourceId,
        arc: Arc<IndexedMesh>,
    ) -> Result<(), SceneError> {
        if rid.index() >= self.mesh_resources.len() {
            return Err(SceneError::DanglingMeshResource { id: rid });
        }
        if self.mesh_resources[rid.index()].is_some() {
            return Err(SceneError::MeshResourceSlotOccupied { id: rid });
        }
        self.mesh_resources[rid.index()] = Some(arc);
        Ok(())
    }

    /// Tombstone the mesh resource at `rid`, returning the `Arc` that lived
    /// there so the paired [`Self::install_mesh_resource_at`] can re-install
    /// the same content.
    ///
    /// Errors:
    /// - [`SceneError::DanglingMeshResource`] if no live resource lives at
    ///   `rid`.
    /// - [`SceneError::MeshResourceInUse`] if at least one live
    ///   `Leaf(MeshResource(rid))` still references the resource. Callers must
    ///   remove every referencing leaf first (the compound paste-undo path
    ///   orders edits accordingly).
    ///
    /// On error the scene is unchanged.
    pub fn remove_mesh_resource(
        &mut self,
        rid: MeshResourceId,
    ) -> Result<Arc<IndexedMesh>, SceneError> {
        if self.mesh_resource(rid).is_none() {
            return Err(SceneError::DanglingMeshResource { id: rid });
        }
        if self.any_live_leaf_references(rid) {
            return Err(SceneError::MeshResourceInUse { id: rid });
        }
        let arc = self.mesh_resources[rid.index()]
            .take()
            .expect("checked Some above");
        Ok(arc)
    }

    /// Swap the mesh stored at the live resource slot `rid` for
    /// `mesh`, returning the `Arc` that lived there so a typed edit
    /// can snapshot it for undo.
    ///
    /// Errors:
    /// - [`SceneError::DanglingMeshResource`] if no live resource
    ///   lives at `rid`.
    ///
    /// On error the scene is unchanged.
    pub fn replace_mesh_resource(
        &mut self,
        rid: MeshResourceId,
        mesh: Arc<IndexedMesh>,
    ) -> Result<Arc<IndexedMesh>, SceneError> {
        let Some(slot) = self
            .mesh_resources
            .get_mut(rid.index())
            .and_then(Option::as_mut)
        else {
            return Err(SceneError::DanglingMeshResource { id: rid });
        };
        Ok(std::mem::replace(slot, mesh))
    }

    /// Swap the inline mesh of the `Leaf(Mesh(_))` node at `id` for
    /// `mesh`, returning the `Arc` that lived there so a typed edit
    /// can snapshot it for undo.
    ///
    /// Errors:
    /// - [`SceneError::DanglingNodeId`] if no live node lives at `id`.
    /// - [`SceneError::NotALeaf`] for Group, Csg, and Reference nodes.
    /// - [`SceneError::NotAnInlineMeshLeaf`] for a leaf that stores
    ///   its mesh by resource id.
    ///
    /// On error the scene is unchanged.
    pub fn replace_inline_leaf_mesh(
        &mut self,
        id: NodeId,
        mesh: Arc<IndexedMesh>,
    ) -> Result<Arc<IndexedMesh>, SceneError> {
        let Some(node) = self.nodes.get_mut(id.index()).and_then(Option::as_mut) else {
            return Err(SceneError::DanglingNodeId { id });
        };
        match &mut node.kind {
            NodeKind::Leaf(LeafPayload::Mesh(arc)) => Ok(std::mem::replace(arc, mesh)),
            NodeKind::Leaf(LeafPayload::MeshResource(_)) => {
                Err(SceneError::NotAnInlineMeshLeaf { node_id: id })
            }
            NodeKind::Group { .. } | NodeKind::Csg { .. } | NodeKind::Reference { .. } => {
                Err(SceneError::NotALeaf { node_id: id })
            }
        }
    }

    fn any_live_leaf_references(&self, rid: MeshResourceId) -> bool {
        self.live_leaf_references_resource_excluding(rid, None)
    }

    /// Returns true if some live leaf other than `exclude` carries a
    /// `LeafPayload::MeshResource(rid)`.
    ///
    /// Use it before removing the leaf at `exclude`:
    /// - False means the removal drops the last live reference, and the caller
    ///   should pair `RemoveRootLeaf` with `RemoveMeshResource`.
    /// - True means at least one sibling still holds the resource, so it has to
    ///   stay.
    ///
    /// A dangling `exclude` is harmless: the check still answers correctly for
    /// the live-leaf population.
    pub fn other_leaf_references_resource(&self, rid: MeshResourceId, exclude: NodeId) -> bool {
        self.live_leaf_references_resource_excluding(rid, Some(exclude))
    }

    fn live_leaf_references_resource_excluding(
        &self,
        rid: MeshResourceId,
        exclude: Option<NodeId>,
    ) -> bool {
        let exclude_idx = exclude.map(NodeId::index);
        self.nodes.iter().enumerate().any(|(idx, slot)| {
            Some(idx) != exclude_idx
                && matches!(
                    slot.as_ref().map(|n| &n.kind),
                    Some(NodeKind::Leaf(LeafPayload::MeshResource(r))) if *r == rid
                )
        })
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

    pub fn add_reference(
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
            kind: NodeKind::Reference { resource },
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
            return Err(SceneError::ExpansionCycle { node_id: cycle_id });
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
    /// `sc_scene_edit`) can round-trip a paste insertion
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

    /// Tombstone any leaf, returning the [`LeafAttachment`],
    /// [`LeafPayload`], and [`Transform3D`] needed to re-install it
    /// identically via [`Self::install_leaf_at`].
    ///
    /// Unlike [`Self::remove_root_leaf`] this works on:
    /// - inline-mesh and mesh-resource leaves alike, returning the
    ///   payload as-is;
    /// - root and parented leaves alike, with the attachment
    ///   recording which case applied and, for parented leaves,
    ///   the position in the parent's children list.
    ///
    /// Any [`SceneMetadata`] on the removed leaf is dropped.
    ///
    /// Errors:
    /// - [`SceneError::DanglingNodeId`] if no live node lives at
    ///   `id`.
    /// - [`SceneError::NotALeaf`] if the node is not a leaf.
    /// - [`SceneError::NotARoot`] if the leaf has no parent yet is
    ///   not in `roots()`, signalling a corrupted graph state.
    ///
    /// On error the scene is unchanged.
    pub fn remove_leaf(
        &mut self,
        id: NodeId,
    ) -> Result<(LeafAttachment, LeafPayload, Transform3D), SceneError> {
        let node = self.node(id).ok_or(SceneError::DanglingNodeId { id })?;
        if !matches!(node.kind, NodeKind::Leaf(_)) {
            return Err(SceneError::NotALeaf { node_id: id });
        }
        let attachment = if let Some(parent) = self.parents[id.index()] {
            let parent_node = self.nodes[parent.index()]
                .as_mut()
                .expect("parents[id] must point at a live parent by Scene invariants");
            let children = match &mut parent_node.kind {
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => children,
                NodeKind::Leaf(_) | NodeKind::Reference { .. } => unreachable!(
                    "Scene invariants forbid a Leaf or Reference from parenting a child"
                ),
            };
            let position = children
                .iter()
                .position(|c| *c == id)
                .expect("parent.children must list this child by Scene invariants");
            children.remove(position);
            self.parents[id.index()] = None;
            LeafAttachment::Parent { parent, position }
        } else {
            let root_pos = self
                .roots
                .iter()
                .position(|&r| r == id)
                .ok_or(SceneError::NotARoot { node_id: id })?;
            self.roots.remove(root_pos);
            LeafAttachment::Root
        };
        let removed = self.nodes[id.index()]
            .take()
            .expect("checked Some via node(id) above");
        let local_transform = removed.local_transform;
        let payload = match removed.kind {
            NodeKind::Leaf(payload) => payload,
            NodeKind::Group { .. } | NodeKind::Csg { .. } | NodeKind::Reference { .. } => {
                unreachable!("validated Leaf at top of remove_leaf")
            }
        };
        Ok((attachment, payload, local_transform))
    }

    /// Re-install a leaf at a previously-allocated `id`, with the
    /// given payload, attachment, and transform. The inverse of
    /// [`Self::remove_leaf`]: re-attaches the leaf at the same
    /// parent and the same position in the parent's children list
    /// (so the sibling order is preserved) or restores its root
    /// status, matching the attachment that the paired remove
    /// captured.
    ///
    /// `id` must come from [`Self::allocate_unbound_node_id`]. The
    /// installed leaf carries no [`SceneMetadata`]; callers that
    /// need to round-trip metadata must snapshot it before
    /// removing.
    ///
    /// Errors:
    /// - [`SceneError::DanglingNodeId`] if `id` is out of range, or
    ///   if the attachment is `Parent` and the parent node is gone.
    /// - [`SceneError::NodeSlotOccupied`] if the slot already holds
    ///   a node.
    /// - [`SceneError::DanglingMeshResource`] if the payload
    ///   references a missing mesh resource.
    /// - [`SceneError::NotABranch`] if the attachment is `Parent`
    ///   and the parent is a leaf or reference.
    ///
    /// On error the scene is unchanged. The position field of a
    /// `Parent` attachment is not bounds-checked here: paired use
    /// with the position recorded by [`Self::remove_leaf`] keeps it
    /// valid, but passing a position greater than the parent's
    /// current children count panics inside `Vec::insert`.
    pub fn install_leaf_at(
        &mut self,
        id: NodeId,
        attachment: LeafAttachment,
        payload: LeafPayload,
        local_transform: Transform3D,
    ) -> Result<(), SceneError> {
        if id.index() >= self.nodes.len() {
            return Err(SceneError::DanglingNodeId { id });
        }
        if self.nodes[id.index()].is_some() {
            return Err(SceneError::NodeSlotOccupied { id });
        }
        if let LeafPayload::MeshResource(rid) = payload
            && self.mesh_resource(rid).is_none()
        {
            return Err(SceneError::DanglingMeshResource { id: rid });
        }
        if let LeafAttachment::Parent { parent, .. } = attachment {
            let parent_node = self
                .node(parent)
                .ok_or(SceneError::DanglingNodeId { id: parent })?;
            match parent_node.kind {
                NodeKind::Group { .. } | NodeKind::Csg { .. } => {}
                NodeKind::Leaf(_) | NodeKind::Reference { .. } => {
                    return Err(SceneError::NotABranch { node_id: parent });
                }
            }
        }
        self.nodes[id.index()] = Some(Node {
            meta: None,
            local_transform,
            kind: NodeKind::Leaf(payload),
        });
        match attachment {
            LeafAttachment::Root => {
                self.roots.push(id);
            }
            LeafAttachment::Parent { parent, position } => {
                let parent_node = self.nodes[parent.index()]
                    .as_mut()
                    .expect("validated live above");
                let children = match &mut parent_node.kind {
                    NodeKind::Group { children } | NodeKind::Csg { children, .. } => children,
                    NodeKind::Leaf(_) | NodeKind::Reference { .. } => {
                        unreachable!("validated branch above")
                    }
                };
                children.insert(position, id);
                self.parents[id.index()] = Some(parent);
            }
        }
        Ok(())
    }
}

impl From<IndexedMesh> for Scene {
    /// Single-leaf graph carrying `mesh` under identity transform.
    /// Used by format readers whose on-disk model is a single mesh
    /// (STL) and by test fixtures that need a one-object scene.
    fn from(mesh: IndexedMesh) -> Self {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("freshly allocated graph has no resources to dangle");
        g.add_root(leaf)
            .expect("freshly allocated leaf has no parent and is not a root");
        g
    }
}

#[cfg(test)]
mod sharing_query_tests {
    use super::*;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use std::sync::Arc;

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

    fn registered_subtree(g: &mut Scene) -> SubtreeResourceId {
        let root = g.add_group(None, Transform3D::Identity);
        g.register_subtree_resource(root).unwrap()
    }

    #[test]
    fn empty_scene_reports_no_sharing() {
        let g = Scene::new();
        assert_eq!(g.first_reference_node(), None);
        assert!(!g.has_subtree_resources());
    }

    #[test]
    fn scene_without_references_reports_none_across_node_kinds() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_child(group, leaf).unwrap();
        g.add_root(group).unwrap();
        let csg_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![csg_leaf])
            .unwrap();
        g.add_root(csg).unwrap();
        assert_eq!(g.first_reference_node(), None);
        assert!(!g.has_subtree_resources());
    }

    #[test]
    fn parented_reference_is_found_by_id() {
        let mut g = Scene::new();
        let rid = registered_subtree(&mut g);
        let root = g.add_group(None, Transform3D::Identity);
        let reference = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        g.add_child(root, reference).unwrap();
        g.add_root(root).unwrap();
        // Two group nodes occupy earlier arena slots; the scan must
        // filter by kind, not return the first live node.
        assert_eq!(g.first_reference_node(), Some(reference));
    }

    #[test]
    fn detached_reference_still_counts() {
        let mut g = Scene::new();
        let rid = registered_subtree(&mut g);
        let reference = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        // Never parented, never a root: the arena scan still sees it.
        assert_eq!(g.first_reference_node(), Some(reference));
    }

    #[test]
    fn earliest_slot_wins_with_two_references() {
        let mut g = Scene::new();
        let rid = registered_subtree(&mut g);
        let first = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        let second = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        assert!(first.get() < second.get());
        assert_eq!(g.first_reference_node(), Some(first));
    }

    #[test]
    fn subtree_registration_flips_the_flag_without_references() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        assert!(!g.has_subtree_resources());
        g.register_subtree_resource(root).unwrap();
        assert!(g.has_subtree_resources());
        assert_eq!(g.first_reference_node(), None);
    }
}

#[cfg(test)]
mod scene_construction_tests {
    use super::*;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::transform::AffineTransform3D;

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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        let child = g.add_group(None, Transform3D::Identity);
        let result = g.add_child(parent, child);
        assert_eq!(result, Err(SceneError::NotABranch { node_id: parent }));
    }

    #[test]
    fn add_child_under_reference_errors() {
        let mut g = Scene::new();
        let res_root = g.add_group(None, Transform3D::Identity);
        let rid = g.register_subtree_resource(res_root).unwrap();
        let parent = g.add_reference(None, Transform3D::Identity, rid).unwrap();
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
    fn add_child_direct_reference_cycle_rejected() {
        let mut g = Scene::new();
        let res_root = g.add_group(None, Transform3D::Identity);
        let rid = g.register_subtree_resource(res_root).unwrap();
        let reference = g.add_reference(None, Transform3D::Identity, rid).unwrap();
        // Try to put the Reference inside its own subtree.
        let result = g.add_child(res_root, reference);
        assert!(matches!(result, Err(SceneError::ExpansionCycle { .. })));
        // Rollback worked: reference still parent-less, res_root has no children.
        assert_eq!(g.parent_of(reference), None);
        assert!(matches!(
            &g.node(res_root).unwrap().kind,
            NodeKind::Group { children } if children.is_empty()
        ));
    }

    #[test]
    fn add_child_indirect_reference_cycle_rejected() {
        // S1 root contains a reference to S2; S2 root would then need
        // a reference to S1 to close the loop.
        let mut g = Scene::new();
        let s1_root = g.add_group(None, Transform3D::Identity);
        let s2_root = g.add_group(None, Transform3D::Identity);
        let s1_id = g.register_subtree_resource(s1_root).unwrap();
        let s2_id = g.register_subtree_resource(s2_root).unwrap();
        let inst_to_s2 = g.add_reference(None, Transform3D::Identity, s2_id).unwrap();
        g.add_child(s1_root, inst_to_s2).unwrap();
        let inst_to_s1 = g.add_reference(None, Transform3D::Identity, s1_id).unwrap();
        let result = g.add_child(s2_root, inst_to_s1);
        assert!(matches!(result, Err(SceneError::ExpansionCycle { .. })));
    }

    #[test]
    fn add_child_children_only_cycle_rejected() {
        // Floating subtree a -> b (a is never registered as a scene
        // root); closing b -> a would form a children-only cycle.
        let mut g = Scene::new();
        let a = g.add_group(None, Transform3D::Identity);
        let b = g.add_group(None, Transform3D::Identity);
        g.add_child(a, b).unwrap();
        let result = g.add_child(b, a);
        assert!(matches!(result, Err(SceneError::ExpansionCycle { .. })));
        // Rollback worked: a still parent-less, b has no children.
        assert_eq!(g.parent_of(a), None);
        assert!(matches!(
            &g.node(b).unwrap().kind,
            NodeKind::Group { children } if children.is_empty()
        ));
    }

    #[test]
    fn add_child_self_loop_rejected() {
        let mut g = Scene::new();
        let c = g.add_group(None, Transform3D::Identity);
        let result = g.add_child(c, c);
        assert!(matches!(result, Err(SceneError::ExpansionCycle { .. })));
        // Rollback worked: c still parent-less and childless.
        assert_eq!(g.parent_of(c), None);
        assert!(matches!(
            &g.node(c).unwrap().kind,
            NodeKind::Group { children } if children.is_empty()
        ));
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
    fn add_reference_with_dangling_resource_errors() {
        let mut g = Scene::new();
        let bogus = SubtreeResourceId(NonZeroU32::new(7).unwrap());
        let result = g.add_reference(None, Transform3D::Identity, bogus);
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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
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
    fn leaf_mesh_arc_returns_the_inline_payload_arc() {
        let mut g = Scene::new();
        let arc = Arc::new(unit_triangle());
        let id = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(arc.clone()))
            .unwrap();
        let got = g.leaf_mesh_arc(id).expect("inline-mesh leaf is reachable");
        assert!(std::ptr::eq(Arc::as_ptr(&arc), Arc::as_ptr(got)));
    }

    #[test]
    fn leaf_mesh_arc_follows_resource_indirection() {
        let mut g = Scene::new();
        let arc = Arc::new(unit_triangle());
        let rid = g.add_mesh_resource_arc(arc.clone());
        let id = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        let got = g
            .leaf_mesh_arc(id)
            .expect("resource-backed leaf resolves to the resource arc");
        assert!(std::ptr::eq(Arc::as_ptr(&arc), Arc::as_ptr(got)));
    }

    #[test]
    fn leaf_mesh_arc_returns_none_for_group() {
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        assert!(g.leaf_mesh_arc(group).is_none());
    }

    #[test]
    fn leaf_mesh_arc_returns_none_on_dangling_id() {
        let g = Scene::new();
        let bogus = NodeId(NonZeroU32::new(99).unwrap());
        assert!(g.leaf_mesh_arc(bogus).is_none());
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
                    LeafPayload::Mesh(Arc::new(unit_triangle())),
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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_child(root, first_leaf).unwrap();
        let second_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
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
                LeafPayload::Mesh(Arc::new(unit_triangle())),
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
    fn allocate_unbound_mesh_resource_id_leaves_slot_empty() {
        let mut g = Scene::new();
        let rid = g.allocate_unbound_mesh_resource_id();
        assert!(g.mesh_resource(rid).is_none());
        assert_eq!(g.mesh_resources_len(), 1);
    }

    #[test]
    fn install_mesh_resource_at_freshly_allocated_id_succeeds() {
        let mut g = Scene::new();
        let rid = g.allocate_unbound_mesh_resource_id();
        let arc = Arc::new(unit_triangle());
        g.install_mesh_resource_at(rid, Arc::clone(&arc)).unwrap();
        assert_eq!(g.mesh_resource(rid).unwrap().vertices.len(), 3);
        assert!(std::ptr::eq(
            Arc::as_ptr(&arc),
            Arc::as_ptr(g.mesh_resource_arc(rid).unwrap())
        ));
    }

    #[test]
    fn install_mesh_resource_at_out_of_range_id_errors() {
        let mut g = Scene::new();
        let bogus = MeshResourceId(NonZeroU32::new(99).unwrap());
        let arc = Arc::new(unit_triangle());
        assert_eq!(
            g.install_mesh_resource_at(bogus, arc),
            Err(SceneError::DanglingMeshResource { id: bogus })
        );
    }

    #[test]
    fn install_mesh_resource_at_occupied_slot_errors() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let arc = Arc::new(unit_triangle());
        assert_eq!(
            g.install_mesh_resource_at(rid, arc),
            Err(SceneError::MeshResourceSlotOccupied { id: rid })
        );
    }

    #[test]
    fn remove_mesh_resource_returns_arc_and_clears_slot() {
        let mut g = Scene::new();
        let arc = Arc::new(unit_triangle());
        let rid = g.add_mesh_resource_arc(Arc::clone(&arc));
        let removed = g.remove_mesh_resource(rid).unwrap();
        assert!(std::ptr::eq(Arc::as_ptr(&arc), Arc::as_ptr(&removed)));
        assert!(g.mesh_resource(rid).is_none());
    }

    #[test]
    fn remove_mesh_resource_on_dangling_id_errors() {
        let mut g = Scene::new();
        let bogus = MeshResourceId(NonZeroU32::new(99).unwrap());
        assert_eq!(
            g.remove_mesh_resource(bogus),
            Err(SceneError::DanglingMeshResource { id: bogus })
        );
    }

    #[test]
    fn remove_mesh_resource_with_live_referencing_leaf_errors() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let _leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        assert_eq!(
            g.remove_mesh_resource(rid),
            Err(SceneError::MeshResourceInUse { id: rid })
        );
        assert!(g.mesh_resource(rid).is_some());
    }

    #[test]
    fn replace_mesh_resource_swaps_the_arc_and_returns_the_previous_one() {
        let mut g = Scene::new();
        let before = Arc::new(unit_triangle());
        let rid = g.add_mesh_resource_arc(Arc::clone(&before));
        let after = Arc::new(unit_triangle());
        let returned = g.replace_mesh_resource(rid, Arc::clone(&after)).unwrap();
        assert!(Arc::ptr_eq(&returned, &before));
        assert!(Arc::ptr_eq(g.mesh_resource_arc(rid).unwrap(), &after));
    }

    #[test]
    fn replace_mesh_resource_on_dangling_id_errors() {
        let mut g = Scene::new();
        let bogus = MeshResourceId(NonZeroU32::new(99).unwrap());
        assert_eq!(
            g.replace_mesh_resource(bogus, Arc::new(unit_triangle())),
            Err(SceneError::DanglingMeshResource { id: bogus })
        );
    }

    #[test]
    fn replace_inline_leaf_mesh_swaps_the_arc_and_returns_the_previous_one() {
        let mut g = Scene::new();
        let before = Arc::new(unit_triangle());
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::clone(&before)),
            )
            .unwrap();
        let after = Arc::new(unit_triangle());
        let returned = g
            .replace_inline_leaf_mesh(leaf, Arc::clone(&after))
            .unwrap();
        assert!(Arc::ptr_eq(&returned, &before));
        assert!(Arc::ptr_eq(g.leaf_mesh_arc(leaf).unwrap(), &after));
    }

    #[test]
    fn replace_inline_leaf_mesh_rejects_every_non_inline_target_unchanged() {
        let mut g = Scene::new();
        let dangling = NodeId(NonZeroU32::new(99).unwrap());
        assert_eq!(
            g.replace_inline_leaf_mesh(dangling, Arc::new(unit_triangle())),
            Err(SceneError::DanglingNodeId { id: dangling })
        );
        let group = g.add_group(None, Transform3D::Identity);
        assert_eq!(
            g.replace_inline_leaf_mesh(group, Arc::new(unit_triangle())),
            Err(SceneError::NotALeaf { node_id: group })
        );
        let shared = Arc::new(unit_triangle());
        let rid = g.add_mesh_resource_arc(Arc::clone(&shared));
        let backed = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        assert_eq!(
            g.replace_inline_leaf_mesh(backed, Arc::new(unit_triangle())),
            Err(SceneError::NotAnInlineMeshLeaf { node_id: backed })
        );
        // The refusals left the resource-backed storage untouched.
        assert!(Arc::ptr_eq(g.mesh_resource_arc(rid).unwrap(), &shared));
    }

    #[test]
    fn other_leaf_references_resource_is_false_when_exclude_is_the_only_reference() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        assert!(!g.other_leaf_references_resource(rid, leaf));
    }

    #[test]
    fn other_leaf_references_resource_is_true_when_a_sibling_leaf_holds_the_same_rid() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let leaf_a = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        let leaf_b = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        assert!(g.other_leaf_references_resource(rid, leaf_a));
        assert!(g.other_leaf_references_resource(rid, leaf_b));
    }

    #[test]
    fn other_leaf_references_resource_ignores_inline_mesh_leaves() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let resource_leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        let _inline_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        assert!(!g.other_leaf_references_resource(rid, resource_leaf));
    }

    #[test]
    fn other_leaf_references_resource_ignores_non_leaf_nodes() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        let _group = g.add_group(None, Transform3D::Identity);
        let _csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, Vec::new())
            .unwrap();
        assert!(!g.other_leaf_references_resource(rid, leaf));
    }

    #[test]
    fn other_leaf_references_resource_with_dangling_exclude_still_reports_live_references() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let _leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        let bogus = NodeId(NonZeroU32::new(9999).unwrap());
        assert!(g.other_leaf_references_resource(rid, bogus));
    }

    #[test]
    fn remove_leaf_on_root_inline_mesh_leaf_returns_root_attachment_and_payload() {
        let mut g = Scene::new();
        let arc = Arc::new(unit_triangle());
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::clone(&arc)),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let (attachment, payload, transform) = g.remove_leaf(leaf).unwrap();
        assert_eq!(attachment, LeafAttachment::Root);
        match payload {
            LeafPayload::Mesh(returned) => {
                assert!(std::ptr::eq(Arc::as_ptr(&arc), Arc::as_ptr(&returned)));
            }
            LeafPayload::MeshResource(_) => panic!("expected inline mesh payload"),
        }
        assert_eq!(transform, Transform3D::Identity);
        assert!(g.node(leaf).is_none());
        assert!(!g.roots().contains(&leaf));
    }

    #[test]
    fn remove_leaf_on_root_mesh_resource_leaf_returns_root_attachment_and_rid() {
        let mut g = Scene::new();
        let rid = g.add_mesh_resource(unit_triangle());
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        g.add_root(leaf).unwrap();
        let (attachment, payload, _) = g.remove_leaf(leaf).unwrap();
        assert_eq!(attachment, LeafAttachment::Root);
        assert!(matches!(payload, LeafPayload::MeshResource(r) if r == rid));
    }

    #[test]
    fn remove_leaf_on_parented_leaf_returns_parent_attachment_with_position() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let arc = Arc::new(unit_triangle());
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(arc))
            .unwrap();
        g.add_child(root, leaf).unwrap();
        let (attachment, payload, _) = g.remove_leaf(leaf).unwrap();
        assert_eq!(
            attachment,
            LeafAttachment::Parent {
                parent: root,
                position: 0,
            }
        );
        assert!(matches!(payload, LeafPayload::Mesh(_)));
        // Detached from parent: children list is empty, parent slot
        // is empty for the removed node.
        let root_node = g.node(root).unwrap();
        assert_eq!(root_node.kind.children(), &[] as &[NodeId]);
        assert!(g.parent_of(leaf).is_none());
    }

    #[test]
    fn remove_leaf_preserves_sibling_position_for_round_trip() {
        // Three siblings under a Group root: removing the middle
        // one reports position=1 and the remaining children list
        // is [a, c].
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let a = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_child(root, a).unwrap();
        let b = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_child(root, b).unwrap();
        let c = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_child(root, c).unwrap();
        let (attachment, _, _) = g.remove_leaf(b).unwrap();
        assert_eq!(
            attachment,
            LeafAttachment::Parent {
                parent: root,
                position: 1,
            }
        );
        let root_node = g.node(root).unwrap();
        assert_eq!(root_node.kind.children(), &[a, c]);
    }

    #[test]
    fn remove_leaf_on_non_leaf_node_errors_not_a_leaf_and_leaves_scene_unchanged() {
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_root(group).unwrap();
        let before = g.clone();
        assert_eq!(
            g.remove_leaf(group),
            Err(SceneError::NotALeaf { node_id: group })
        );
        assert_eq!(g, before);
    }

    #[test]
    fn remove_leaf_on_dangling_id_errors_dangling_node_id() {
        let mut g = Scene::new();
        let bogus = NodeId(NonZeroU32::new(9999).unwrap());
        assert_eq!(
            g.remove_leaf(bogus),
            Err(SceneError::DanglingNodeId { id: bogus })
        );
    }

    #[test]
    fn install_leaf_at_round_trips_a_parented_leaf() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_child(root, leaf).unwrap();
        let snapshot = g.clone();
        let (attachment, payload, transform) = g.remove_leaf(leaf).unwrap();
        g.install_leaf_at(leaf, attachment, payload, transform)
            .unwrap();
        assert_eq!(g, snapshot);
    }

    #[test]
    fn install_leaf_at_round_trips_a_root_leaf() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let snapshot = g.clone();
        let (attachment, payload, transform) = g.remove_leaf(leaf).unwrap();
        g.install_leaf_at(leaf, attachment, payload, transform)
            .unwrap();
        assert_eq!(g, snapshot);
    }

    #[test]
    fn install_leaf_at_on_occupied_slot_errors_node_slot_occupied() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let before = g.clone();
        assert_eq!(
            g.install_leaf_at(
                leaf,
                LeafAttachment::Root,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
                Transform3D::Identity,
            ),
            Err(SceneError::NodeSlotOccupied { id: leaf })
        );
        assert_eq!(g, before);
    }

    #[test]
    fn install_leaf_at_with_dangling_rid_errors_dangling_mesh_resource() {
        let mut g = Scene::new();
        let id = g.allocate_unbound_node_id();
        let bogus_rid = MeshResourceId(NonZeroU32::new(9999).unwrap());
        let before = g.clone();
        assert_eq!(
            g.install_leaf_at(
                id,
                LeafAttachment::Root,
                LeafPayload::MeshResource(bogus_rid),
                Transform3D::Identity,
            ),
            Err(SceneError::DanglingMeshResource { id: bogus_rid })
        );
        assert_eq!(g, before);
    }

    #[test]
    fn install_leaf_at_with_leaf_parent_errors_not_a_branch() {
        let mut g = Scene::new();
        let leaf_parent = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .unwrap();
        g.add_root(leaf_parent).unwrap();
        let id = g.allocate_unbound_node_id();
        let before = g.clone();
        assert_eq!(
            g.install_leaf_at(
                id,
                LeafAttachment::Parent {
                    parent: leaf_parent,
                    position: 0,
                },
                LeafPayload::Mesh(Arc::new(unit_triangle())),
                Transform3D::Identity,
            ),
            Err(SceneError::NotABranch {
                node_id: leaf_parent
            })
        );
        assert_eq!(g, before);
    }

    #[test]
    fn install_leaf_at_with_dangling_parent_errors_dangling_node_id() {
        let mut g = Scene::new();
        let id = g.allocate_unbound_node_id();
        let bogus_parent = NodeId(NonZeroU32::new(9999).unwrap());
        let before = g.clone();
        assert_eq!(
            g.install_leaf_at(
                id,
                LeafAttachment::Parent {
                    parent: bogus_parent,
                    position: 0,
                },
                LeafPayload::Mesh(Arc::new(unit_triangle())),
                Transform3D::Identity,
            ),
            Err(SceneError::DanglingNodeId { id: bogus_parent })
        );
        assert_eq!(g, before);
    }

    #[test]
    fn allocate_install_remove_install_round_trips_mesh_resource_at_same_id() {
        let mut g = Scene::new();
        let arc = Arc::new(unit_triangle());
        let rid = g.allocate_unbound_mesh_resource_id();
        g.install_mesh_resource_at(rid, Arc::clone(&arc)).unwrap();
        let snapshot_after_first = g.clone();
        let removed = g.remove_mesh_resource(rid).unwrap();
        g.install_mesh_resource_at(rid, removed).unwrap();
        assert_eq!(g, snapshot_after_first);
    }
}

#[cfg(test)]
mod from_indexed_mesh_tests {
    use super::*;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};

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
