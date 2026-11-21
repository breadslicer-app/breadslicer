//! DragonFruit `src/supports/Curves/BezierUtils.ts`.
//!
//! `toVector3` / `toVec3` are identities here: `Vec3` is both the data record
//! and the math type.

use crate::supports::types::Vec3;
use crate::geom::math_utils::{js_max, js_min};

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
