/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-level spatial layer. Owns a [`SceneGraph`] and the per-mesh
//! caches it needs.
//!
//! Co-ownership is the load-bearing design choice. A [`SceneSpatial`]
//! holds the [`SceneGraph`] by value; readers obtain an
//! [`ObjectQuery<'_>`] handle via [`SceneSpatial::leaf`] (by stable
//! [`NodeId`]). The query bundles the leaf node, the resolved mesh
//! and the spatial cache that belongs to that mesh; mismatched
//! pairings are structurally impossible.
//!
//! The type also keeps a flat [`Scene`] mirror alongside the graph;
//! see the `mirror` field below. The mirror is a temporary bridge
//! and is removed once every consumer of [`SceneSpatial::scene`]
//! has been retargeted to [`SceneSpatial::graph`]; see the comments
//! marked "BRIDGE" through this file for what to delete then.

use sc_mesh_core::{
    Aabb, IndexedMesh, LeafPayload, MeshResourceId, Node, NodeId, NodeKind, SceneGraph,
    SceneGraphError, Vertex,
    scene::{Scene, SceneError, Transform3D},
    scene_edit::SceneEdit,
};

use crate::clusters::TriangleClusters;
use crate::mesh_spatial::MeshSpatial;

/// SceneGraph plus per-mesh spatial caches. The cache crate's main
/// public type. Callers in the GUI swap their `SceneGraph` storage
/// for this when they need cached spatial queries; headless callers
/// (`mesh-formats`, `slicer-cli`) never construct one.
///
/// Built either lazily ([`SceneSpatial::from_graph`]) or eagerly
/// ([`SceneSpatial::primed_graph`]). The query API is identical for
/// both; the only difference is when cache priming work runs.
pub struct SceneSpatial {
    graph: SceneGraph,
    /// BRIDGE: flat [`Scene`] mirror kept in sync with `graph` for
    /// the duration of the migration that flips `SceneSpatial` to a
    /// graph-shaped public surface. Some GUI callers still consume a
    /// `&Scene` (sidebar list, GPU upload, viewport render struct)
    /// and the mirror lets them stay unchanged commit-to-commit
    /// while the graph-based accessors are added.
    ///
    /// DELETE-ON-CLEANUP: when the last consumer of
    /// [`SceneSpatial::scene`] has been retargeted to
    /// [`SceneSpatial::graph`], drop this field, drop the `scene()`
    /// method, drop the mirror-allocation in `build` and the
    /// mirror-sync in `apply_edit`, and make `from_graph` and
    /// `primed_graph` infallible again.
    mirror: Scene,
    /// Per-leaf spatial caches. Indexed by `NodeId::index()`. The
    /// vector is sized to [`SceneGraph::nodes_len`] so any live
    /// `NodeId` is always in bounds. `Some(_)` only for nodes whose
    /// kind is `Leaf(LeafPayload::Mesh(_))`; `None` for groups, CSG
    /// nodes, instances, mesh-resource-backed leaves (whose cache
    /// lives in `mesh_resources` below) and tombstoned slots.
    leaves: Vec<Option<MeshSpatial>>,
    /// Per-mesh-resource spatial caches. Indexed by
    /// `MeshResourceId::index()`, sized to
    /// [`SceneGraph::mesh_resources_len`].
    mesh_resources: Vec<MeshSpatial>,
}

impl SceneSpatial {
    /// Wrap a [`SceneGraph`] without priming any caches. First query
    /// on each mesh will build the relevant cache lazily.
    ///
    /// Returns `Err(SceneGraphError)` when the bridge `Scene` mirror
    /// cannot be built from the graph (i.e. the graph contains CSG
    /// or `Instance` content). DELETE-ON-CLEANUP: drop the
    /// `Result` wrapping and let this method be infallible.
    pub fn from_graph(graph: SceneGraph) -> Result<Self, SceneGraphError> {
        let mirror = graph.try_flatten()?;
        Ok(Self::build(mirror, graph))
    }

    /// Wrap a [`SceneGraph`] and eagerly populate every per-mesh
    /// cache reachable from the scene roots. `Send`-safe; intended
    /// to run on the worker thread that already hosts parsing and
    /// GPU upload.
    ///
    /// Mesh-resource caches are primed via the leaves that reach
    /// them. A resource referenced by N leaves is primed N times;
    /// `MeshSpatial::prime` is idempotent, so the extra work is
    /// duplicated cache lookups, not duplicated computation.
    ///
    /// Returns `Err` for the same reason as [`Self::from_graph`].
    /// DELETE-ON-CLEANUP: drop the `Result` wrapping when the
    /// mirror is removed.
    pub fn primed_graph(graph: SceneGraph) -> Result<Self, SceneGraphError> {
        let s = Self::from_graph(graph)?;
        s.prime_reachable_caches();
        Ok(s)
    }

    /// BRIDGE: build a [`SceneSpatial`] from a flat [`Scene`].
    /// `scene` itself becomes the mirror (no flattening work
    /// needed). The graph is built from a clone of `scene` because
    /// `SceneGraph::try_from` consumes its input.
    ///
    /// Returns `Err(SceneError)` when the input `Scene` is
    /// structurally invalid (notably a `Resource(idx)` reference
    /// past the resource vector).
    ///
    /// DELETE-ON-CLEANUP: drop this method when the last consumer
    /// has flipped to [`Self::from_graph`].
    pub fn new(scene: Scene) -> Result<Self, SceneError> {
        let graph = SceneGraph::try_from(scene.clone())?;
        Ok(Self::build(scene, graph))
    }

    /// BRIDGE: eager-prime variant of [`Self::new`]. Same
    /// fallibility, same DELETE-ON-CLEANUP applies.
    pub fn primed(scene: Scene) -> Result<Self, SceneError> {
        let s = Self::new(scene)?;
        s.prime_reachable_caches();
        Ok(s)
    }

    /// Borrow the underlying graph for read-only access.
    pub fn graph(&self) -> &SceneGraph {
        &self.graph
    }

    /// Consume the spatial and return the inner graph, discarding
    /// any cached state.
    pub fn into_graph(self) -> SceneGraph {
        self.graph
    }

    /// BRIDGE: borrow the flat [`Scene`] mirror of the underlying
    /// graph. Kept in sync by [`Self::apply_edit`] for every
    /// transform-only edit.
    ///
    /// DELETE-ON-CLEANUP: drop this method when the last consumer
    /// has switched to [`Self::graph`].
    pub fn scene(&self) -> &Scene {
        &self.mirror
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
                (mesh, cache)
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

    /// BRIDGE: look up a leaf by its zero-based pre-order position
    /// in the scene-root traversal. Equivalent to
    /// `self.leaf(self.leaf_id_at_index(idx)?)`.
    ///
    /// DELETE-ON-CLEANUP: drop this method when the last consumer
    /// has switched to [`Self::leaf`].
    pub fn object(&self, idx: usize) -> Option<ObjectQuery<'_>> {
        let id = self.leaf_id_at_index(idx)?;
        self.leaf(id)
    }

    /// Apply a typed scene edit and update caches accordingly.
    /// Transform-only edits do not invalidate the per-mesh caches,
    /// which are object-space (local AABB, clusters).
    pub fn apply_edit(&mut self, edit: SceneEdit) {
        let SceneEdit::SetLocalTransform { node_id, after, .. } = edit;
        let _ = self.graph.set_local_transform(node_id, after);
        // BRIDGE: keep the `Scene` mirror in sync. Resolve the
        // leaf's pre-order index in the (still-flat) graph and
        // update the matching mirror entry. DELETE-ON-CLEANUP with
        // the mirror field.
        if let Some(idx) = self.leaf_index_of(node_id)
            && let Some(obj) = self.mirror.objects.get_mut(idx)
        {
            obj.transform = after;
        }
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
    /// by the legacy idx-keyed bridge methods and by the GUI's
    /// in-progress migration glue.
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

    /// BRIDGE: inverse of [`Self::leaf_id_at_index`]: walk the scene
    /// roots in pre-order and return the leaf-index of `target`, or
    /// `None` when the node is not a reachable leaf. Used by
    /// `apply_edit` to update the [`Scene`] mirror by idx, and by
    /// the slicer-gui selection bridge so that legacy idx-keyed
    /// renderer / sidebar / FSM consumers keep working after the
    /// selection switches to [`NodeId`] internally.
    ///
    /// DELETE-ON-CLEANUP with the mirror field and the legacy
    /// idx-keyed selection bridge in `slicer-gui/src/selection.rs`.
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

    /// Infallible inner constructor shared by all entry points.
    /// `mirror` must already be a flat representation of `graph`;
    /// the entry points enforce that contract.
    fn build(mirror: Scene, graph: SceneGraph) -> Self {
        let mut leaves: Vec<Option<MeshSpatial>> = (0..graph.nodes_len()).map(|_| None).collect();
        let mesh_resources: Vec<MeshSpatial> = (0..graph.mesh_resources_len())
            .map(|_| MeshSpatial::default())
            .collect();
        Self::allocate_inline_cache_slots(&graph, &mut leaves);
        Self {
            graph,
            mirror,
            leaves,
            mesh_resources,
        }
    }

    fn allocate_inline_cache_slots(graph: &SceneGraph, leaves: &mut [Option<MeshSpatial>]) {
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
/// Constructed only via [`SceneSpatial::leaf`] (or the legacy
/// idx-keyed [`SceneSpatial::object`] bridge); the mesh and the
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

    /// Leaf's parent-relative transform. For a graph produced from a
    /// flat `Scene` (a single identity-root Group), this equals the
    /// world transform.
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
fn compose_world_transform(graph: &SceneGraph, leaf: NodeId) -> Transform3D {
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
fn for_each_reachable_node<F>(graph: &SceneGraph, mut visit: F)
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
