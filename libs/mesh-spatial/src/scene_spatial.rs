/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-level spatial layer. Owns a [`Scene`] and the per-mesh
//! caches it needs.
//!
//! Co-ownership is the design choice everything else here rests on.
//! A [`SceneSpatial`]
//! holds the [`Scene`] by value; readers obtain an
//! [`ObjectQuery<'_>`] handle via [`SceneSpatial::leaf`] (by stable
//! [`NodeId`]). The query bundles the leaf node, the resolved mesh
//! and the spatial cache that belongs to that mesh; mismatched
//! pairings are structurally impossible.

use std::sync::Arc;

use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{Aabb, IndexedMesh, Vertex};
use sc_scene::{
    CsgPolicy, LeafPayload, MeshResourceId, Node, NodeId, NodeKind, ReferencePolicy, Scene,
    SceneError,
};
use sc_scene_edit::SceneEdit;

use crate::clusters::TriangleClusters;
use crate::mesh_spatial::MeshSpatial;
use crate::primed::PrimedMesh;

/// Triangle counts reported while eager cache priming runs.
///
/// One tick fires per primed leaf, after its cache is built.
/// `total_triangles` is fixed for the whole run, computed by the same
/// traversal that primes: a mesh resource referenced by N leaves
/// contributes its triangles N times to BOTH counts, matching the
/// idempotent re-prime the traversal performs, so the counts stay
/// consistent and the final tick reaches the total. A zero-triangle
/// leaf still ticks, with the count unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimingProgress {
    /// Triangles whose leaf caches have been primed so far.
    pub primed_triangles: u64,
    /// Triangles reachable from the scene roots, per leaf visit.
    pub total_triangles: u64,
}

/// Scene plus per-mesh spatial caches. The cache crate's main
/// public type. Callers in the GUI swap their `Scene` storage
/// for this when they need cached spatial queries; headless callers
/// (`mesh-formats`, `slicer-cli`) never construct one.
///
/// Construction contract: the cached walks do not expand
/// `Reference` nodes, so scenes using the sharing vocabulary
/// (registered subtree resources and references to them) are
/// outside this type's supported phase and are rejected at
/// construction in debug builds.
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
    /// nodes, references, mesh-resource-backed leaves (whose cache
    /// lives in `mesh_resources` below) and tombstoned slots.
    leaves: Vec<Option<Arc<MeshSpatial>>>,
    /// Per-mesh-resource spatial caches. Indexed by
    /// `MeshResourceId::index()`, sized to
    /// [`Scene::mesh_resources_len`].
    ///
    /// A resource added by an edit for a mesh another slot already
    /// holds (the same `Arc`, as a paste of a copied object adds it)
    /// shares that slot's cache: the caches are object-space, and a
    /// mesh shared by two slots cannot change, so the copy starts
    /// with everything the original already computed.
    mesh_resources: Vec<Arc<MeshSpatial>>,
}

impl std::fmt::Debug for SceneSpatial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneSpatial")
            .field("graph", &self.graph)
            .finish_non_exhaustive()
    }
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
        Self::primed_graph_with_progress(graph, |_| {})
    }

    /// [`SceneSpatial::primed_graph`], reporting per-leaf priming
    /// progress; see [`PrimingProgress`] for the tick contract.
    pub fn primed_graph_with_progress(
        graph: Scene,
        mut progress: impl FnMut(PrimingProgress),
    ) -> Self {
        let s = Self::from_graph(graph);
        s.prime_reachable_caches(&mut progress);
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
    /// here, and an inline-mesh leaf installed at the id through a
    /// typed scene edit gets its entry when the edit applies.
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
    ///   references do not carry geometry of their own),
    /// * the leaf payload is a `MeshResource` whose resource is
    ///   dangling,
    /// * the leaf sits inside a registered subtree: such scenes are
    ///   outside the construction contract (rejected in debug
    ///   builds), and no cache slot exists for their inline meshes.
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
        let world_transform = self.graph.world_transform_of(id).ok()?;
        let parent_world = match self.graph.parent_of(id) {
            Some(parent) => self.graph.world_transform_of(parent).ok()?,
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

    /// Apply a typed scene edit through
    /// [`SceneEdit::apply_to_scene`] (Compound atomicity included)
    /// and, on success, clear the cached state the edit made stale.
    /// On error the graph and every cache are unchanged.
    pub fn apply_edit(&mut self, edit: SceneEdit) -> Result<(), SceneError> {
        self.apply_edit_primed(edit, &[])
    }

    /// [`Self::apply_edit`], then each slot the edit put a mesh in takes
    /// the caches of `primed` built for that very mesh (the same `Arc`),
    /// so the first query on it builds nothing. A slot whose mesh is not
    /// among them keeps what `apply_edit` leaves it; one that some later
    /// edit gives another mesh drops the caches, as `apply_edit` drops any.
    /// On error nothing changes, as with `apply_edit`.
    pub fn apply_edit_primed(
        &mut self,
        edit: SceneEdit,
        primed: &[PrimedMesh],
    ) -> Result<(), SceneError> {
        edit.apply_to_scene(&mut self.graph)?;
        self.invalidate_for(&edit);
        self.share_for(&edit);
        if !primed.is_empty() {
            self.adopt_for(&edit, primed);
        }
        Ok(())
    }

    /// Gives each slot `edit` put a mesh in the caches of `primed` built
    /// for the mesh the slot holds now. The check is against the scene as
    /// the edit left it, not against the edit: a slot takes caches only for
    /// the `Arc` it holds.
    fn adopt_for(&mut self, edit: &SceneEdit, primed: &[PrimedMesh]) {
        let built_for = |mesh: &Arc<IndexedMesh>| {
            primed
                .iter()
                .find_map(|p| p.cache_for(mesh))
                .map(Arc::clone)
        };
        match edit {
            SceneEdit::AddMeshResource { rid, .. } | SceneEdit::ReplaceMeshResource { rid, .. } => {
                if let Some(cache) = self.graph.mesh_resource_arc(*rid).and_then(built_for)
                    && let Some(slot) = self.mesh_resources.get_mut(rid.index())
                {
                    *slot = cache;
                }
            }
            SceneEdit::AddLeaf { node_id, .. } | SceneEdit::ReplaceLeafMesh { node_id, .. } => {
                if let Some(node) = self.graph.node(*node_id)
                    && let NodeKind::Leaf(LeafPayload::Mesh(mesh)) = &node.kind
                    && let Some(cache) = built_for(mesh)
                    && let Some(slot) = self.leaves.get_mut(node_id.index())
                {
                    *slot = Some(cache);
                }
            }
            SceneEdit::Compound(edits) => {
                for edit in edits {
                    self.adopt_for(edit, primed);
                }
            }
            SceneEdit::SetLocalTransform { .. }
            | SceneEdit::RemoveLeaf { .. }
            | SceneEdit::RemoveMeshResource { .. } => {}
        }
    }

    /// Point each mesh resource `edit` added at the cache of another
    /// slot that holds the same mesh `Arc`, when there is one.
    fn share_for(&mut self, edit: &SceneEdit) {
        match edit {
            SceneEdit::AddMeshResource { rid, .. } => self.share_resource_cache(*rid),
            SceneEdit::AddLeaf {
                node_id,
                payload: LeafPayload::Mesh(_),
                ..
            } => self.give_inline_cache(*node_id),
            SceneEdit::Compound(edits) => {
                for edit in edits {
                    self.share_for(edit);
                }
            }
            SceneEdit::SetLocalTransform { .. }
            | SceneEdit::AddLeaf { .. }
            | SceneEdit::RemoveLeaf { .. }
            | SceneEdit::RemoveMeshResource { .. }
            | SceneEdit::ReplaceMeshResource { .. }
            | SceneEdit::ReplaceLeafMesh { .. } => {}
        }
    }

    /// Give the resource `rid` the cache of another slot holding the
    /// mesh `Arc` it holds now. No-op when no other slot holds it.
    fn share_resource_cache(&mut self, rid: MeshResourceId) {
        let Some(mesh) = self.graph.mesh_resource_arc(rid) else {
            return;
        };
        let found = self.cache_held_for(mesh, Some(rid), None);
        if let (Some(cache), Some(slot)) = (found, self.mesh_resources.get_mut(rid.index())) {
            *slot = cache;
        }
    }

    /// Give the inline-mesh leaf `id`, just installed by an edit, a
    /// cache slot: the cache of another slot holding the same mesh
    /// `Arc` (a copy shows the mesh its original shows), else an empty
    /// one. A slot the leaf held before it was removed is kept: the
    /// edit that installs the leaf again (the removal's inverse) brings
    /// back the same `Arc`, whose caches the slot still holds.
    fn give_inline_cache(&mut self, id: NodeId) {
        let Some(NodeKind::Leaf(LeafPayload::Mesh(mesh))) = self.graph.node(id).map(|n| &n.kind)
        else {
            return;
        };
        if matches!(self.leaves.get(id.index()), Some(Some(_))) {
            return;
        }
        let cache = self
            .cache_held_for(mesh, None, Some(id))
            .unwrap_or_default();
        if self.leaves.len() <= id.index() {
            self.leaves.resize(id.index() + 1, None);
        }
        self.leaves[id.index()] = Some(cache);
    }

    /// The cache of a slot other than `rid` and `id` that holds the
    /// mesh `Arc` `mesh`: a mesh resource, else a reachable inline leaf.
    fn cache_held_for(
        &self,
        mesh: &Arc<IndexedMesh>,
        rid: Option<MeshResourceId>,
        id: Option<NodeId>,
    ) -> Option<Arc<MeshSpatial>> {
        for (other, _) in self.graph.mesh_resources_iter() {
            if Some(other) != rid
                && let Some(arc) = self.graph.mesh_resource_arc(other)
                && Arc::ptr_eq(arc, mesh)
                && let Some(cache) = self.mesh_resources.get(other.index())
            {
                return Some(Arc::clone(cache));
            }
        }
        let mut found: Option<Arc<MeshSpatial>> = None;
        self.graph.for_each_reachable_node(|other, node| {
            if found.is_none()
                && Some(other) != id
                && let NodeKind::Leaf(LeafPayload::Mesh(arc)) = &node.kind
                && Arc::ptr_eq(arc, mesh)
                && let Some(Some(cache)) = self.leaves.get(other.index())
            {
                found = Some(Arc::clone(cache));
            }
        });
        found
    }

    /// Clear the cache slots `edit` made stale: only the mesh-swap
    /// arms need it. Per-mesh caches are object-space, so transform
    /// edits leave them valid; membership edits target slots that
    /// were sized at allocation time and stay keyed by their ids.
    fn invalidate_for(&mut self, edit: &SceneEdit) {
        match edit {
            SceneEdit::ReplaceMeshResource { rid, .. } => self.invalidate_mesh_resource(*rid),
            SceneEdit::ReplaceLeafMesh { node_id, .. } => self.invalidate_leaf(*node_id),
            SceneEdit::Compound(edits) => {
                for edit in edits {
                    self.invalidate_for(edit);
                }
            }
            SceneEdit::SetLocalTransform { .. }
            | SceneEdit::AddLeaf { .. }
            | SceneEdit::RemoveLeaf { .. }
            | SceneEdit::AddMeshResource { .. }
            | SceneEdit::RemoveMeshResource { .. } => {}
        }
    }

    /// Allocate a fresh [`MeshResourceId`] without populating its
    /// slot, and extend the per-resource cache vector to keep
    /// `self.mesh_resources.len() == self.graph.mesh_resources_len()`.
    /// Mirrors [`Self::allocate_unbound_node_id`] for the
    /// mesh-resource arena.
    pub fn allocate_unbound_mesh_resource_id(&mut self) -> MeshResourceId {
        let rid = self.graph.allocate_unbound_mesh_resource_id();
        self.mesh_resources.push(Arc::default());
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
            *slot = Some(Arc::default());
        }
    }

    /// Clear cached state for the mesh resource at `id`. No-op on a
    /// dangling id.
    pub fn invalidate_mesh_resource(&mut self, id: MeshResourceId) {
        if let Some(cache) = self.mesh_resources.get_mut(id.index()) {
            *cache = Arc::default();
        }
    }

    /// Resolve a zero-based pre-order leaf index against the scene
    /// roots, returning the addressed leaf's stable [`NodeId`]. The
    /// index space is [`Scene::reachable_leaves`] under
    /// (`Descend`, `Skip`): CSG operand leaves have rows, and
    /// references cannot occur here (construction contract).
    pub fn leaf_id_at_index(&self, target: usize) -> Option<NodeId> {
        self.row_leaves().get(target).copied()
    }

    /// Resolve a reachable leaf's [`NodeId`] back to its pre-order
    /// leaf index; `None` when `target` is not a reachable leaf.
    /// Inverse of [`Self::leaf_id_at_index`].
    pub fn leaf_index_of(&self, target: NodeId) -> Option<usize> {
        self.row_leaves().iter().position(|&id| id == target)
    }

    /// Collect the (Descend, Skip) structural leaf order both row
    /// queries resolve against.
    fn row_leaves(&self) -> Vec<NodeId> {
        self.graph
            .reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
            .expect("a (Descend, Skip) structural walk has no error source")
    }

    /// Inner constructor shared by all entry points.
    fn build(graph: Scene) -> Self {
        #[cfg(debug_assertions)]
        Self::assert_supported_vocabulary(&graph);
        let mut leaves: Vec<Option<Arc<MeshSpatial>>> =
            (0..graph.nodes_len()).map(|_| None).collect();
        let mesh_resources: Vec<Arc<MeshSpatial>> = (0..graph.mesh_resources_len())
            .map(|_| Arc::default())
            .collect();
        Self::allocate_inline_cache_slots(&graph, &mut leaves);
        Self {
            graph,
            leaves,
            mesh_resources,
        }
    }

    /// Debug-build enforcement of the construction contract: the
    /// cached walks in this type skip `Reference` nodes, so a scene
    /// using the sharing vocabulary would prime caches that silently
    /// disagree with the graph. Such scenes are rejected here, at
    /// the single shared constructor.
    #[cfg(debug_assertions)]
    fn assert_supported_vocabulary(graph: &Scene) {
        if let Some(node_id) = graph.first_reference_node() {
            panic!(
                "SceneSpatial cannot hold subtree references: node {} references a subtree",
                node_id.get(),
            );
        }
        assert!(
            !graph.has_subtree_resources(),
            "SceneSpatial requires a scene without registered subtree resources",
        );
    }

    fn allocate_inline_cache_slots(graph: &Scene, leaves: &mut [Option<Arc<MeshSpatial>>]) {
        graph.for_each_reachable_node(|id, node| {
            if let NodeKind::Leaf(LeafPayload::Mesh(_)) = &node.kind {
                leaves[id.index()] = Some(Arc::default());
            }
        });
    }

    fn prime_reachable_caches(&self, progress: &mut impl FnMut(PrimingProgress)) {
        let total_triangles = self.reachable_triangle_total();
        let mut primed_triangles: u64 = 0;
        self.graph
            .for_each_reachable_node(|id, node| match &node.kind {
                NodeKind::Leaf(LeafPayload::Mesh(mesh)) => {
                    if let Some(Some(cache)) = self.leaves.get(id.index()) {
                        cache.prime(mesh);
                        primed_triangles += mesh.faces.len() as u64;
                        progress(PrimingProgress {
                            primed_triangles,
                            total_triangles,
                        });
                    }
                }
                NodeKind::Leaf(LeafPayload::MeshResource(rid)) => {
                    if let (Some(mesh), Some(cache)) = (
                        self.graph.mesh_resource(*rid),
                        self.mesh_resources.get(rid.index()),
                    ) {
                        cache.prime(mesh);
                        primed_triangles += mesh.faces.len() as u64;
                        progress(PrimingProgress {
                            primed_triangles,
                            total_triangles,
                        });
                    }
                }
                _ => {}
            });
    }

    /// Count reachable triangles, one contribution per leaf visit,
    /// mirroring exactly what `prime_reachable_caches` primes so the
    /// two agree on the denominator.
    fn reachable_triangle_total(&self) -> u64 {
        let mut total: u64 = 0;
        self.graph
            .for_each_reachable_node(|id, node| match &node.kind {
                NodeKind::Leaf(LeafPayload::Mesh(mesh)) => {
                    if let Some(Some(_)) = self.leaves.get(id.index()) {
                        total += mesh.faces.len() as u64;
                    }
                }
                NodeKind::Leaf(LeafPayload::MeshResource(rid)) => {
                    if let (Some(mesh), Some(_)) = (
                        self.graph.mesh_resource(*rid),
                        self.mesh_resources.get(rid.index()),
                    ) {
                        total += mesh.faces.len() as u64;
                    }
                }
                _ => {}
            });
        total
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

impl std::fmt::Debug for ObjectQuery<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectQuery")
            .field("node", &self.node)
            .field("world_transform", &self.world_transform)
            .field("parent_world", &self.parent_world)
            .finish_non_exhaustive()
    }
}

impl<'a> ObjectQuery<'a> {
    /// Borrow the leaf's resolved mesh: the inline mesh for an
    /// inline leaf, the pointed-to resource for a resource-backed
    /// leaf.
    pub fn mesh(&self) -> &'a IndexedMesh {
        self.mesh
    }

    /// Return the leaf's transform relative to its parent.
    pub fn local_transform(&self) -> Transform3D {
        self.node.local_transform
    }

    /// Return the leaf's world-space transform, composed from its
    /// ancestor chain.
    pub fn world_transform(&self) -> Transform3D {
        self.world_transform
    }

    /// Return the composed world transform of the ancestor chain,
    /// excluding the leaf's own local transform; `Identity` for a
    /// parentless leaf. For any `local`,
    /// `parent_world().compose(&local)` is the leaf's world
    /// transform under that `local`.
    pub fn parent_world(&self) -> Transform3D {
        self.parent_world
    }

    /// Compute the object-space AABB of [`Self::mesh`], cached on
    /// first call; `None` when the mesh has no vertices.
    pub fn local_aabb(&self) -> Option<Aabb> {
        self.cache.local_aabb(self.mesh)
    }

    /// Compute the world-space AABB of [`Self::mesh`] under
    /// `transform`, from the transformed vertices: the tight AABB of
    /// the rotated mesh, not the larger box Arvo's formula yields
    /// from the local AABB. `None` when the mesh has no vertices.
    ///
    /// Internally cached by the linear part of `transform`: the same
    /// linear key (typically the current rotation/scale) reuses the
    /// computed AABB and only shifts it by the new translation. Sound
    /// because a translation moves every vertex, and therefore the
    /// tight AABB, by the same offset: AABB(M v + t) = AABB(M v) + t.
    /// A translate-mode drag, whose linear part is constant tick to
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

    /// Lowest actual mesh vertex under a world transform. Cached vertex-block
    /// bounds prune the search; no transformed local-box corners are substituted
    /// for geometry. Returns None for an empty mesh.
    pub fn minimum_world_z(&self, transform: Transform3D) -> Option<f64> {
        match transform {
            Transform3D::Identity => self.local_aabb().map(|b| f64::from(b.min().coords()[2])),
            Transform3D::Affine(a) => self
                .cache
                .minimum_projection(self.mesh, a.linear()[2])
                .map(|z| z + a.translation()[2]),
        }
    }

    /// Compute the object-space triangle clusters over
    /// [`Self::mesh`], cached on first call; `None` when the mesh
    /// has no faces or has corrupt face indices.
    pub fn clusters(&self) -> Option<&'a TriangleClusters> {
        self.cache.clusters(self.mesh)
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
