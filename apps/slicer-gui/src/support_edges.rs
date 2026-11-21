/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Supports that reach past the edge of their plate (#628): a model placed
//! near the edge by hand can get supports that do not fit on the plate.
//! Generate says so, and Arrange all, which measures each model with its
//! supports, puts them back. A slice refuses a plate whose models or
//! supports stick out of it. A raft's edge past the plate is cut and the
//! print comes out the same (Damian): only what stands above the raft
//! counts.

use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::build_volume::BuildVolume;
use crate::plates::Plates;

/// How far past the edge a support may reach before it counts (mm): less
/// than a pixel of any printer is rounding.
const PAST_MM: f32 = 0.01;

/// What [`past_on`] looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reach {
    /// Supports alone, past the plate's edge (after Generate).
    Supports,
    /// Everything that prints, past the plate's edge or above the printer's
    /// height (before a slice: what would not print whole).
    Everything,
}

/// The supports of `spatial` reaching past the edge of the plate they stand
/// on, each as the model it belongs to (`model_of`), or itself when it
/// belongs to none (one raft for all, supports a file came with): once each,
/// in scene order.
pub(crate) fn past_the_plate(
    spatial: &SceneSpatial,
    volume: &BuildVolume,
    plates: Plates,
    model_of: &dyn Fn(NodeId) -> Option<NodeId>,
    raft_top_mm: f32,
) -> Vec<NodeId> {
    past_on(
        spatial,
        volume,
        plates,
        model_of,
        raft_top_mm,
        None,
        Reach::Supports,
    )
}

/// What `reach` says of `spatial` sticks out of its plate, `only` that plate
/// when given: each as the model it belongs to (`model_of`), or itself, once
/// each, in scene order. Of a support mesh only what stands above
/// `raft_top_mm` counts, and the one raft for all not at all.
pub(crate) fn past_on(
    spatial: &SceneSpatial,
    volume: &BuildVolume,
    plates: Plates,
    model_of: &dyn Fn(NodeId) -> Option<NodeId>,
    raft_top_mm: f32,
    only: Option<u32>,
    reach: Reach,
) -> Vec<NodeId> {
    let (half_x, half_y) = (volume.width_mm() / 2.0, volume.depth_mm() / 2.0);
    let top = volume.height_mm();
    let mut past = Vec::new();
    for leaf in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
        let Some(mesh) = spatial.graph().leaf_mesh_arc(leaf) else {
            continue;
        };
        let supports = sc_geom::supports::is_any_support_mesh(mesh);
        if mesh.faces().is_empty()
            || (reach == Reach::Supports && !supports)
            || sc_geom::supports::is_plate_raft(mesh)
        {
            continue;
        }
        let Some(query) = spatial.leaf(leaf) else {
            continue;
        };
        let world = query.world_transform();
        let reached = if supports {
            above_the_raft(mesh, &world, raft_top_mm)
        } else {
            query
                .transformed_aabb(world)
                .map(|aabb| (aabb.min().coords(), aabb.max().coords()))
        };
        let Some((min, max)) = reached else {
            continue;
        };
        let owner = model_of(leaf).unwrap_or(leaf);
        let Some(plate) =
            crate::plate_layout::plate_holding(spatial, volume, plates, leaf, model_of)
        else {
            // Past every plate: on none of several, so no plate's slice
            // holds it; with one plate the whole scene is sliced, and it
            // sticks out of it.
            if (only.is_none() || plates.count() <= 1) && !past.contains(&owner) {
                past.push(owner);
            }
            continue;
        };
        if only.is_some_and(|only| only != plate) {
            continue;
        }
        let centre = crate::plates::offset(volume, plate);
        let mut out = min[0] < centre.x - half_x - PAST_MM
            || max[0] > centre.x + half_x + PAST_MM
            || min[1] < centre.y - half_y - PAST_MM
            || max[1] > centre.y + half_y + PAST_MM;
        if reach == Reach::Everything {
            out |= max[2] > top + PAST_MM;
        }
        if out && !past.contains(&owner) {
            past.push(owner);
        }
    }
    past
}

/// How far a support mesh reaches above its raft, in the world: the box of
/// its points over `raft_top_mm` (a hair over, so the raft's top face stays
/// out). `None` when nothing stands above it.
fn above_the_raft(
    mesh: &sc_mesh_core::IndexedMesh,
    world: &sc_mesh_core::transform::Transform3D,
    raft_top_mm: f32,
) -> Option<([f32; 3], [f32; 3])> {
    let floor = raft_top_mm + RAFT_TOP_SLACK_MM;
    let mut reach: Option<([f32; 3], [f32; 3])> = None;
    for vertex in mesh.vertices() {
        let p = world.apply_to_vertex(*vertex).coords();
        if p[2] <= floor {
            continue;
        }
        let (min, max) = reach.get_or_insert((p, p));
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    reach
}

/// Over the raft's top this much before a support point counts (mm): the
/// raft's own top face is not a support standing on it.
const RAFT_TOP_SLACK_MM: f32 = 0.05;
