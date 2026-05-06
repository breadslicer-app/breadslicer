/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Spatial-query acceleration over [`sc_mesh_core`] types.
//!
//! This crate caches the results of geometric queries over
//! [`IndexedMesh`] and [`Scene`]: object-space AABBs and
//! [`TriangleClusters`] today; future spatial structures (BVH,
//! half-edge, slab buckets, distance fields) live here too.
//!
//! See `README.md` at the crate root for the architectural rule and
//! the connection model.

pub mod clusters;
mod mesh_spatial;
mod scene_spatial;

pub use clusters::TriangleClusters;
pub use scene_spatial::{ObjectQuery, SceneEdit, SceneSpatial};

use sc_mesh_core::{
    Aabb,
    scene::{Scene, Transform3D, arvo_world_aabb},
};

/// World-space AABB of a scene, computed through cached per-mesh
/// AABBs and unioned via Arvo's transformed-AABB formula. Returns
/// `None` for a scene with no geometry.
///
/// This is the cached counterpart to
/// [`sc_mesh_core::scene::Scene::aabb`], which is uncached and
/// recomputes from vertex data each call. Callers that query
/// repeatedly should construct a [`SceneSpatial`] and use this
/// function; one-shot callers may use the uncached form directly.
pub fn scene_aabb(spatial: &SceneSpatial) -> Option<Aabb> {
    let scene: &Scene = spatial.scene();
    let mut aabb = Aabb::empty();
    let mut has_any = false;
    for idx in 0..scene.objects.len() {
        let Some(query) = spatial.object(idx) else {
            continue;
        };
        let Some(local) = query.local_aabb() else {
            continue;
        };
        match &query.object().transform {
            Transform3D::Identity => aabb.union(&local),
            Transform3D::Affine(a) => aabb.union(&arvo_world_aabb(local, a)),
        }
        has_any = true;
    }
    has_any.then_some(aabb)
}
