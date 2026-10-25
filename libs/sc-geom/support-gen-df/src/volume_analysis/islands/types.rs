//! DragonFruit `src/volumeAnalysis/Islands/types.ts`: the detected island
//! auto-support is handed, with the wire shapes it carries.

use serde::{Deserialize, Serialize};

use super::voxel_footprint::VoxelFootprint;
use crate::supports::types::Vec3;

/// Which detector produced an island.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IslandSource {
    #[default]
    Voxel,
    Minima,
    Overhang,
}

/// Classification once the voxel and minima sets are intersected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IslandClass {
    Intersection,
    VoxelOnly,
    MinimaOnly,
}

/// A closed perimeter loop, `[x, y, z]` per point (world mm).
pub type Vec3Loop = Vec<[f64; 3]>;

/// Wire shape of the Rust `StabilityReport` (camelCase).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoseStabilityWire {
    pub bearing_area_mm2: f64,
    pub bearing_edges: f64,
    pub centroid_depth_mm: f64,
    pub contact_depth_mm: f64,
    pub drag_moment_mm3: f64,
    pub margin_mm: f64,
    pub adhesion_ratio: f64,
    pub push_dir_deg: f64,
    pub drag_top_mm: f64,
    pub bearing_is_raft: bool,
}

/// One island, from whichever detector found it. All coordinates are world /
/// build-plate space (Z-up, mm).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedIsland {
    /// Stable unique id across sources, e.g. "v12" (voxel) or "m7" (minima).
    pub id: String,
    pub source: IslandSource,
    /// Representative contact point (world mm).
    pub contact: Vec3,
    /// Z-ordering sort key, equal to `contact.z` (world mm).
    pub base_z: f64,
    /// Contact footprint area at the island's base layer (mm²).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_mm2: Option<f64>,
    /// Mean surface angle from horizontal (degrees), from the overhang detector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overhang_angle_deg: Option<f64>,
    /// Peel-drag moment this overhang patch carries (mm³).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drag_moment_mm3: Option<f64>,
    /// Direction its drag pushes the part (deg) — the side that lifts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drag_dir_deg: Option<f64>,
    /// 3D surface area of the patch (mm²).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_area_mm2: Option<f64>,
    /// Highest point of the patch (mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_z: Option<f64>,
    /// A large planar face past the self-support angle: a toppling patch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steep_flat: Option<bool>,
    /// Model-triangle indices of the overhang region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triangle_ids: Option<Vec<usize>>,
    /// Region mean surface normal (world space, away from the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_normal: Option<Vec3>,
    /// Inclusive [firstLayer, lastLayer] of the unsupported contact region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_span: Option<[f64; 2]>,
    /// Contact voxel positions at the base layer (overhang regions carry Z).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_voxels: Option<VoxelFootprint>,
    /// Triangle-accurate perimeter loops, inset by 0.25 mm (world mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perimeter_loops: Option<Vec<Vec3Loop>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertex_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_triangle_id: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<IslandClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fully_supported: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<DetectedIsland>>,
}

/// Default proximity (mm) for the "already supported" dedupe.
pub const SUPPORTED_RADIUS_MM: f64 = 0.3;
