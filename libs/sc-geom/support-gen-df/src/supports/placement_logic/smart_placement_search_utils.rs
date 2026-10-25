//! DragonFruit `src/supports/PlacementLogic/smartPlacementSearchUtils.ts`.
//!
//! The geometry helpers. The search-node types and the A* queue ordering
//! (`queueSortValue`, `isBetterSearchState`) belong to the retired lattice
//! engine and have no caller in the V3 router, so they are not ported.

use std::f64::consts::PI;

use crate::supports::types::Vec3;
use crate::three::math_utils::{js_atan2, js_max, js_min, js_round};

const POSITION_KEY_MM: f64 = 0.5;

pub fn distance_xy(a: Vec3, b: Vec3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

pub fn distance_3d(a: Vec3, b: Vec3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

pub fn segment_angle_from_horizontal_deg(start: Vec3, end: Vec3) -> f64 {
    let horizontal = distance_xy(start, end);
    let vertical_drop = start.z - end.z;
    if vertical_drop <= 0.0 {
        return f64::NEG_INFINITY;
    }
    js_atan2(vertical_drop, js_max(horizontal, 0.0001)) * (180.0 / PI)
}

pub fn segment_angle_from_vertical_deg(start: Vec3, end: Vec3) -> f64 {
    let angle_from_horizontal = segment_angle_from_horizontal_deg(start, end);
    if !angle_from_horizontal.is_finite() {
        return f64::INFINITY;
    }
    90.0 - angle_from_horizontal
}

/// The angle a member's shaft leaves its host at, in degrees from vertical.
///
/// Rise-aware, unlike [`segment_angle_from_vertical_deg`], which describes a
/// *descending* chain segment and answers Infinity for anything that rises. A
/// branch leaves its knot going up, so the trunk-chain helper cannot measure it,
/// and a caller that reaches for it will refuse every member.
///
/// This is the gate that matters for attached members. The contact cone is
/// clamped toward the surface normal at the tip, so a member can satisfy a
/// knot-to-tip chord gate and still leave the host nearly level, bending into a
/// steep cone only at the tip: gate the shaft, not the chord.
pub fn member_departure_angle_from_vertical_deg(knot_pos: Vec3, first_joint_pos: Vec3) -> f64 {
    let lateral = distance_xy(knot_pos, first_joint_pos);
    (js_atan2(lateral, first_joint_pos.z - knot_pos.z) * 180.0) / PI
}

pub fn segment_satisfies_max_angle_from_vertical(
    start: Vec3,
    end: Vec3,
    max_angle_from_vertical_deg: f64,
) -> bool {
    segment_angle_from_vertical_deg(start, end) <= max_angle_from_vertical_deg
}

const LENGTH_AWARE_UPPER_SPAN_TIGHTEN_START_MM: f64 = 5.0;
const LENGTH_AWARE_UPPER_SPAN_MIN_MAX_ANGLE_FROM_VERTICAL_DEG: f64 = 15.0;
const LENGTH_AWARE_UPPER_SPAN_TIGHTEN_DEGREES_PER_MM: f64 = 3.0;
// Short-span detour slack: segments this short may bend up to the routing
// detour angle (the old A* exploration budget) even when the trunk's base
// angle is tighter. Local detours around bumps/crowns are geometrically
// impossible within the base trunk angle (the clearance envelope around an
// obstacle is locally steeper than any fixed cap), and a short strut segment
// at 60° is mechanically sound. Long spans keep the existing tightening.
pub const SHORT_SPAN_DETOUR_MAX_LENGTH_MM: f64 = 3.0;
pub const SHORT_SPAN_DETOUR_MAX_ANGLE_FROM_VERTICAL_DEG: f64 = 60.0;

/// Lean of a trunk's diagonal, degrees from vertical: the shape rule. A routed
/// trunk is one 45° diagonal and one joint, and it does not escalate to 60° or
/// 75° when no 45° leg reaches a clear column, because a flatter member reads as
/// a strut leaning off the model rather than a support. A contact the shape
/// cannot serve takes a pillar instead.
pub const TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG: f64 = 45.0;

/// Lean of a span, in degrees from vertical, measured on its vertical extent.
///
/// Direction-agnostic on purpose: a chain is passed socket-first by the router and
/// base-first by anything that reads geometry, and measuring the signed rise made
/// every descending span read as a violation. A span that is level with any
/// lateral offset returns Infinity, because that is as flat as a member gets.
/// Callers compare the result against the allowance for that span's length.
pub fn span_lean_from_vertical_deg(start: Vec3, end: Vec3) -> f64 {
    let lateral = distance_xy(start, end);
    let vertical = (end.z - start.z).abs();
    if vertical <= 1e-6 {
        return f64::INFINITY;
    }
    (js_atan2(lateral, vertical) * 180.0) / PI
}

/// The angle a segment of this length is allowed to sit at, from vertical.
///
/// Long spans tighten, because a long slanted shaft sags and shears. That
/// tightening exists to keep spans upright, not to forbid the angle the app
/// itself configures for routed trunks: a caller that knows that angle passes it
/// as `floor_max_angle_from_vertical_deg` (DragonFruit's default is 15°), and
/// the result is never stricter than it.
pub fn get_length_aware_max_angle_from_vertical_deg(
    segment_length_mm: f64,
    base_max_angle_from_vertical_deg: f64,
    floor_max_angle_from_vertical_deg: Option<f64>,
) -> f64 {
    let floor = floor_max_angle_from_vertical_deg
        .unwrap_or(LENGTH_AWARE_UPPER_SPAN_MIN_MAX_ANGLE_FROM_VERTICAL_DEG);
    let short_span_max_angle = js_max(
        base_max_angle_from_vertical_deg,
        SHORT_SPAN_DETOUR_MAX_ANGLE_FROM_VERTICAL_DEG,
    );

    if segment_length_mm <= SHORT_SPAN_DETOUR_MAX_LENGTH_MM {
        return short_span_max_angle;
    }

    if segment_length_mm <= LENGTH_AWARE_UPPER_SPAN_TIGHTEN_START_MM {
        // Taper from the detour slack back to the base angle.
        let t = (segment_length_mm - SHORT_SPAN_DETOUR_MAX_LENGTH_MM)
            / (LENGTH_AWARE_UPPER_SPAN_TIGHTEN_START_MM - SHORT_SPAN_DETOUR_MAX_LENGTH_MM);
        return short_span_max_angle
            + (base_max_angle_from_vertical_deg - short_span_max_angle) * t;
    }

    let excess_length = segment_length_mm - LENGTH_AWARE_UPPER_SPAN_TIGHTEN_START_MM;
    let tightened = base_max_angle_from_vertical_deg
        - excess_length * LENGTH_AWARE_UPPER_SPAN_TIGHTEN_DEGREES_PER_MM;
    js_max(floor, js_min(base_max_angle_from_vertical_deg, tightened))
}

pub fn segment_satisfies_length_aware_max_angle_from_vertical(
    start: Vec3,
    end: Vec3,
    base_max_angle_from_vertical_deg: f64,
    floor_max_angle_from_vertical_deg: Option<f64>,
) -> bool {
    let segment_length_mm = distance_3d(start, end);
    let allowed_max_angle = get_length_aware_max_angle_from_vertical_deg(
        segment_length_mm,
        base_max_angle_from_vertical_deg,
        floor_max_angle_from_vertical_deg,
    );
    segment_satisfies_max_angle_from_vertical(start, end, allowed_max_angle)
}

// Socket elbow: the FIRST chain segment below the contact-cone socket may form
// a short, steep "elbow" to dodge small obstacles directly under the tip —
// the standard shape mainstream mSLA slicers emit. Longer first segments fall
// back to the regular length-aware rule.
pub const SOCKET_ELBOW_MAX_LENGTH_MM: f64 = 2.5;
pub const SOCKET_ELBOW_MAX_ANGLE_FROM_VERTICAL_DEG: f64 = 75.0;

pub fn first_segment_satisfies_socket_elbow_max_angle(
    start: Vec3,
    end: Vec3,
    base_max_angle_from_vertical_deg: f64,
) -> bool {
    let segment_length_mm = distance_3d(start, end);
    if segment_length_mm <= SOCKET_ELBOW_MAX_LENGTH_MM {
        let allowed_max_angle = js_max(
            get_length_aware_max_angle_from_vertical_deg(
                segment_length_mm,
                base_max_angle_from_vertical_deg,
                None,
            ),
            SOCKET_ELBOW_MAX_ANGLE_FROM_VERTICAL_DEG,
        );
        return segment_satisfies_max_angle_from_vertical(start, end, allowed_max_angle);
    }
    segment_satisfies_length_aware_max_angle_from_vertical(
        start,
        end,
        base_max_angle_from_vertical_deg,
        None,
    )
}

pub fn chain_satisfies_length_aware_upper_span_rule(
    points: &[Vec3],
    base_max_angle_from_vertical_deg: f64,
) -> bool {
    if points.len() < 3 {
        return true;
    }
    points.windows(2).all(|pair| {
        segment_satisfies_length_aware_max_angle_from_vertical(
            pair[0],
            pair[1],
            base_max_angle_from_vertical_deg,
            None,
        )
    })
}

/// `${qx},${qy},${qz}` of the position on a 0.5 mm lattice.
pub fn position_key(pos: Vec3) -> String {
    let part = |v: f64| {
        let q = js_round(v / POSITION_KEY_MM);
        if q == 0.0 {
            "0".to_owned()
        } else if q.is_nan() {
            "NaN".to_owned()
        } else if q.is_infinite() {
            if q > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
        } else {
            format!("{q}")
        }
    };
    format!("{},{},{}", part(pos.x), part(pos.y), part(pos.z))
}
