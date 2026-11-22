//! An island as placing reads it.

use serde::{Deserialize, Serialize};

use super::voxel_footprint::VoxelFootprint;
use crate::native::islands::body::IslandBody;
use crate::supports::types::Vec3;

/// What found an island.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IslandSource {
    /// A layer that starts with nothing under it.
    #[default]
    Voxel,
    /// A low point of the surface.
    Minima,
    /// A face looking down past the overhang angle.
    Overhang,
}

/// Whether a layer island and a low point turned out to be one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IslandClass {
    Intersection,
    VoxelOnly,
    MinimaOnly,
}

/// A closed loop of points `[x, y, z]`, mm.
pub type Vec3Loop = Vec<[f64; 3]>;

/// One island, on the plate (Z up, mm).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedIsland {
    /// Unique across what found them: "v12", "m7", "o3".
    pub id: String,
    pub source: IslandSource,
    /// Where it is held.
    pub contact: Vec3,
    /// Its height, for order: the contact's.
    pub base_z: f64,
    /// Its area where it starts, mm².
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_mm2: Option<f64>,
    /// An overhang's mean angle from horizontal, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overhang_angle_deg: Option<f64>,
    /// An overhang's top, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_z: Option<f64>,
    /// An overhang's faces, numbered as the model's tree names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triangle_ids: Option<Vec<usize>>,
    /// An overhang's mean normal, away from the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_normal: Option<Vec3>,
    /// The first and last layer it stays bare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_span: Option<[f64; 2]>,
    /// The cells where it starts (an overhang's with their height).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_voxels: Option<VoxelFootprint>,
    /// An overhang's edge loops, 0.25 mm inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perimeter_loops: Option<Vec<Vec3Loop>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<IslandClass>,
    /// Its body, from a scan that measured it (#360).
    #[serde(skip)]
    pub body: Option<std::sync::Arc<IslandBody>>,
}
