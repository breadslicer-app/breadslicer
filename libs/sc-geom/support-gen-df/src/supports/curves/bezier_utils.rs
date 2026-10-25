//! DragonFruit `src/supports/Curves/BezierUtils.ts`.
//!
//! `toVector3` / `toVec3` are identities here: `Vec3` is both the data record
//! and the math type.

use crate::supports::types::Vec3;
use crate::three::math_utils::{js_max, js_min};

/// Calculates cubic Bezier control points to maintain C1 continuity (tangent matching).
///
/// `tension`: how "tight" the curve follows the tangents (0.1 = loose, 2.0 = tight).
/// DragonFruit's defaults are `tension = 0.5`, `bias = 0.5`.
/// Returns the two control points `[cp1, cp2]`.
pub fn calculate_bezier_control_points(
    start_pos: Vec3,
    end_pos: Vec3,
    start_tangent: Vec3,
    end_tangent: Vec3,
    tension: f64,
    bias: f64,
) -> [Vec3; 2] {
    let p0 = start_pos;
    let p3 = end_pos;
    let t0 = start_tangent.normalize();
    let t3 = end_tangent.normalize();

    // Calculate distance between endpoints
    let dist = p0.distance_to(p3);

    // Map input tension (0.1 - 2.0) to handle scale factor.
    // High Tension (2.0) = Short Handles = Sharp/Taut (Scale ~0.25)
    // Low Tension (0.1) = Long Handles = Flowy/Loose (Scale ~0.6)

    // Normalize tension roughly to 0..1 range for calculation, assuming input is 0..2
    let clamped_tension = js_max(0.0, js_min(tension, 2.0));
    let t = clamped_tension / 2.0; // 0..1

    // Invert logic: Higher T -> Lower Scale
    // Range: 0.6 (at t=0) down to 0.25 (at t=1)
    // Updated: Scaled down by ~30% to prevent overlapping handles (0.42 -> 0.175)
    let base_scale_factor = (0.6 - (t * 0.35)) * 0.7;

    // Apply Bias (0..1)
    // Low Bias (0) -> Bottom Tight (Short Handle), Top Loose (Long Handle)
    // High Bias (1) -> Bottom Loose (Long Handle), Top Tight (Short Handle)
    // Start Mult = Bias * 2 (0 -> 0, 0.5 -> 1, 1 -> 2)
    // End Mult = (1-Bias) * 2 (0 -> 2, 0.5 -> 1, 1 -> 0)
    let start_mult = bias * 2.0;
    let end_mult = (1.0 - bias) * 2.0;

    let start_handle_length = dist * base_scale_factor * start_mult;
    let end_handle_length = dist * base_scale_factor * end_mult;

    // Control Point 1: Start + (StartTangent * scale)
    let p1 = p0.add(t0.multiply_scalar(start_handle_length));

    // Control Point 2: End - (EndTangent * scale)
    let p2 = p3.sub(t3.multiply_scalar(end_handle_length));

    [p1, p2]
}

/// Samples a point on a cubic Bezier curve at parameter t [0..1]
pub fn get_bezier_point_at_t(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f64) -> Vec3 {
    // Cubic Bezier formula:
    // B(t) = (1-t)^3*P0 + 3(1-t)^2*t*P1 + 3(1-t)*t^2*P2 + t^3*P3
    let one_minus_t = 1.0 - t;

    let c0 = one_minus_t * one_minus_t * one_minus_t;
    let c1 = 3.0 * one_minus_t * one_minus_t * t;
    let c2 = 3.0 * one_minus_t * t * t;
    let c3 = t * t * t;

    p0.multiply_scalar(c0)
        .add(p1.multiply_scalar(c1))
        .add(p2.multiply_scalar(c2))
        .add(p3.multiply_scalar(c3))
}

/// Calculates the tangent vector (normalized) at parameter t [0..1]
pub fn get_bezier_tangent_at_t(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f64) -> Vec3 {
    // Derivative of Cubic Bezier:
    // B'(t) = 3(1-t)^2(P1-P0) + 6(1-t)t(P2-P1) + 3t^2(P3-P2)
    let one_minus_t = 1.0 - t;

    let d0 = p1.sub(p0).multiply_scalar(3.0 * one_minus_t * one_minus_t);
    let d1 = p2.sub(p1).multiply_scalar(6.0 * one_minus_t * t);
    let d2 = p3.sub(p2).multiply_scalar(3.0 * t * t);

    d0.add(d1).add(d2).normalize()
}

/// Calculates optimal resolution (number of segments) for a curve based on its approximate length and radius.
/// Ensures smoother curves for larger/longer segments while saving polys on small ones.
pub fn calculate_optimal_resolution(start_pos: Vec3, end_pos: Vec3, _radius: f64) -> f64 {
    let dist = start_pos.distance_to(end_pos);

    // e.g., 1 segment every 2mm?
    let segments = (dist / 2.0).ceil();

    // Clamp resolution
    const MIN_RES: f64 = 8.0;
    const MAX_RES: f64 = 64.0;

    js_max(MIN_RES, js_min(segments, MAX_RES))
}

fn distance3(a: Vec3, b: Vec3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Options for [`calculate_adaptive_bezier_resolution`]; `None` takes DragonFruit's default.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AdaptiveBezierResolutionOptions {
    pub min_resolution: Option<f64>,
    pub max_resolution: Option<f64>,
    pub target_chord_length_mm: Option<f64>,
    pub curvature_weight: Option<f64>,
}

pub fn calculate_adaptive_bezier_resolution(
    p0: Vec3,
    p1: Vec3,
    p2: Vec3,
    p3: Vec3,
    options: AdaptiveBezierResolutionOptions,
) -> f64 {
    let min_resolution = js_max(2.0, options.min_resolution.unwrap_or(4.0));
    let max_resolution = js_max(min_resolution, options.max_resolution.unwrap_or(24.0));
    let target_chord_length_mm = js_max(0.25, options.target_chord_length_mm.unwrap_or(2.5));
    let curvature_weight = js_max(0.5, options.curvature_weight.unwrap_or(2.0));

    let chord_length = distance3(p0, p3);
    let control_net_length = distance3(p0, p1) + distance3(p1, p2) + distance3(p2, p3);
    let curve_excess = js_max(0.0, control_net_length - chord_length);

    let length_driven_segments = (control_net_length / target_chord_length_mm).ceil();
    let curvature_driven_segments = (curve_excess * curvature_weight).ceil();
    let estimate = js_max(length_driven_segments, curvature_driven_segments + 2.0);

    js_max(min_resolution, js_min(max_resolution, estimate))
}

/// Subdivides a cubic Bezier curve at parameter t into two curves that exactly match the geometry.
/// Returns `[leftCurve, rightCurve]`, where each is `[p0, p1, p2, p3]`.
pub fn subdivide_cubic_bezier(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f64) -> [[Vec3; 4]; 2] {
    // De Casteljau's Algorithm
    let p01 = p0.lerp(p1, t);
    let p12 = p1.lerp(p2, t);
    let p23 = p2.lerp(p3, t);

    let p012 = p01.lerp(p12, t);
    let p123 = p12.lerp(p23, t);

    // The split point (should match point at t)
    let p0123 = p012.lerp(p123, t);

    [[p0, p01, p012, p0123], [p0123, p123, p23, p3]]
}

/// Converts a Bezier curve into a series of line segments for printing/export/collision.
pub fn bezier_to_line_segments(
    p0: Vec3,
    p1: Vec3,
    p2: Vec3,
    p3: Vec3,
    resolution: Option<f64>,
) -> Vec<Vec3> {
    let adaptive_resolution = calculate_adaptive_bezier_resolution(
        p0,
        p1,
        p2,
        p3,
        AdaptiveBezierResolutionOptions::default(),
    );
    let segments = js_max(2.0, resolution.unwrap_or(adaptive_resolution).floor());
    // A non-finite count cannot size an array in JavaScript either (RangeError).
    assert!(segments.is_finite(), "invalid bezier resolution {segments}");
    let segments = segments as usize;
    let mut points = Vec::with_capacity(segments + 1);

    for i in 0..=segments {
        let t = i as f64 / segments as f64;
        let one_minus_t = 1.0 - t;

        let c0 = one_minus_t * one_minus_t * one_minus_t;
        let c1 = 3.0 * one_minus_t * one_minus_t * t;
        let c2 = 3.0 * one_minus_t * t * t;
        let c3 = t * t * t;

        points.push(Vec3::new(
            p0.x * c0 + p1.x * c1 + p2.x * c2 + p3.x * c3,
            p0.y * c0 + p1.y * c1 + p2.y * c2 + p3.y * c3,
            p0.z * c0 + p1.z * c1 + p2.z * c2 + p3.z * c3,
        ));
    }

    points
}
