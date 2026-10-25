//! DragonFruit `src/supports/SupportTypes/Kickstand/types.ts`.
//!
//! `KickstandBuildInput` carries the collision mesh and is ported with the
//! kickstand builder.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::supports::support_type_registry::SupportTypeId;
// Declared with the other support types; re-exported so this module stays the
// import site for everything kickstand-shaped.
pub use crate::supports::types::Kickstand;
use crate::supports::types::{Knot, LimitationCode, Roots, Vec3};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KickstandHostTarget {
    pub segment_id: String,
    /// One of `KICKSTAND_HOST_TYPES`.
    pub support_kind: SupportTypeId,
    pub t: f64,
    pub pos: Vec3,
    pub diameter_mm: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_t: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KickstandPlacementLayout {
    pub first_joint_height_ratio: f64,
    pub second_joint_height_ratio: f64,
    pub min_joint_spacing_mm: f64,
    pub min_terminal_clearance_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KickstandBuildResult {
    pub root: Roots,
    pub host_knot: Knot,
    pub kickstand: Kickstand,
    /// Why this column is illegal, when it is. Force placement ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KickstandState {
    pub kickstands: IndexMap<String, Kickstand>,
    pub roots: IndexMap<String, Roots>,
    pub knots: IndexMap<String, Knot>,
    pub selected_id: Option<String>,
}
