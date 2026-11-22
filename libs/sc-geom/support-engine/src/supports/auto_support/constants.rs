//! Distances and angles the auto-support passes share.

/// A tip lower than this (mm) gets a small anchor instead of a trunk.
pub const ANCHOR_HEIGHT_THRESHOLD_MM: f64 = 5.0;

/// The longest leaf hung from a pillar's knot, mm.
pub const MAX_AUTO_LEAF_SPAN_MM: f64 = 2.5;

/// Past this span (mm) a member is a branch with a shaft of its own, not a
/// long tapering leaf.
pub const MAX_LEAF_SPAN_BEFORE_BRANCH_MM: f64 = 6.0;

/// How wide a tip's cover grows the higher above it (mm up, mm wide), a
/// line through these points and level past the last.
const COVER_GROWTH: [(f64, f64); 4] = [(0.0, 3.0), (3.9, 4.0), (15.0, 5.0), (40.0, 6.0)];

/// The radius a tip covers `diff_z_mm` above it, mm.
pub fn influence_radius_mm(diff_z_mm: f64) -> f64 {
    let z = crate::geom::math_utils::js_max(0.0, diff_z_mm);
    if z <= COVER_GROWTH[0].0 {
        return COVER_GROWTH[0].1;
    }
    for pair in COVER_GROWTH.windows(2) {
        let ((z0, r0), (z1, r1)) = (pair[0], pair[1]);
        if z <= z1 {
            return r0 + ((r1 - r0) * (z - z0)) / (z1 - z0);
        }
    }
    COVER_GROWTH[COVER_GROWTH.len() - 1].1
}

/// The longest cavity bridge kept, mm.
pub const MAX_CAVITY_BRIDGE_MM: f64 = 12.0;

/// How far (seen from above) a tip with no way down looks for a support to
/// hang from before it is bridged model to model, mm: the widest search, as
/// a bridge leaves a second mark on the model. The member's angle still has
/// to pass.
pub const CAVITY_FAN_RADIUS_MM: f64 = 12.0;

/// How far a contact in a closed cavity looks for a stick to hang from
/// (#632), mm: near enough that the shared stick stays stout.
pub const CAVITY_STICK_FAN_RADIUS_MM: f64 = 6.0;

/// How many members one stick in a cavity carries (#632).
pub const CAVITY_STICK_MAX_MEMBERS: f64 = 4.0;

/// A tip within this of a support's tip already counts as held, mm.
pub const ALREADY_SUPPORTED_RADIUS_MM: f64 = 3.0;

/// Without the grid, a candidate this near a trunk joins it, mm.
pub const GRIDLESS_MERGE_RADIUS_MM: f64 = 4.0;

/// How far short of the tip a member's quick check stops, mm: the contact
/// disk stands the cone off the surface.
pub const FAN_LINK_TIP_INSET_MM: f64 = 0.5;

/// How much a host's longest member counts against it (per mm) when a
/// candidate picks a host by distance.
pub const MERGE_HOST_LOAD_WEIGHT: f64 = 0.5;

/// The least reach of a hanging search, mm.
pub const MIN_LEAF_FAN_RADIUS_MM: f64 = 8.0;

/// How far from a shaft an island may be to hang from it, mm.
pub const LEAF_FAN_RADIUS_MM: f64 = MIN_LEAF_FAN_RADIUS_MM;

/// How far from a pillar of the grid, mm: close, so members do not sweep
/// across the pillars.
pub const GRID_HOST_FAN_RADIUS_MM: f64 = 2.5;

/// The most a hung leaf leans from vertical, degrees.
pub const LEAF_FAN_MAX_ANGLE_DEG: f64 = 45.0;

/// How far a lone overhang pillar's tip may be from the shaft it joins, mm
/// (the nearest shaft is taken).
pub const CONSOLIDATION_FAN_RADIUS_MM: f64 = 8.0;

/// The most a joining link leans from vertical, degrees: on a gentle slope
/// a neighbour's link is always that flat.
pub const CONSOLIDATION_MAX_ANGLE_DEG: f64 = 75.0;

/// Joining links routed as branches only above this height, mm.
pub const CONSOLIDATION_BRANCH_MIN_HEIGHT_MM: f64 = 10.0;

/// Surfaces flatter than this from horizontal (degrees) are overhangs.
pub const OVERHANG_SELF_SUPPORT_ANGLE_DEG: f64 = 45.0;
