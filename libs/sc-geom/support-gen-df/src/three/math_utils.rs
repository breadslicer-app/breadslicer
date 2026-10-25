//! three.js r181 `src/math/MathUtils.js` (MIT), the subset the support code calls.
//!
//! `clamp` and `lerp` keep JavaScript's `Math.max`/`Math.min` semantics: a NaN
//! operand yields NaN rather than the other operand.

/// `Math.max(a, b)`: NaN if either is NaN.
#[inline]
pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `Math.min(a, b)`: NaN if either is NaN.
#[inline]
pub fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

#[inline]
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    js_max(min, js_min(max, value))
}

#[inline]
pub fn lerp(x: f64, y: f64, t: f64) -> f64 {
    (1.0 - t) * x + t * y
}

pub const DEG2RAD: f64 = std::f64::consts::PI / 180.0;
pub const RAD2DEG: f64 = 180.0 / std::f64::consts::PI;
