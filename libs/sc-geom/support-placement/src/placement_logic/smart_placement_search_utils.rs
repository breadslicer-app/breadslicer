//! Distances and leans of support segments, and how far a segment of a given
//! length may lean from vertical.

use std::f64::consts::PI;

use support_geom::geom::math_utils::{js_atan2, js_max, js_min};
use support_model::types::Vec3;

/// Distance between two points seen from above, mm.
pub fn distance_xy(a: Vec3, b: Vec3) -> f64 {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    (dx * dx + dy * dy).sqrt()
}

/// Distance between two points, mm.
pub fn distance_3d(a: Vec3, b: Vec3) -> f64 {
    let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// How steeply a segment going down from `start` to `end` falls, degrees
/// from vertical; a segment that does not go down is infinitely flat.
fn descent_lean_deg(start: Vec3, end: Vec3) -> f64 {
    let drop = start.z - end.z;
    if drop <= 0.0 {
        return f64::INFINITY;
    }
    let run = js_max(distance_xy(start, end), 0.0001);
    90.0 - js_atan2(drop, run) * (180.0 / PI)
}

/// The lean, degrees from vertical, of a member's shaft as it rises from its
/// host's knot to its first joint. A member leaves its host going up, so the
/// lean is measured on the rise, not on the drop.
pub fn member_departure_angle_from_vertical_deg(knot_pos: Vec3, first_joint_pos: Vec3) -> f64 {
    let lateral = distance_xy(knot_pos, first_joint_pos);
    (js_atan2(lateral, first_joint_pos.z - knot_pos.z) * 180.0) / PI
}

/// Whether a segment going down from `start` to `end` leans no more than
/// `max_angle_from_vertical_deg`.
pub fn segment_satisfies_max_angle_from_vertical(
    start: Vec3,
    end: Vec3,
    max_angle_from_vertical_deg: f64,
) -> bool {
    descent_lean_deg(start, end) <= max_angle_from_vertical_deg
}

/// Up to this length a segment may take the leaner detour angle below: a
/// short strut around a bump is sound at that lean, and no fixed cap can
/// route round an obstacle that close.
pub const SHORT_SPAN_DETOUR_MAX_LENGTH_MM: f64 = 3.0;
/// The lean a short segment may take, degrees from vertical.
pub const SHORT_SPAN_DETOUR_MAX_ANGLE_FROM_VERTICAL_DEG: f64 = 60.0;
/// From this length on, a segment's allowed lean shrinks with its length.
const TIGHTEN_FROM_MM: f64 = 5.0;
/// How many degrees the allowed lean loses per millimetre past that.
const TIGHTEN_DEG_PER_MM: f64 = 3.0;
/// The least lean a long segment is ever held to without a caller's floor.
const TIGHTEST_LEAN_DEG: f64 = 15.0;

/// A routed trunk's one diagonal leans this far from vertical: a contact
/// that a diagonal at this lean cannot reach takes a straight pillar instead
/// of a flatter strut.
pub const TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG: f64 = 45.0;

/// How far a span leans from vertical, whichever way it is walked; a level
/// span is infinitely flat.
pub fn span_lean_from_vertical_deg(start: Vec3, end: Vec3) -> f64 {
    let lateral = distance_xy(start, end);
    let vertical = (end.z - start.z).abs();
    if vertical <= 1e-6 {
        return f64::INFINITY;
    }
    (js_atan2(lateral, vertical) * 180.0) / PI
}

/// The lean, degrees from vertical, a segment of `segment_length_mm` may
/// take: the detour lean when short, easing to `base_max_angle_from_vertical_deg`
/// by [`TIGHTEN_FROM_MM`], then tighter the longer it is (a long slanted
/// shaft sags), never below `floor_max_angle_from_vertical_deg` (or
/// [`TIGHTEST_LEAN_DEG`]).
pub fn get_length_aware_max_angle_from_vertical_deg(
    segment_length_mm: f64,
    base_max_angle_from_vertical_deg: f64,
    floor_max_angle_from_vertical_deg: Option<f64>,
) -> f64 {
    let base = base_max_angle_from_vertical_deg;
    let detour = js_max(base, SHORT_SPAN_DETOUR_MAX_ANGLE_FROM_VERTICAL_DEG);
    if segment_length_mm <= SHORT_SPAN_DETOUR_MAX_LENGTH_MM {
        return detour;
    }
    if segment_length_mm <= TIGHTEN_FROM_MM {
        let t = (segment_length_mm - SHORT_SPAN_DETOUR_MAX_LENGTH_MM)
            / (TIGHTEN_FROM_MM - SHORT_SPAN_DETOUR_MAX_LENGTH_MM);
        return detour + (base - detour) * t;
    }
    let tightened = base - (segment_length_mm - TIGHTEN_FROM_MM) * TIGHTEN_DEG_PER_MM;
    js_max(
        floor_max_angle_from_vertical_deg.unwrap_or(TIGHTEST_LEAN_DEG),
        js_min(base, tightened),
    )
}

/// Whether a segment going down from `start` to `end` leans no more than its
/// length allows ([`get_length_aware_max_angle_from_vertical_deg`]).
pub fn segment_satisfies_length_aware_max_angle_from_vertical(
    start: Vec3,
    end: Vec3,
    base_max_angle_from_vertical_deg: f64,
    floor_max_angle_from_vertical_deg: Option<f64>,
) -> bool {
    let allowed = get_length_aware_max_angle_from_vertical_deg(
        distance_3d(start, end),
        base_max_angle_from_vertical_deg,
        floor_max_angle_from_vertical_deg,
    );
    segment_satisfies_max_angle_from_vertical(start, end, allowed)
}

/// The most a tip's elbow, where it turns from the socket into the shaft,
/// may lean from vertical.
pub const SOCKET_ELBOW_MAX_ANGLE_FROM_VERTICAL_DEG: f64 = 75.0;
