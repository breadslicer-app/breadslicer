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

use indexmap::IndexMap;

use super::sdf_cache::SdfCache;
use crate::geom::Matrix4;

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
}
