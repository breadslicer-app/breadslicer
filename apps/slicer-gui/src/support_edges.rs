/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Supports that reach past the edge of their plate (#627): a model placed
//! near the edge by hand can get supports that do not fit on the plate.
//! Generate says so, and Arrange all, which measures each model with its
//! supports, puts them back.

use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::build_volume::BuildVolume;
use crate::plates::Plates;

/// How far past the edge a support may reach before it counts (mm): less
/// than a pixel of any printer is rounding.
const PAST_MM: f32 = 0.01;

/// The supports of `spatial` reaching past the edge of the plate they stand
/// on, each as the model it belongs to (`model_of`), or itself when it
/// belongs to none (one raft for all, supports a file came with): once each,
/// in scene order.
pub(crate) fn past_the_plate(
    spatial: &SceneSpatial,
    volume: &BuildVolume,
    plates: Plates,
    model_of: &dyn Fn(NodeId) -> Option<NodeId>,
) -> Vec<NodeId> {
    let (half_x, half_y) = (volume.width_mm() / 2.0, volume.depth_mm() / 2.0);
    let mut past = Vec::new();
    for leaf in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
        let Some(mesh) = spatial.graph().leaf_mesh_arc(leaf) else {
            continue;
        };
        if !sc_geom::df_supports::is_any_support_mesh(mesh) || mesh.faces().is_empty() {
            continue;
        }
        let Some(query) = spatial.leaf(leaf) else {
            continue;
        };
        let Some(aabb) = query.transformed_aabb(query.world_transform()) else {
            continue;
        };
        let Some(plate) =
            crate::plate_layout::plate_holding(spatial, volume, plates, leaf, model_of)
        else {
            continue;
        };
        let centre = crate::plates::offset(volume, plate);
        let (min, max) = (aabb.min().coords(), aabb.max().coords());
        let out = min[0] < centre.x - half_x - PAST_MM
            || max[0] > centre.x + half_x + PAST_MM
            || min[1] < centre.y - half_y - PAST_MM
            || max[1] > centre.y + half_y + PAST_MM;
        let owner = model_of(leaf).unwrap_or(leaf);
        if out && !past.contains(&owner) {
            past.push(owner);
        }
    }
    past
}
