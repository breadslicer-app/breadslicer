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
pub use scene_spatial::{ObjectQuery, PrimingProgress, SceneSpatial};

use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{Aabb, NodeId, NodeKind};

/// World-space AABB of a scene, computed through cached per-mesh
/// AABBs and unioned via Arvo's transformed-AABB formula. Returns
/// `None` for a scene with no geometry.
///
/// The walk does not descend `Reference` nodes. That cannot drop
/// geometry here because [`SceneSpatial`]'s construction contract
/// rejects scenes with subtree references, so on every
/// constructible input this function agrees with the uncached,
/// reference-expanding [`sc_mesh_core::Scene::aabb`] walk, which
/// recomputes from vertex data each call. Callers that query
/// repeatedly should construct a [`SceneSpatial`] and use this
/// function; one-shot callers may use the uncached form directly.
pub fn scene_aabb(spatial: &SceneSpatial) -> Option<Aabb> {
    let graph = spatial.graph();
    let mut aabb = Aabb::empty();
    let mut has_any = false;
    let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        match &node.kind {
            NodeKind::Leaf(_) => {
                let Some(query) = spatial.leaf(id) else {
                    continue;
                };
                let Some(local) = query.local_aabb() else {
                    continue;
                };
                match query.world_transform() {
                    Transform3D::Identity => aabb.union(&local),
                    Transform3D::Affine(a) => aabb.union(&local.transformed_arvo(&a)),
                }
                has_any = true;
            }
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Reference { .. } => {}
        }
    }
    has_any.then_some(aabb)
}
