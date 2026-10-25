//! DragonFruit `src/supports/SupportTypes/Trunk/trunkRouteTypes.ts`.

use serde::{Deserialize, Serialize};

use crate::supports::types::{LimitationCode, Vec3, WarningCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrunkRouteKind {
    Straight,
    Routed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrunkRouteValidity {
    Valid,
    RouteInvalid,
    HardInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnappedRouteValidity {
    Valid,
    InvalidAssisted,
    HardInvalid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrunkRouteResult {
    pub kind: TrunkRouteKind,
    pub base_pos: Vec3,
    pub socket_pos: Vec3,
    pub unsnapped_bottom_pos: Vec3,
    pub joints: Vec<Vec3>,
    pub construction_joints: Vec<Vec3>,
    pub validity: TrunkRouteValidity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<WarningCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone_axis: Option<Vec3>,
    /// The grid was dropped for this route: its base is routed, not snapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_ignored: Option<bool>,
}

/// `SnappedTrunkRouteResult extends TrunkRouteResult`: the snapped fields are
/// present exactly when the route snapped to a grid node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnappedRoute {
    pub snapped_root_pos: Vec3,
    pub snapped_node_key: Option<String>,
    pub snapped_validity: SnappedRouteValidity,
}

/// `TrunkRouteResult | SnappedTrunkRouteResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrunkRoute {
    #[serde(flatten)]
    pub route: TrunkRouteResult,
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub snapped: Option<SnappedRoute>,
}
