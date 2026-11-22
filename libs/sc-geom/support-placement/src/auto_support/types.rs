//! The candidate contacts the auto-support passes hand one another.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use support_geom::bvh::Side;
use support_model::model_mesh::ModelMesh;
use support_model::types::Vec3;

/// The model as the auto-support passes read it: its distance field and
/// rays, the side rays hit, and its faces painted to take no support.
#[derive(Clone, Copy, Debug)]
pub struct AutoSupportMesh<'a> {
    pub mesh: ModelMesh<'a>,
    pub side: Side,
    pub blocked: Option<&'a HashSet<usize>>,
}

/// An ordering from a difference: its sign, with a NaN difference equal.
pub fn js_compare(d: f64) -> std::cmp::Ordering {
    d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
}

/// What made a candidate.
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

/// A contact a support may be placed on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidatePoint {
    /// A stable id, from the island or rule that made it.
    pub id: String,
    /// Where it touches the model, world mm.
    pub tip_pos: Vec3,
    /// The surface normal there, smoothed.
    pub tip_normal: Vec3,
    /// The model this candidate belongs to.
    pub model_id: String,
    /// What made it.
    pub source: CandidateSource,
    /// The area of the region it holds, mm² (0 for a lone lowest point).
    pub island_area_mm2: f64,
    /// Its height above the plate, mm.
    pub z_height: f64,
    /// Placed first when higher.
    pub priority: f64,
    /// Its own tip contact diameter, mm; `None` takes the size's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip_diameter_mm: Option<f64>,
    /// It must stand as a trunk of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_point: Option<bool>,
    /// The zone size it takes (#360), an index into the rules' sizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_size: Option<usize>,
    /// It lies in a closed cavity of its model, sized for it (#632).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub enclosed: bool,
}
