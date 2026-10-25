//! DragonFruit `src/supports/PlacementLogic/Pathfinding/SDFCachePool.ts`.
//!
//! SDFCachePool — one cached distance field per mesh, shared by everything that
//! needs collision answers.
//!
//! This is infrastructure, not routing: the SDF is the oracle the routers ask
//! questions of, so it outlives any particular router.
//!
//! The pool is first-caller-wins on cell size: a mesh's accuracy must not depend
//! on which subsystem happened to touch it first.
//!
//! DragonFruit keeps the pool and its `onSDFMatrixDrift` listeners as module
//! globals; here the pool is a value its owner (one per placement worker) holds,
//! and the drift notification is the pool's `refresh_matrix`.

use std::sync::Arc;

use indexmap::IndexMap;

use super::sdf_cache::SdfCache;
use crate::three::Matrix4;
use crate::three_mesh_bvh::MeshBvh;

/// Single source of truth for the lazy SDF grid resolution. Gates whose margins
/// are finer than the cell-centre substitution error (~cellSize·√3/2) must use
/// `SdfCache::exact_signed_distance_at` instead of relying on grid resolution; the
/// contact-cone gate does.
pub const SDF_DEFAULT_CELL_SIZE_MM: f64 = 0.5;

type DriftListener = Box<dyn FnMut(&str)>;

#[derive(Default)]
pub struct SdfCachePool {
    caches: IndexMap<String, SdfCache>,
    drift_listeners: Vec<DriftListener>,
}

impl std::fmt::Debug for SdfCachePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdfCachePool")
            .field("caches", &self.caches.keys().collect::<Vec<_>>())
            .field("drift_listeners", &self.drift_listeners.len())
            .finish()
    }
}

impl SdfCachePool {
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached distance field for a mesh, built on first use.
    pub fn get_or_create_sdf_cache(
        &mut self,
        mesh_uuid: &str,
        bvh: impl FnOnce() -> (Arc<MeshBvh>, Matrix4),
    ) -> &mut SdfCache {
        self.caches.entry(mesh_uuid.to_owned()).or_insert_with(|| {
            let (bvh, matrix_world) = bvh();
            SdfCache::new(bvh, matrix_world, Some(SDF_DEFAULT_CELL_SIZE_MM))
        })
    }

    pub fn get(&self, mesh_uuid: &str) -> Option<&SdfCache> {
        self.caches.get(mesh_uuid)
    }

    /// `sdf.refreshMatrix()` on one mesh's cache, notifying the drift listeners
    /// when its world matrix changed. Returns whether it did.
    pub fn refresh_matrix(&mut self, mesh_uuid: &str, matrix_world: Matrix4) -> bool {
        let Some(cache) = self.caches.get_mut(mesh_uuid) else {
            return false;
        };
        let drifted = cache.refresh_matrix(matrix_world);
        if drifted {
            for listener in &mut self.drift_listeners {
                listener(mesh_uuid);
            }
        }
        drifted
    }

    /// Subscribe to "a mesh's world matrix changed" events, detected the next time
    /// any caller refreshes that mesh's pooled SDF. Used by world-space caches
    /// (stagnation points, placement results) whose entries are only valid for the
    /// model position they were recorded at.
    pub fn on_sdf_matrix_drift(&mut self, listener: impl FnMut(&str) + 'static) {
        self.drift_listeners.push(Box::new(listener));
    }

    /// Drop one mesh's cache (call when the mesh is removed or replaced).
    pub fn clear_sdf_cache_for_mesh(&mut self, mesh_uuid: &str) {
        if let Some(cache) = self.caches.shift_remove(mesh_uuid) {
            cache.clear();
        }
    }

    /// Drop every mesh's cache.
    pub fn clear_all_sdf_caches(&mut self) {
        for cache in self.caches.values() {
            cache.clear();
        }
        self.caches.clear();
    }
}
