/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-level spatial layer. Owns a [`Scene`] and the per-mesh
//! caches it needs.
//!
//! Co-ownership is the load-bearing design choice. A [`SceneSpatial`]
//! holds the [`Scene`] by value; readers can only obtain a
//! `(SceneObject, IndexedMesh, MeshSpatial)` triple through
//! [`SceneSpatial::object`], which returns an [`ObjectQuery<'_>`].
//! There is no public surface that lets a caller separate a
//! `MeshSpatial` from the `IndexedMesh` it was built alongside, so
//! mismatched queries are structurally impossible.

use sc_mesh_core::{
    Aabb, IndexedMesh,
    scene::{Scene, SceneObject, SceneObjectContent},
};

use crate::clusters::TriangleClusters;
use crate::mesh_spatial::MeshSpatial;

/// Scene plus per-mesh spatial caches. The cache crate's main
/// public type. Callers in the GUI swap their `Scene` storage for
/// this when they need cached spatial queries; headless callers
/// (`mesh-formats`, `slicer-cli`) never construct one.
///
/// Built either lazily ([`SceneSpatial::new`]) or eagerly
/// ([`SceneSpatial::primed`]). The query API is identical for both;
/// the only difference is when cache priming work runs.
pub struct SceneSpatial {
    scene: Scene,
    /// Parallel to `scene.objects`. `Some(_)` when
    /// `objects[i].content` is an inline `Mesh(_)`; `None` when it
    /// is a `Resource(_)` reference (the resource's cache lives in
    /// `resources` below).
    inline: Vec<Option<MeshSpatial>>,
    /// Parallel to `scene.resources`.
    resources: Vec<MeshSpatial>,
}

impl SceneSpatial {
    /// Wrap a [`Scene`] without priming any caches. First query on
    /// each mesh will build the relevant cache lazily.
    pub fn new(scene: Scene) -> Self {
        let inline = scene
            .objects
            .iter()
            .map(|o| match &o.content {
                SceneObjectContent::Mesh(_) => Some(MeshSpatial::default()),
                SceneObjectContent::Resource(_) => None,
            })
            .collect();
        let resources = (0..scene.resources.len())
            .map(|_| MeshSpatial::default())
            .collect();
        Self {
            scene,
            inline,
            resources,
        }
    }

    /// Wrap a [`Scene`] and eagerly populate every per-mesh cache.
    /// `Send`-safe; intended to run on the worker thread that already
    /// hosts parsing and GPU upload, so the GUI thread receives a
    /// fully-primed spatial without paying the priming cost.
    pub fn primed(scene: Scene) -> Self {
        let s = Self::new(scene);
        for (idx, slot) in s.inline.iter().enumerate() {
            if let Some(cache) = slot
                && let SceneObjectContent::Mesh(mesh) = &s.scene.objects[idx].content
            {
                cache.prime(mesh);
            }
        }
        for (idx, cache) in s.resources.iter().enumerate() {
            cache.prime(&s.scene.resources[idx]);
        }
        s
    }

    /// Borrow the underlying scene for read-only access.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Consume the spatial and return the inner scene, discarding
    /// any cached state. Useful for tests and for paths that want to
    /// hand the scene off to a non-spatial consumer.
    pub fn into_scene(self) -> Scene {
        self.scene
    }

    /// Look up a `(SceneObject, IndexedMesh, MeshSpatial)` triple by
    /// index into `scene.objects`. Returns `None` when the index is
    /// out of bounds, or when the object is a `Resource(idx)` whose
    /// resource index is out of bounds.
    ///
    /// This is the *only* public path from a spatial cache to a mesh
    /// in this crate. The triple comes from a single internal lookup,
    /// so the mesh and the cache are guaranteed to belong together —
    /// callers cannot pair them incorrectly.
    pub fn object(&self, idx: usize) -> Option<ObjectQuery<'_>> {
        let object = self.scene.objects.get(idx)?;
        let (mesh, cache) = match &object.content {
            SceneObjectContent::Mesh(mesh) => {
                let cache = self.inline.get(idx)?.as_ref()?;
                (mesh, cache)
            }
            SceneObjectContent::Resource(r) => {
                let mesh = self.scene.resources.get(*r)?;
                let cache = self.resources.get(*r)?;
                (mesh, cache)
            }
        };
        Some(ObjectQuery {
            object,
            mesh,
            cache,
        })
    }

    /// Apply a typed scene edit and update caches accordingly. The
    /// exhaustive `match` on the edit shape ensures that any future
    /// variant must declare its invalidation pattern at compile time
    /// — adding a variant without an arm here is a compile error.
    pub fn apply_edit(&mut self, edit: SceneEdit) {
        match &edit {
            SceneEdit::ObjectTransform { .. } => {
                // Transform-only edits do not invalidate. Per-mesh
                // caches are object-space (local AABB, clusters),
                // and a `Transform3D` change does not touch vertex
                // or face data. Future edit variants that mutate
                // geometry will call `invalidate_object` /
                // `invalidate_resource` from this arm.
            }
        }
        edit.apply_to_scene(&mut self.scene);
    }

    /// Clear cached state for the inline mesh at `objects[idx]`. No-op
    /// when the object is a resource reference (resources have their
    /// own slots) or when the index is out of bounds. Call this after
    /// any mutation to vertex or face data of the inline mesh.
    pub fn invalidate_object(&mut self, idx: usize) {
        if let Some(slot) = self.inline.get_mut(idx)
            && slot.is_some()
        {
            *slot = Some(MeshSpatial::default());
        }
    }

    /// Clear cached state for the resource mesh at
    /// `resources[idx]`. No-op when the index is out of bounds.
    pub fn invalidate_resource(&mut self, idx: usize) {
        if let Some(slot) = self.resources.get_mut(idx) {
            *slot = MeshSpatial::default();
        }
    }
}

/// Per-query handle bundling a [`SceneObject`], its resolved
/// [`IndexedMesh`], and the spatial cache that belongs to that
/// mesh. Constructed only via [`SceneSpatial::object`]; the mesh
/// and the cache cannot be separated by callers.
pub struct ObjectQuery<'a> {
    object: &'a SceneObject,
    mesh: &'a IndexedMesh,
    cache: &'a MeshSpatial,
}

impl<'a> ObjectQuery<'a> {
    /// The scene object this query refers to. Carries the
    /// transform, the metadata, and the content variant.
    pub fn object(&self) -> &'a SceneObject {
        self.object
    }

    /// The resolved mesh — for an inline object, the inline mesh;
    /// for a resource reference, the resource it points to.
    pub fn mesh(&self) -> &'a IndexedMesh {
        self.mesh
    }

    /// Object-space AABB of [`Self::mesh`]. Cached. `None` when the
    /// mesh has no vertices.
    pub fn local_aabb(&self) -> Option<Aabb> {
        self.cache.local_aabb(self.mesh)
    }

    /// Object-space triangle clusters over [`Self::mesh`]. Cached.
    /// `None` when the mesh has no faces or has corrupt face indices.
    pub fn clusters(&self) -> Option<&'a TriangleClusters> {
        self.cache.clusters(self.mesh)
    }
}

/// Typed scene mutation. The only mutation entry point on
/// [`SceneSpatial`] — see [`SceneSpatial::apply_edit`].
///
/// This mirrors the GUI's existing `SceneEdit` so the GUI can pass
/// edits through the spatial layer without translation. Headless
/// callers that don't go through `SceneSpatial` keep using the
/// scene types directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneEdit {
    /// `objects[object_idx].transform` was changed from `before` to
    /// `after`. Applying writes `after`; `inverse()` produces the
    /// undo edit.
    ObjectTransform {
        object_idx: usize,
        before: sc_mesh_core::scene::Transform3D,
        after: sc_mesh_core::scene::Transform3D,
    },
}

impl SceneEdit {
    /// True when applying this edit would not change the scene.
    /// Useful for filtering no-op gestures from the undo stack.
    pub fn is_noop(&self) -> bool {
        match self {
            Self::ObjectTransform { before, after, .. } => before == after,
        }
    }

    /// Inverse edit: applying `self` then `self.inverse()` returns
    /// the scene to its pre-edit state.
    pub fn inverse(&self) -> Self {
        match self {
            Self::ObjectTransform {
                object_idx,
                before,
                after,
            } => Self::ObjectTransform {
                object_idx: *object_idx,
                before: *after,
                after: *before,
            },
        }
    }

    fn apply_to_scene(&self, scene: &mut Scene) {
        match self {
            Self::ObjectTransform {
                object_idx, after, ..
            } => {
                if let Some(obj) = scene.objects.get_mut(*object_idx) {
                    obj.transform = *after;
                }
            }
        }
    }
}
