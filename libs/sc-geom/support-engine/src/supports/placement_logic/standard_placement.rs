//! The plainest placement of a trunk: its tip cone on the surface and a
//! straight drop from the socket to the plate.

use serde::{Deserialize, Serialize};

use super::cone_axis_policy::{ConeAngleMode, ConeAxisPolicyInput, resolve_cone_axis_policy};
use crate::supports::settings::types::SupportSettings;
use crate::supports::tips::SupportTipProfile;
use crate::supports::tips::get_socket_position;
use crate::supports::tips::nib_length;
use crate::supports::types::{LimitationCode, Vec3, WarningCode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrunkPlacementInput {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub tip_profile: SupportTipProfile,
    /// Height where the root ends and shaft begins
    pub roots_top_z: f64,
    /// Small islands bypass construction joint
    pub is_small_island: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrunkPlacementResult {
    pub base_pos: Vec3,
    pub socket_pos: Vec3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsnapped_bottom_pos: Option<Vec3>,
    /// `snappedNodeKey?: string | null`. Absent and null behave alike (only a
    /// key is read), so both are `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapped_node_key: Option<String>,
    /// Set when the grid was dropped for this placement: the base is where the
    /// router put it, not on a node. Nothing downstream may snap it back, or the
    /// member the snap would need comes back with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_ignored: Option<bool>,
    /// List of all joints for multi-segment paths
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joints: Option<Vec<Vec3>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub construction_joints: Option<Vec<Vec3>>,
    /// Block placement
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
    /// Allow with warning
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<WarningCode>,
    /// Surface angle in degrees (0=Up, 90=Vert, 180=Down)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    /// Cone axis used for socket placement (may differ from surface normal)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone_axis: Option<Vec3>,
    /// True if the pathfinder stagnated (trapped in cavity). Signals that fallback searches are futile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagnated: Option<bool>,
    /// True if A* exhausted its expansion budget without reaching the goal.
    /// When true, the V1 raycast fallback is very unlikely to succeed and can be skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exhausted_budget: Option<bool>,
}

/// A trunk dropped straight down from the socket of its tip cone. The
/// surface decides: facing up (less than the cone mode's least angle from
/// up) it cannot be touched from below, an error; up to 115° from up it is a
/// wall a cone barely holds, a warning.
pub fn calculate_standard_placement(
    input: &TrunkPlacementInput,
    settings: &SupportSettings,
) -> TrunkPlacementResult {
    let (tip, normal, profile) = (input.tip_pos, input.tip_normal, input.tip_profile);
    let policy = resolve_cone_axis_policy(ConeAxisPolicyInput {
        surface_normal: normal,
        cone_angle_mode: settings
            .tip
            .cone_angle_mode
            .unwrap_or(ConeAngleMode::Normal),
        adaptive_cone_angle_offset_deg: Some(
            settings.tip.adaptive_cone_angle_offset_deg.unwrap_or(30.0),
        ),
    });
    let axis = policy.cone_axis;
    let from_up = policy.surface_angle_from_up_deg;
    let (error, warning) = if from_up < policy.min_allowed_surface_angle_from_up_deg {
        (Some(LimitationCode::AngleTooSteep), None)
    } else if from_up <= 115.0 {
        (None, Some(WarningCode::AngleVerticalWarning))
    } else {
        (None, None)
    };
    // A disk tip's nib stands the cone off the surface.
    let nib = if profile.is_disk() {
        nib_length(normal, axis, Some(&profile.as_disk_profile()))
    } else {
        0.0
    };
    let cone_start = Vec3::new(
        tip.x + normal.x * nib,
        tip.y + normal.y * nib,
        tip.z + normal.z * nib,
    );
    let socket_pos = get_socket_position(cone_start, axis, Some(&profile));
    let base_pos = Vec3::new(socket_pos.x, socket_pos.y, 0.0);
    TrunkPlacementResult {
        base_pos,
        socket_pos,
        unsnapped_bottom_pos: Some(base_pos),
        joints: Some(Vec::new()),
        construction_joints: Some(Vec::new()),
        error,
        warning,
        angle: Some(from_up),
        cone_axis: Some(axis),
        ..TrunkPlacementResult::default()
    }
}
