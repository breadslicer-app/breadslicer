//! DragonFruit `src/supports/SupportTypes/Trunk/trunkRouteResolution.ts`.

use super::trunk_route_types::{SnappedRouteValidity, TrunkRoute, TrunkRouteValidity};
use crate::supports::types::Vec3;

pub fn get_resolved_snapped_node_key(route: &TrunkRoute) -> Option<&str> {
    route
        .snapped
        .as_ref()
        .and_then(|s| s.snapped_node_key.as_deref())
}

pub fn has_resolved_snapped_root(route: &TrunkRoute) -> bool {
    get_resolved_snapped_node_key(route).is_some()
}

pub fn get_resolved_snapped_root_pos(route: &TrunkRoute, fallback_root_pos: Vec3) -> Vec3 {
    route
        .snapped
        .as_ref()
        .map_or(fallback_root_pos, |s| s.snapped_root_pos)
}

pub fn get_resolved_snapped_validity(route: &TrunkRoute) -> Option<SnappedRouteValidity> {
    route.snapped.as_ref().map(|s| s.snapped_validity)
}

pub fn get_default_snapped_validity(route: &TrunkRoute) -> SnappedRouteValidity {
    match route.route.validity {
        TrunkRouteValidity::HardInvalid => SnappedRouteValidity::HardInvalid,
        TrunkRouteValidity::RouteInvalid => SnappedRouteValidity::InvalidAssisted,
        TrunkRouteValidity::Valid => SnappedRouteValidity::Valid,
    }
}
