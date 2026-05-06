/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-mesh acceleration structures.
//!
//! [`MeshSpatial`] is a private type. Callers cannot hold one in
//! isolation from the [`IndexedMesh`] it belongs to: the only path to
//! these caches is through
//! [`SceneSpatial::object`](crate::SceneSpatial::object), which pairs
//! a mesh and its `MeshSpatial` from a single internal table. That
//! pairing is what makes mismatched (mesh, cache) queries
//! structurally impossible.

use std::sync::OnceLock;

use sc_mesh_core::{Aabb, IndexedMesh};

use crate::clusters::TriangleClusters;

#[derive(Default)]
pub(crate) struct MeshSpatial {
    local_aabb: OnceLock<Aabb>,
    clusters: OnceLock<TriangleClusters>,
}

impl MeshSpatial {
    /// Object-space AABB of `mesh`. Cached on first call; `None` for
    /// empty meshes.
    ///
    /// The caller is responsible for passing the same `mesh` it
    /// would pass on every other call to this `MeshSpatial`. The
    /// public surface of the crate ([`SceneSpatial::object`]) takes
    /// care of that — this type is module-private precisely to keep
    /// the pairing inside the spatial layer.
    pub(crate) fn local_aabb(&self, mesh: &IndexedMesh) -> Option<Aabb> {
        if let Some(cached) = self.local_aabb.get() {
            return Some(*cached);
        }
        let computed = mesh.local_aabb()?;
        let _ = self.local_aabb.set(computed);
        Some(computed)
    }

    /// Object-space cluster AABBs over `mesh.faces`. Cached on first
    /// call; `None` for empty face sets or out-of-range face vertex
    /// indices (corrupt mesh).
    pub(crate) fn clusters(&self, mesh: &IndexedMesh) -> Option<&TriangleClusters> {
        if mesh.faces.is_empty() {
            return None;
        }
        if let Some(cached) = self.clusters.get() {
            return Some(cached);
        }
        let built = TriangleClusters::for_mesh(mesh)?;
        let _ = self.clusters.set(built);
        self.clusters.get()
    }

    /// Eagerly populate every cache in this slot. Used by
    /// [`SceneSpatial::primed`](crate::SceneSpatial::primed) to do
    /// the work off the UI thread before the mesh ever reaches a
    /// query site.
    pub(crate) fn prime(&self, mesh: &IndexedMesh) {
        let _ = self.local_aabb(mesh);
        let _ = self.clusters(mesh);
    }
}
