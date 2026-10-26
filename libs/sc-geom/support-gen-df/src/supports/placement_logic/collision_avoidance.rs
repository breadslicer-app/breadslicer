//! DragonFruit `src/supports/PlacementLogic/CollisionAvoidance.ts`.
//!
//! DragonFruit asks the model's shared distance field (`SDFCache`, from
//! `SDFCachePool`) and falls back to BVH whisker rays (`checkShaftCollision`)
//! only for a mesh with no `boundsTree`. SC always builds the field, so these
//! take the field directly, as [`CollisionField`]: the three queries
//! DragonFruit makes of an `SDFCache`. The `SDFCache` port implements it.

use crate::supports::types::Vec3;
use crate::three::math_utils::{js_max, js_min, js_round, lerp};

const DEFAULT_FRUSTUM_SEGMENT_COUNT: f64 = 5.0;
/// Safety margin used only for contact-cone body sampling.
const CONTACT_CONE_COLLISION_SAFETY_MM: f64 = 0.05;
/// Ignore the first tip-adjacent region so contact attachment is still allowed.
const CONTACT_CONE_TIP_IGNORE_MM: f64 = 0.25;
/// Extra margin for the first cone sample. The cone is tangent to the surface at
/// the tip, so the near-tip region is clear by construction — but the sphere
/// approximation only clears if the sample depth exceeds its radius. With a
/// fixed 0.25mm ignore the first sample's radius (~0.24mm) leaves 0.015mm of
/// clearance: pure SDF quantization noise, which flips to a false collision on
/// sloped geometry. Scale the ignore with the tip radius so the margin is robust.
const CONE_TIP_SDF_MARGIN_MM: f64 = 0.15;
/// Target sampling stride for contact-cone checks (adaptive with SDF cell size).
const CONTACT_CONE_COLLISION_SAMPLE_STEP_MM: f64 = 0.2;

/// The distance-field queries collision checks make (`SDFCache`, matrix refreshed).
pub trait CollisionField {
    /// Grid cell edge (mm) of the quantized cache.
    fn cell_size(&self) -> f64;
    /// Whether a capsule of `radius` around `start..end` comes within the mesh
    /// (adaptive sphere tracing over the quantized cell cache).
    fn segment_blocked(&self, start: Vec3, end: Vec3, radius: f64) -> bool;
    /// Exact signed distance to the mesh (negative inside), unbounded.
    fn exact_signed_distance_at(&self, point: Vec3) -> f64;

    /// Runs `job(0..jobs)` in parallel, each against its own throwaway view
    /// of this field, and keeps the expensive answers they computed for the
    /// real queries that follow until [`clear_prefetch`](Self::clear_prefetch).
    ///
    /// Purely a speed-up: the real queries stay sequential and in their own
    /// order, and a kept answer is exactly what they would have computed, so
    /// results never change however well the jobs guess. A field with nothing
    /// worth prefetching runs nothing.
    fn prefetch(&self, _jobs: usize, _job: &(dyn Fn(usize, &dyn CollisionField) + Sync)) {}

    /// Drops what [`prefetch`](Self::prefetch) kept.
    fn clear_prefetch(&self) {}
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionFrustumProfile {
    pub start_radius: f64,
    pub end_radius: f64,
    pub segment_count: Option<f64>,
}

fn segment_blocked_with_best_available_method(
    start: Vec3,
    end: Vec3,
    collision_radius: f64,
    field: &dyn CollisionField,
) -> bool {
    field.segment_blocked(start, end, collision_radius)
}

pub fn is_collision_segment_blocked(
    start: Vec3,
    end: Vec3,
    collision_radius: f64,
    field: &dyn CollisionField,
) -> bool {
    segment_blocked_with_best_available_method(start, end, collision_radius, field)
}

/// SDF-based shaft collision check — replaces BVH whisker-ray `checkShaftCollision`.
///
/// Uses `sdf.segmentBlocked()` with adaptive sphere tracing, which is more
/// accurate than the 9-ray bundle and shares its cell cache with the router.
///
/// `shaft_radius` is the shaft radius in mm (used as clearance margin).
pub fn is_shaft_blocked(
    start: Vec3,
    end: Vec3,
    shaft_radius: f64,
    field: &dyn CollisionField,
) -> bool {
    field.segment_blocked(start, end, shaft_radius)
}

/// DragonFruit's default `segmentCount` is 5.
pub fn is_collision_frustum_blocked(
    start: Vec3,
    end: Vec3,
    start_radius: f64,
    end_radius: f64,
    field: &dyn CollisionField,
    segment_count: Option<f64>,
) -> bool {
    let segment_count = segment_count.unwrap_or(DEFAULT_FRUSTUM_SEGMENT_COUNT);
    let normalized_start_radius = js_max(0.001, start_radius);
    let normalized_end_radius = js_max(0.001, end_radius);

    if (normalized_start_radius - normalized_end_radius).abs() <= 0.000001 {
        return segment_blocked_with_best_available_method(
            start,
            end,
            normalized_end_radius,
            field,
        );
    }

    let segment_total = js_max(1.0, js_round(segment_count));
    let segment_total_count = segment_total as usize;

    for i in 0..segment_total_count {
        let t0 = i as f64 / segment_total;
        let t1 = (i + 1) as f64 / segment_total;
        let seg_start = start.lerp(end, t0);
        let seg_end = start.lerp(end, t1);
        let radius0 = lerp(normalized_start_radius, normalized_end_radius, t0);
        let radius1 = lerp(normalized_start_radius, normalized_end_radius, t1);
        let slice_radius = js_max(radius0, radius1);

        if segment_blocked_with_best_available_method(seg_start, seg_end, slice_radius, field) {
            return true;
        }
    }

    false
}

/// The contact cone as a collision subject: disk surface in, socket out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactConeSweep {
    /// Where the cone leaves the contact disk (tip side, small radius).
    pub start: Vec3,
    /// The socket, where the cone meets the shaft (body side, large radius).
    pub end: Vec3,
    pub start_radius: f64,
    pub end_radius: f64,
}

/// The contact cone's own collision gate: does the cone body intersect the model?
///
/// `is_collision_frustum_blocked` slices the frustum and asks the quantized grid,
/// which is right for a shaft but wrong for a cone: the cone's safety margin
/// (0.05mm) is far below the grid's cell-centre substitution error
/// (~cellSize·√3/2), so geometry the cone actually clears reads as intersecting.
/// Sample the axis and ask for the exact signed distance instead, and skip the
/// tangent tip region — the cone is tangent to its own attachment surface, so
/// those samples are clear by construction.
///
/// Returns true when the cone body intersects the model.
pub fn is_contact_cone_blocked(sdf: &dyn CollisionField, cone: ContactConeSweep) -> bool {
    let dx = cone.end.x - cone.start.x;
    let dy = cone.end.y - cone.start.y;
    let dz = cone.end.z - cone.start.z;
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    if length <= 0.000001 {
        return false;
    }

    let min_step = js_max(
        js_min(CONTACT_CONE_COLLISION_SAMPLE_STEP_MM, sdf.cell_size() * 0.8),
        0.1,
    );
    let tip_ignore = js_max(
        CONTACT_CONE_TIP_IGNORE_MM,
        cone.start_radius + CONTACT_CONE_COLLISION_SAFETY_MM + CONE_TIP_SDF_MARGIN_MM,
    );
    let start_t = js_min(1.0, tip_ignore / length);
    // Sphere-trace the axis instead of stepping it at a fixed 0.1 mm.
    //
    // This gate cannot use the quantized cell cache (its margins are smaller
    // than the grid's substitution error), so every sample is a fresh BVH
    // query, and the router runs the gate for the straight cone plus up to 24
    // deviations. The field is 1-Lipschitz, so from a sample with distance `d`
    // every point within `d - r` of it is at least `r` from the mesh: advancing
    // by `d - maxRadius` therefore skips only points the fixed-step loop would
    // also have found clear, and the verdict is the same while the queries drop
    // by an order of magnitude on a cone that is in the open.
    //
    // `maxRadius` is the largest radius over the remaining span, since the cone
    // widens towards the socket; the socket itself is always sampled, because
    // it is where the cone is widest.
    let max_radius = cone.end_radius + CONTACT_CONE_COLLISION_SAFETY_MM;
    let mut t = start_t;
    loop {
        let radius = cone.start_radius
            + (cone.end_radius - cone.start_radius) * t
            + CONTACT_CONE_COLLISION_SAFETY_MM;
        // Unbounded query, so interior samples still sign negative: a cone
        // through solid material must stay detected.
        let distance = sdf.exact_signed_distance_at(Vec3::new(
            cone.start.x + dx * t,
            cone.start.y + dy * t,
            cone.start.z + dz * t,
        ));
        if distance < radius {
            return true;
        }
        if t >= 1.0 {
            break;
        }
        let advance_mm = js_max(distance - max_radius, min_step);
        t = js_min(1.0, t + advance_mm / length);
    }

    false
}

/// Calculates the required standoff distance (offset) from a surface to ensure
/// a connecting element (like a cone) does not collide with the mesh.
///
/// - `surface_pos` - The starting point on the model surface.
/// - `surface_normal` - The normal vector at the surface point (direction to extend).
/// - `target_pos` - The target position the element connects to (e.g., Socket).
/// - `collision_radius` - The radius of the element for collision checking (include safety margin).
/// - `field` - The model's distance field.
/// - `min_offset` - The minimum/starting offset to test.
/// - `max_offset` - The maximum allowable offset.
/// - `step` - The increment for iterative testing (DragonFruit default: 0.2mm).
///
/// Returns the calculated safe offset distance.
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
    let start = surface_pos;
    let normal = surface_normal.normalize();
    let normalized_step = js_max(0.025, step);
    let mut previous_blocked_offset = min_offset;

    let test_offset = |offset: f64| -> bool {
        // Calculate proposed start position: Surface + (Normal * t)
        let test_start = start.add(normal.multiply_scalar(offset));

        if let Some(frustum) = collision_frustum {
            return is_collision_frustum_blocked(
                test_start,
                target_pos,
                frustum.start_radius,
                frustum.end_radius,
                field,
                frustum.segment_count,
            );
        }

        segment_blocked_with_best_available_method(test_start, target_pos, collision_radius, field)
    };

    if !test_offset(min_offset) {
        return min_offset;
    }

    let mut t = min_offset + normalized_step;
    while t <= max_offset + 0.000001 {
        let clamped_offset = js_min(t, max_offset);
        if !test_offset(clamped_offset) {
            let mut low = previous_blocked_offset;
            let mut high = clamped_offset;

            for _ in 0..6 {
                let mid = (low + high) / 2.0;
                if test_offset(mid) {
                    low = mid;
                } else {
                    high = mid;
                }
            }

            return high;
        }

        previous_blocked_offset = clamped_offset;
        t += normalized_step;
    }

    max_offset
}
