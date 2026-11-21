//! DragonFruit `src/supports/PlacementLogic/StandardPlacement.ts`.

use serde::{Deserialize, Serialize};

use super::cone_axis_policy::{ConeAngleMode, ConeAxisPolicyInput, resolve_cone_axis_policy};
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_socket_position;
use crate::supports::support_primitives::contact_cone::types::SupportTipProfile;
use crate::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
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

/// Standard Placement Strategy:
/// - Base is placed directly below the socket (vertical drop).
/// - Ignores surface normal for direction (always vertical).
/// - Checks angle:
///   - < 90 deg (Upward): Error.
///   - 90-100 deg (Vertical/Horizontal): Warning.
///   - > 100 deg (Downward): OK.
pub fn calculate_standard_placement(
    input: &TrunkPlacementInput,
    settings: &SupportSettings,
) -> TrunkPlacementResult {
    let TrunkPlacementInput {
        tip_pos,
        tip_normal,
        tip_profile,
        ..
    } = *input;
    let mut error = None;
    let mut warning = None;

    // 0. Check Surface Angle
    // Normal points OUT of the mesh.
    // Up = (0,0,1).
    // Angle 0 = Facing Up.
    // Angle 90 = Vertical Wall.
    // Angle 180 = Facing Down.
    let cone_angle_mode = settings
        .tip
        .cone_angle_mode
        .unwrap_or(ConeAngleMode::Normal);
    let adaptive_cone_angle_offset_deg =
        settings.tip.adaptive_cone_angle_offset_deg.unwrap_or(30.0);

    let policy = resolve_cone_axis_policy(ConeAxisPolicyInput {
        surface_normal: tip_normal,
        cone_angle_mode,
        adaptive_cone_angle_offset_deg: Some(adaptive_cone_angle_offset_deg),
    });
    let cone_axis = policy.cone_axis;

    if policy.surface_angle_from_up_deg < policy.min_allowed_surface_angle_from_up_deg {
        // Upward facing (or effectively flat/up/vertical)
        error = Some(LimitationCode::AngleTooSteep);
    } else if policy.surface_angle_from_up_deg <= 115.0 {
        // Vertical wall (Horizontal normal) to 30-degree overhang -> Warning
        // "Horizontal angles are not good for holding up supports..."
        warning = Some(WarningCode::AngleVerticalWarning);
    }

    // 1. Calculate Socket Position (where shaft connects to cone)
    let mut disk_thickness = 0.0;
    if tip_profile.is_disk() {
        disk_thickness =
            calculate_disk_thickness(tip_normal, cone_axis, Some(&tip_profile.as_disk_profile()));
    }

    let cone_start_pos = Vec3::new(
        tip_pos.x + tip_normal.x * disk_thickness,
        tip_pos.y + tip_normal.y * disk_thickness,
        tip_pos.z + tip_normal.z * disk_thickness,
    );

    let socket_pos = get_socket_position(cone_start_pos, cone_axis, Some(&tip_profile));

    // 2. Calculate Base Position
    // Updated Behavior: Vertical drop from the SOCKET position.
    // This aligns the trunk center with the joint where the shaft meets the cone.
    let base_pos = Vec3::new(socket_pos.x, socket_pos.y, 0.0);

    TrunkPlacementResult {
        base_pos,
        socket_pos,
        unsnapped_bottom_pos: Some(base_pos),
        snapped_node_key: None,
        grid_ignored: None,
        joints: Some(Vec::new()),
        construction_joints: Some(Vec::new()),
        error,
        warning,
        angle: Some(policy.surface_angle_from_up_deg), // Verified
        cone_axis: Some(cone_axis),
        stagnated: None,
        exhausted_budget: None,
    }
}
