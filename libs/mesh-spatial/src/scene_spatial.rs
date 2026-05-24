/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-level spatial layer. Owns a [`Scene`] and the per-mesh
//! caches it needs.
//!
//! Co-ownership is the load-bearing design choice. A [`SceneSpatial`]
//! holds the [`Scene`] by value; readers obtain an
//! [`ObjectQuery<'_>`] handle via [`SceneSpatial::leaf`] (by stable
//! [`NodeId`]). The query bundles the leaf node, the resolved mesh
//! and the spatial cache that belongs to that mesh; mismatched
//! pairings are structurally impossible.

use sc_mesh_core::scene::edit::SceneEdit;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{
    Aabb, IndexedMesh, LeafPayload, MeshResourceId, Node, NodeId, NodeKind, Scene, Vertex,
};

use crate::clusters::TriangleClusters;
use crate::mesh_spatial::MeshSpatial;

/// Scene plus per-mesh spatial caches. The cache crate's main
/// public type. Callers in the GUI swap their `Scene` storage
/// for this when they need cached spatial queries; headless callers
/// (`mesh-formats`, `slicer-cli`) never construct one.
///
/// Built either lazily ([`SceneSpatial::from_graph`]) or eagerly
/// ([`SceneSpatial::primed_graph`]). The query API is identical for
/// both; the only difference is when cache priming work runs.
pub struct SceneSpatial {
    graph: Scene,
    /// Per-leaf spatial caches. Indexed by `NodeId::index()`. The
    /// vector is sized to [`Scene::nodes_len`] so any live
    /// `NodeId` is always in bounds. `Some(_)` only for nodes whose
    /// kind is `Leaf(LeafPayload::Mesh(_))`; `None` for groups, CSG
    /// nodes, instances, mesh-resource-backed leaves (whose cache
    /// lives in `mesh_resources` below) and tombstoned slots.
    leaves: Vec<Option<MeshSpatial>>,
    /// Per-mesh-resource spatial caches. Indexed by
    /// `MeshResourceId::index()`, sized to
    /// [`Scene::mesh_resources_len`].
    mesh_resources: Vec<MeshSpatial>,
}

impl SceneSpatial {
    /// Wrap a [`Scene`] without priming any caches. First query
    /// on each mesh will build the relevant cache lazily.
    pub fn from_graph(graph: Scene) -> Self {
        Self::build(graph)
    }

    /// Wrap a [`Scene`] and eagerly populate every per-mesh
    /// cache reachable from the scene roots. `Send`-safe; intended
    /// to run on the worker thread that already hosts parsing and
    /// GPU upload.
    ///
    /// Mesh-resource caches are primed via the leaves that reach
    /// them. A resource referenced by N leaves is primed N times;
    /// `MeshSpatial::prime` is idempotent, so the extra work is
    /// duplicated cache lookups, not duplicated computation.
    pub fn primed_graph(graph: Scene) -> Self {
        let s = Self::from_graph(graph);
        s.prime_reachable_caches();
        s
    }

    /// Borrow the underlying graph for read-only access.
    pub fn graph(&self) -> &Scene {
        &self.graph
    }

    /// Consume the spatial and return the inner graph, discarding
    /// any cached state.
    pub fn into_graph(self) -> Scene {
        self.graph
    }

    /// Allocate a fresh [`NodeId`] without installing any node, and
    /// extend the per-leaf cache vector to keep
    /// `self.leaves.len() == self.graph.nodes_len()`. The new cache
    /// slot stays `None`: only inline-mesh leaves carry an entry
    /// here, and a node installed at an unbound id through a typed
    /// scene edit is always a mesh-resource leaf.
    ///
    /// See [`Scene::allocate_unbound_node_id`] for the underlying
    /// arena contract and its one-slot leak warning.
    pub fn allocate_unbound_node_id(&mut self) -> NodeId {
        let id = self.graph.allocate_unbound_node_id();
        self.leaves.push(None);
        id
    }

    /// Look up a leaf by stable [`NodeId`]. Returns `None` when:
    ///
    /// * the id is dangling or names a tombstone,
    /// * the addressed node is not a `Leaf` (groups, CSG and
    ///   instances do not carry geometry of their own),
    /// * the leaf payload is a `MeshResource` whose resource is
    ///   dangling.
    ///
    /// The returned [`ObjectQuery`] bundles the node, the resolved
    /// mesh and the spatial cache that belongs to that mesh, plus
    /// the leaf's world transform composed from its ancestor chain
    /// at query construction time.
    pub fn leaf(&self, id: NodeId) -> Option<ObjectQuery<'_>> {
        let node = self.graph.node(id)?;
        let NodeKind::Leaf(payload) = &node.kind else {
            return None;
        };
        let (mesh, cache) = match payload {
            LeafPayload::Mesh(mesh) => {
                let cache = self.leaves.get(id.index())?.as_ref()?;
                (mesh.as_ref(), cache)
            }
            LeafPayload::MeshResource(rid) => {
                let mesh = self.graph.mesh_resource(*rid)?;
                let cache = self.mesh_resources.get(rid.index())?;
                (mesh, cache)
            }
        };
        let world_transform = compose_world_transform(&self.graph, id);
        let parent_world = match self.graph.parent_of(id) {
            Some(parent) => compose_world_transform(&self.graph, parent),
            None => Transform3D::Identity,
        };
        Some(ObjectQuery {
            node,
            mesh,
            cache,
            world_transform,
            parent_world,
        })
    }

    /// Apply a typed scene edit and update caches accordingly.
    /// `SetLocalTransform` leaves the per-mesh caches alone (they
    /// are object-space: local AABB, clusters). `AddRootLeaf` /
    /// `RemoveRootLeaf` operate only on mesh-resource-backed leaves,
    /// whose caches live in `mesh_resources` (keyed by
    /// [`MeshResourceId`]) and stay valid across the leaf's
    /// installation / removal. `AddMeshResource` /
    /// `RemoveMeshResource` install / tombstone the underlying
    /// resource at a previously-allocated slot; the per-resource
    /// cache slot is already sized to match because it was extended
    /// at allocation time. `Compound` recurses through the inner
    /// edits in order so each one observes the side-effects of the
    /// previous ones.
    pub fn apply_edit(&mut self, edit: SceneEdit) {
        match edit {
            SceneEdit::SetLocalTransform { node_id, after, .. } => {
                let _ = self.graph.set_local_transform(node_id, after);
            }
            SceneEdit::AddRootLeaf {
                node_id,
                mesh,
                local_transform,
            } => {
                let _ = self.graph.install_root_leaf(node_id, mesh, local_transform);
            }
            SceneEdit::RemoveRootLeaf { node_id, .. } => {
                let _ = self.graph.remove_root_leaf(node_id);
            }
            SceneEdit::AddMeshResource { rid, mesh } => {
                let _ = self.graph.install_mesh_resource_at(rid, mesh);
            }
            SceneEdit::RemoveMeshResource { rid, .. } => {
                let _ = self.graph.remove_mesh_resource(rid);
            }
            SceneEdit::Compound(edits) => {
                for edit in edits {
                    self.apply_edit(edit);
                }
            }
        }
    }

    /// Allocate a fresh [`MeshResourceId`] without populating its
    /// slot, and extend the per-resource cache vector to keep
    /// `self.mesh_resources.len() == self.graph.mesh_resources_len()`.
    /// Mirrors [`Self::allocate_unbound_node_id`] for the
    /// mesh-resource arena.
    pub fn allocate_unbound_mesh_resource_id(&mut self) -> MeshResourceId {
        let rid = self.graph.allocate_unbound_mesh_resource_id();
        self.mesh_resources.push(MeshSpatial::default());
        rid
    }

    /// Clear cached state for the inline mesh on the leaf at `id`.
    /// No-op when `id` is dangling, names a non-leaf, or names a
    /// resource-backed leaf (mesh-resource caches live in their own
    /// pool; see [`Self::invalidate_mesh_resource`]).
    pub fn invalidate_leaf(&mut self, id: NodeId) {
        if let Some(slot) = self.leaves.get_mut(id.index())
            && slot.is_some()
        {
            *slot = Some(MeshSpatial::default());
        }
    }

    /// Clear cached state for the mesh resource at `id`. No-op on a
    /// dangling id.
    pub fn invalidate_mesh_resource(&mut self, id: MeshResourceId) {
        if let Some(cache) = self.mesh_resources.get_mut(id.index()) {
            *cache = MeshSpatial::default();
        }
    }

    /// Resolve a zero-based pre-order leaf index against the scene
    /// roots, returning the addressed leaf's stable [`NodeId`]. Used
    /// by the slicer-gui boundary code that translates a Slint `i32`
    /// row index (sidebar click, Tab/Shift+Tab cycle) into a
    /// [`NodeId`] before storing it on the selection.
    pub fn leaf_id_at_index(&self, target: usize) -> Option<NodeId> {
        let mut count = 0usize;
        let mut stack: Vec<NodeId> = self.graph.roots().iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = self.graph.node(id) else {
                continue;
            };
            match &node.kind {
                NodeKind::Leaf(_) => {
                    if count == target {
                        return Some(id);
                    }
                    count += 1;
                }
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                    for child in children.iter().rev() {
                        stack.push(*child);
                    }
                }
                NodeKind::Instance { .. } => {
                    // Phase contract: production code does not
                    // construct instances at this stage of the
                    // graph migration. When instance expansion
                    // lands, ranking inside an instance subtree
                    // gets its own resolver.
                }
            }
        }
        None
    }

    /// Inverse of [`Self::leaf_id_at_index`]: walk the scene roots
    /// in pre-order and return the leaf-index of `target`, or `None`
    /// when the node is not a reachable leaf. Used by the slicer-gui
    /// boundary code that resolves a [`NodeId`] back to a Slint
    /// `i32` row index (the `selected_object_index` mirror and the
    /// Tab/Shift+Tab cycle helpers).
    pub fn leaf_index_of(&self, target: NodeId) -> Option<usize> {
        let mut count = 0usize;
        let mut stack: Vec<NodeId> = self.graph.roots().iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = self.graph.node(id) else {
                continue;
            };
            match &node.kind {
                NodeKind::Leaf(_) => {
                    if id == target {
                        return Some(count);
                    }
                    count += 1;
                }
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

    /// Inner constructor shared by all entry points.
    fn build(graph: Scene) -> Self {
        let mut leaves: Vec<Option<MeshSpatial>> = (0..graph.nodes_len()).map(|_| None).collect();
        let mesh_resources: Vec<MeshSpatial> = (0..graph.mesh_resources_len())
            .map(|_| MeshSpatial::default())
            .collect();
        Self::allocate_inline_cache_slots(&graph, &mut leaves);
        Self {
            graph,
            leaves,
            mesh_resources,
        }
    }

    fn allocate_inline_cache_slots(graph: &Scene, leaves: &mut [Option<MeshSpatial>]) {
        for_each_reachable_node(graph, |id, node| {
            if let NodeKind::Leaf(LeafPayload::Mesh(_)) = &node.kind {
                leaves[id.index()] = Some(MeshSpatial::default());
            }
        });
    }

    fn prime_reachable_caches(&self) {
        for_each_reachable_node(&self.graph, |id, node| match &node.kind {
            NodeKind::Leaf(LeafPayload::Mesh(mesh)) => {
                if let Some(Some(cache)) = self.leaves.get(id.index()) {
                    cache.prime(mesh);
                }
            }
            NodeKind::Leaf(LeafPayload::MeshResource(rid)) => {
                if let (Some(mesh), Some(cache)) = (
                    self.graph.mesh_resource(*rid),
                    self.mesh_resources.get(rid.index()),
                ) {
                    cache.prime(mesh);
                }
            }
            _ => {}
        });
    }
}

/// Per-query handle bundling a leaf [`Node`], its resolved
/// [`IndexedMesh`], the spatial cache that belongs to that mesh and
/// the leaf's world transform composed from the ancestor chain.
/// Constructed only via [`SceneSpatial::leaf`]; the mesh and the
/// cache cannot be separated by callers.
pub struct ObjectQuery<'a> {
    node: &'a Node,
    mesh: &'a IndexedMesh,
    cache: &'a MeshSpatial,
    world_transform: Transform3D,
    parent_world: Transform3D,
}

impl<'a> ObjectQuery<'a> {
    /// The resolved mesh: for an inline leaf, the leaf's inline
    /// mesh; for a resource-backed leaf, the resource it points to.
    pub fn mesh(&self) -> &'a IndexedMesh {
        self.mesh
    }

    /// Leaf's parent-relative transform. For a loader-shape graph
    /// (a single identity-Group root with leaves attached directly),
    /// this equals the world transform.
    pub fn local_transform(&self) -> Transform3D {
        self.node.local_transform
    }

    /// Leaf's world-space transform, composed from the ancestor
    /// chain at query construction time. This is the transform
    /// that places the leaf's mesh in world coordinates.
    pub fn world_transform(&self) -> Transform3D {
        self.world_transform
    }

    /// Composed world transform of the leaf's ancestor chain,
    /// excluding the leaf's own local transform. Equals
    /// `Transform3D::Identity` when the leaf has no parent. For any
    /// candidate `local`, `parent_world ∘ local` is the leaf's
    /// world-space transform under that candidate. Callers that need
    /// to evaluate the leaf in world space against a different
    /// `local` than the one stored on the node (e.g. a captured drag
    /// original or a proposed-edit candidate) compose this with that
    /// `local`.
    pub fn parent_world(&self) -> Transform3D {
        self.parent_world
    }

    /// Object-space AABB of [`Self::mesh`]. Cached. `None` when the
    /// mesh has no vertices.
    pub fn local_aabb(&self) -> Option<Aabb> {
        self.cache.local_aabb(self.mesh)
    }

    /// World-space AABB of [`Self::mesh`] after applying `transform`.
    /// Computed from the transformed vertices, so the result is the
    /// tight AABB of the rotated mesh, not the larger AABB-of-the-
    /// rotated-local-AABB that Arvo's formula produces. `None` when
    /// the mesh has no vertices.
    ///
    /// Internally cached by the linear part of `transform`: the same
    /// linear key (typically the current rotation/scale) reuses the
    /// computed AABB and only shifts it by the new translation. A
    /// translate-mode drag, whose linear part is constant tick to
    /// tick, pays the vertex walk once.
    pub fn transformed_aabb(&self, transform: Transform3D) -> Option<Aabb> {
        match transform {
            Transform3D::Identity => self.local_aabb(),
            Transform3D::Affine(a) => {
                let aabb = self.cache.linear_aabb(self.mesh, a.linear())?;
                Some(translate_aabb(aabb, a.translation()))
            }
        }
    }

    /// Object-space triangle clusters over [`Self::mesh`]. Cached.
    /// `None` when the mesh has no faces or has corrupt face indices.
    pub fn clusters(&self) -> Option<&'a TriangleClusters> {
        self.cache.clusters(self.mesh)
    }
}

/// Compose the world transform of a leaf by walking its ancestor
/// chain up to the (scene or subtree) root. The order is
/// root-applied-last: `world = root.compose(parent).compose(self)`,
/// which matches the convention in [`Transform3D::compose`] where
/// `self.compose(other)` applies `other` first and `self` second.
fn compose_world_transform(graph: &Scene, leaf: NodeId) -> Transform3D {
    let mut chain: Vec<Transform3D> = Vec::new();
    let mut current = Some(leaf);
    while let Some(id) = current {
        let Some(node) = graph.node(id) else {
            break;
        };
        chain.push(node.local_transform);
        current = graph.parent_of(id);
    }
    let mut acc = Transform3D::Identity;
    for t in chain.iter().rev() {
        acc = acc.compose(t);
    }
    acc
}

/// Pre-order traversal of every node reachable from the scene roots
/// without expanding `Instance` edges. `visit` is called once per
/// reachable node.
fn for_each_reachable_node<F>(graph: &Scene, mut visit: F)
where
    F: FnMut(NodeId, &Node),
{
    let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        visit(id, node);
        match &node.kind {
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Leaf(_) | NodeKind::Instance { .. } => {}
        }
    }
}

fn translate_aabb(aabb: Aabb, t: &[f64; 3]) -> Aabb {
    let tx = t[0] as f32;
    let ty = t[1] as f32;
    let tz = t[2] as f32;
    let min = aabb.min().coords();
    let max = aabb.max().coords();
    Aabb::from_min_max(
        Vertex::new([min[0] + tx, min[1] + ty, min[2] + tz]),
        Vertex::new([max[0] + tx, max[1] + ty, max[2] + tz]),
    )
}
