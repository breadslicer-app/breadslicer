//! Reading a route's grid snap.

use super::trunk_route_types::{SnappedRouteValidity, TrunkRoute, TrunkRouteValidity};
use crate::supports::types::Vec3;

/// The key of the grid node the route snapped to, if any.
pub fn get_resolved_snapped_node_key(route: &TrunkRoute) -> Option<&str> {
    route.snapped.as_ref()?.snapped_node_key.as_deref()
}

/// Whether the route snapped to a grid node.
pub fn has_resolved_snapped_root(route: &TrunkRoute) -> bool {
    get_resolved_snapped_node_key(route).is_some()
}

/// Where the snapped root stands, or `fallback_root_pos` for an unsnapped
/// route.
pub fn get_resolved_snapped_root_pos(route: &TrunkRoute, fallback_root_pos: Vec3) -> Vec3 {
    route
        .snapped
        .as_ref()
        .map_or(fallback_root_pos, |s| s.snapped_root_pos)
}

pub fn get_resolved_snapped_validity(route: &TrunkRoute) -> Option<SnappedRouteValidity> {
    route.snapped.as_ref().map(|s| s.snapped_validity)
}

/// The snap validity that matches the route's own validity.
pub fn get_default_snapped_validity(route: &TrunkRoute) -> SnappedRouteValidity {
    match route.route.validity {
        TrunkRouteValidity::Valid => SnappedRouteValidity::Valid,
        TrunkRouteValidity::RouteInvalid => SnappedRouteValidity::InvalidAssisted,
        TrunkRouteValidity::HardInvalid => SnappedRouteValidity::HardInvalid,
    }
}
