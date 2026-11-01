//! DragonFruit `src/supports/autoSupport/types.ts`: the candidate the passes
//! hand each other. The run's report and plan types are ported with the
//! orchestrator (`autoPlace.ts`).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::supports::model_mesh::ModelMesh;
use crate::supports::types::Vec3;
use crate::three_mesh_bvh::Side;

/// The model mesh as the auto-support passes read it: DragonFruit's
/// `THREE.Mesh` (distance field and BVH rays), the side its material raycasts
/// with, and the model's support-blocker mask (`getSupportBlockedTriangles`).
#[derive(Clone, Copy)]
pub struct AutoSupportMesh<'a> {
    pub mesh: ModelMesh<'a>,
    pub side: Side,
    pub blocked: Option<&'a HashSet<usize>>,
}

/// `(a, b) => d` as a sort comparator: V8's sort treats a NaN difference as
/// equal. Rust's `sort_by` is stable, as `Array.prototype.sort` is.
pub fn js_compare(d: f64) -> std::cmp::Ordering {
    d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
}

/// Which detector (or pass) produced a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateSource {
    Voxel,
    Minima,
    Intersection,
    Overhang,
    Stabilization,
    Reinforcement,
}

/// A single support placement candidate derived from island/minima detection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidatePoint {
    /// Unique stable id from the source DetectedIsland.
    pub id: String,
    /// Contact point on the model surface in world mm coordinates.
    pub tip_pos: Vec3,
    /// Surface normal at the contact point (world space, smoothed).
    pub tip_normal: Vec3,
    /// The model this candidate belongs to.
    pub model_id: String,
    /// Which detector produced this candidate.
    pub source: CandidateSource,
    /// Contact footprint area of the unsupported region (mm²). 0 for minima-only.
    pub island_area_mm2: f64,
    /// Z-height above build plate (mm).
    pub z_height: f64,
    /// Computed placement priority. Higher = place first.
    pub priority: f64,
    /// Per-point tip contact override (mm): small-island candidates and any
    /// candidate the free-width cap shrinks. `None` = the active band default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip_diameter_mm: Option<f64>,
    /// Density-grid point: must become its own standalone trunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_point: Option<bool>,
    /// SoulCrafted (#360): the zone rules' size this support takes (an index
    /// into the rules' sizes); `None` sizes it the usual way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_size: Option<usize>,
}
