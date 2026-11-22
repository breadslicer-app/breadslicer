//! Cubic curves a shaft or a brace may follow between its ends.

use crate::types::Vec3;

/// The point `t` (0 to 1) along the curve from `p0` to `p3` drawn toward
/// `p1` and `p2`.
pub fn get_bezier_point_at_t(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f64) -> Vec3 {
    let s = 1.0 - t;
    let weights = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    p0.multiply_scalar(weights[0])
        .add(p1.multiply_scalar(weights[1]))
        .add(p2.multiply_scalar(weights[2]))
        .add(p3.multiply_scalar(weights[3]))
}

/// The curve split at `t` into the two curves that run along it, each as its
/// four points, by repeated halving of its control polygon.
pub fn subdivide_cubic_bezier(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f64) -> [[Vec3; 4]; 2] {
    let (a, b, c) = (p0.lerp(p1, t), p1.lerp(p2, t), p2.lerp(p3, t));
    let (d, e) = (a.lerp(b, t), b.lerp(c, t));
    let at = d.lerp(e, t);
    [[p0, a, d, at], [at, e, c, p3]]
}
