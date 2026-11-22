//! The data a kickstand is placed with and built into.

use serde::{Deserialize, Serialize};

use support_model::support_kinds::SupportTypeId;
/// The kickstand entity lives with the other support types; it is named here
/// too so everything about kickstands can be imported from one place.
pub use support_model::types::Kickstand;
use support_model::types::{Knot, LimitationCode, Roots, Vec3};

/// The point on another support that a kickstand props up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KickstandHostTarget {
    pub segment_id: String,
    /// One of the kickstand host types.
    pub support_kind: SupportTypeId,
    /// How far along the segment, 0 at its bottom and 1 at its top.
    pub t: f64,
    pub pos: Vec3,
    pub diameter_mm: f64,
    /// The lowest `t` the host point may slide to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_t: Option<f64>,
}

/// Where the column's joints go, and the gaps kept between them.
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
    /// Why the column may not stand where it is, when it may not. A forced
    /// placement keeps it anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
}
