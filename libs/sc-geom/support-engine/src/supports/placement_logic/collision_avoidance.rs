//! Collision checks against the model's distance field: shafts as capsules,
//! tapered pieces as stacks of capsules, tip cones sampled exactly, and how
//! far off a surface a piece must start to clear it.

use crate::geom::math_utils::{js_max, js_min, js_round, lerp};
use crate::supports::types::Vec3;

/// How many capsules a tapered piece is checked as, by default.
const TAPER_SLICES: f64 = 5.0;
/// The clearance a tip cone keeps from the model, mm.
const CONE_CLEARANCE_MM: f64 = 0.05;
/// The stretch next to the contact a cone check skips at the least, mm: the
/// cone touches its own surface there by design.
const CONE_SKIP_MM: f64 = 0.25;
/// Added to the skipped stretch over the tip's radius, so the first sample
/// clears the surface by more than the field's rounding, mm.
const CONE_SKIP_MARGIN_MM: f64 = 0.15;
/// The longest step between cone samples, mm (shorter on a fine field).
const CONE_STEP_MM: f64 = 0.2;

/// The queries collision checks make of a model's distance field.
pub trait CollisionField {
    /// The edge of the field's grid cells, mm.
    fn cell_size(&self) -> f64;
    /// Whether a capsule of `radius` round `start..end` reaches the model.
    fn segment_blocked(&self, start: Vec3, end: Vec3, radius: f64) -> bool;
    /// The exact signed distance to the model (negative inside), mm.
    fn exact_signed_distance_at(&self, point: Vec3) -> f64;

    /// Runs `job(0..jobs)` in parallel, each on a throwaway view of the
    /// field, and keeps the costly answers they found for the queries that
    /// follow, until [`clear_prefetch`](Self::clear_prefetch). Only a
    /// speed-up: a kept answer is the one the query would compute.
    fn prefetch(&self, _jobs: usize, _job: &(dyn Fn(usize, &dyn CollisionField) + Sync)) {}

    /// Drops what [`prefetch`](Self::prefetch) kept.
    fn clear_prefetch(&self) {}
}

impl std::fmt::Debug for dyn CollisionField + '_ {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CollisionField")
    }
}

/// A tapered piece's radius at each end and how many capsules it is checked
/// as (`None`: [`TAPER_SLICES`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionFrustumProfile {
    pub start_radius: f64,
    pub end_radius: f64,
    pub segment_count: Option<f64>,
}

/// Whether a shaft of `shaft_radius` from `start` to `end` reaches the model.
pub fn is_shaft_blocked(
    start: Vec3,
    end: Vec3,
    shaft_radius: f64,
    field: &dyn CollisionField,
) -> bool {
    field.segment_blocked(start, end, shaft_radius)
}

/// Whether a piece tapering from `start_radius` to `end_radius` reaches the
/// model: cut into `segment_count` capsules, each as wide as its wider end.
pub fn is_collision_frustum_blocked(
    start: Vec3,
    end: Vec3,
    start_radius: f64,
    end_radius: f64,
    field: &dyn CollisionField,
    segment_count: Option<f64>,
) -> bool {
    let (r0, r1) = (js_max(0.001, start_radius), js_max(0.001, end_radius));
    if (r0 - r1).abs() <= 0.000001 {
        return field.segment_blocked(start, end, r1);
    }
    let slices = js_max(1.0, js_round(segment_count.unwrap_or(TAPER_SLICES)));
    (0..slices as usize).any(|i| {
        let (a, b) = (i as f64 / slices, (i + 1) as f64 / slices);
        let radius = js_max(lerp(r0, r1, a), lerp(r0, r1, b));
        field.segment_blocked(start.lerp(end, a), start.lerp(end, b), radius)
    })
}

/// A tip cone as checked: from the contact disk to the socket, widening.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactConeSweep {
    /// Where the cone leaves the contact disk (its narrow end).
    pub start: Vec3,
    /// The socket, where the cone meets the shaft (its wide end).
    pub end: Vec3,
    pub start_radius: f64,
    pub end_radius: f64,
}

/// Whether a tip cone's body reaches the model, sampled along its axis with
/// exact distances (the grid's rounding is coarser than the cone's
/// clearance) and the stretch touching its own surface skipped.
///
/// Samples step by the room the last one found beyond the widest radius
/// left (the field is 1-Lipschitz, so nothing skipped can be closer), at
/// least a fixed step, and the socket is always sampled.
pub fn is_contact_cone_blocked(sdf: &dyn CollisionField, cone: ContactConeSweep) -> bool {
    let (dx, dy, dz) = (
        cone.end.x - cone.start.x,
        cone.end.y - cone.start.y,
        cone.end.z - cone.start.z,
    );
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    if length <= 0.000001 {
        return false;
    }
    let least_step = js_max(js_min(CONE_STEP_MM, sdf.cell_size() * 0.8), 0.1);
    let skip = js_max(
        CONE_SKIP_MM,
        cone.start_radius + CONE_CLEARANCE_MM + CONE_SKIP_MARGIN_MM,
    );
    let widest = cone.end_radius + CONE_CLEARANCE_MM;
    let mut t = js_min(1.0, skip / length);
    loop {
        let radius =
            cone.start_radius + (cone.end_radius - cone.start_radius) * t + CONE_CLEARANCE_MM;
        let distance = sdf.exact_signed_distance_at(Vec3::new(
            cone.start.x + dx * t,
            cone.start.y + dy * t,
            cone.start.z + dz * t,
        ));
        if distance < radius {
            return true;
        }
        if t >= 1.0 {
            return false;
        }
        t = js_min(1.0, t + js_max(distance - widest, least_step) / length);
    }
}

/// How far along `surface_normal` from `surface_pos` a piece reaching to
/// `target_pos` must start to clear the model: `min_offset` if it clears
/// there, else the first clear offset found stepping by `step` (at least
/// 0.025 mm), narrowed by halving six times, or `max_offset` if none clears.
/// The piece is a capsule of `collision_radius`, or tapered as
/// `collision_frustum` says.
#[allow(clippy::too_many_arguments)]
pub fn calculate_safe_offset(
    surface_pos: Vec3,
    surface_normal: Vec3,
    target_pos: Vec3,
    collision_radius: f64,
    field: &dyn CollisionField,
    min_offset: f64,
    max_offset: f64,
    step: f64,
    collision_frustum: Option<CollisionFrustumProfile>,
) -> f64 {
    let normal = surface_normal.normalize();
    let blocked = |offset: f64| {
        let from = surface_pos.add(normal.multiply_scalar(offset));
        match collision_frustum {
            Some(f) => is_collision_frustum_blocked(
                from,
                target_pos,
                f.start_radius,
                f.end_radius,
                field,
                f.segment_count,
            ),
            None => field.segment_blocked(from, target_pos, collision_radius),
        }
    };
    if !blocked(min_offset) {
        return min_offset;
    }
    let step = js_max(0.025, step);
    let mut last_blocked = min_offset;
    let mut t = min_offset + step;
    while t <= max_offset + 0.000001 {
        let offset = js_min(t, max_offset);
        if !blocked(offset) {
            let (mut low, mut high) = (last_blocked, offset);
            for _ in 0..6 {
                let mid = (low + high) / 2.0;
                if blocked(mid) {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            return high;
        }
        last_blocked = offset;
        t += step;
    }
    max_offset
}
