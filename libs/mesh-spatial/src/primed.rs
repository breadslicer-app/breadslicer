/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The caches of one mesh, built ahead of time (#395).
//!
//! A mesh that joins an open scene through an edit starts with empty
//! caches: the first query on it (a pick, a box) builds them on the
//! thread that asks, which for a large mesh is a visible hitch. A
//! [`PrimedMesh`] builds them where the mesh was made, a worker thread,
//! and [`SceneSpatial::apply_edit_primed`](crate::SceneSpatial::apply_edit_primed)
//! hands them to the slot that takes the mesh.
//!
//! The caches are those a query would build: the same code builds them,
//! from the same mesh. They hold the mesh's `Arc`, and a slot takes them
//! only while it holds that very `Arc`, so they cannot go to another mesh,
//! nor outlive a change to this one: a holder of the `Arc` cannot write
//! through it while another holds it too, and an edit that changes a mesh
//! puts a new `Arc` in its slot.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;

use crate::clusters::TriangleClusters;
use crate::mesh_spatial::MeshSpatial;

/// A mesh with its caches built. `Send`: build it on a worker and hand it
/// to the thread that applies the edit adding the mesh.
pub struct PrimedMesh {
    mesh: Arc<IndexedMesh>,
    cache: Arc<MeshSpatial>,
}

impl std::fmt::Debug for PrimedMesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrimedMesh")
            .field("faces", &self.mesh.faces.len())
            .finish_non_exhaustive()
    }
}

impl PrimedMesh {
    /// Builds every cache of `mesh` that a scene would build on its first
    /// queries: its box, its triangle clusters and its vertex blocks.
    pub fn new(mesh: Arc<IndexedMesh>) -> Self {
        let cache = Arc::new(MeshSpatial::default());
        cache.prime(&mesh);
        Self { mesh, cache }
    }

    /// The mesh the caches are of.
    pub fn mesh(&self) -> &Arc<IndexedMesh> {
        &self.mesh
    }

    /// The triangle clusters built for [`Self::mesh`]; `None` as a query
    /// gives it: no faces, or corrupt face indices.
    pub fn clusters(&self) -> Option<&TriangleClusters> {
        self.cache.clusters(&self.mesh)
    }

    /// The caches, when `mesh` is the very mesh they were built for.
    pub(crate) fn cache_for(&self, mesh: &Arc<IndexedMesh>) -> Option<&Arc<MeshSpatial>> {
        Arc::ptr_eq(&self.mesh, mesh).then_some(&self.cache)
    }
}
