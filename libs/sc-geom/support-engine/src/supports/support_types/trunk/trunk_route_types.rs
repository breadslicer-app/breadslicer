//! The route a trunk takes from its root to its contact.

use serde::{Deserialize, Serialize};

use crate::supports::types::{LimitationCode, Vec3, WarningCode};

/// Whether the trunk goes straight up or bends around the model.
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
    /// The trunk stands only with its base moved off the snapped node.
    RouteInvalid,
    /// The trunk cannot stand where it is.
    HardInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnappedRouteValidity {
    Valid,
    /// The base had to move off the node for the trunk to stand.
    InvalidAssisted,
    HardInvalid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrunkRouteResult {
    pub kind: TrunkRouteKind,
    pub base_pos: Vec3,
    pub socket_pos: Vec3,
    /// Where the base was before it was snapped to the grid.
    pub unsnapped_bottom_pos: Vec3,
    /// The bends of a routed trunk, bottom to top.
    pub joints: Vec<Vec3>,
    /// Joints added for the shaft's construction below the bends.
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
    /// The grid was set aside for this route: its base is routed, not snapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_ignored: Option<bool>,
}

/// The grid node a route's base snapped to. Present exactly when it snapped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnappedRoute {
    pub snapped_root_pos: Vec3,
    pub snapped_node_key: Option<String>,
    pub snapped_validity: SnappedRouteValidity,
}

/// A route, and the grid node its base snapped to when it did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrunkRoute {
    #[serde(flatten)]
    pub route: TrunkRouteResult,
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub snapped: Option<SnappedRoute>,
}
