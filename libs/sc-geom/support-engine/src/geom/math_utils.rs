/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Small numeric helpers. `js_max`, `js_min` and `clamp` let a NaN through
//! (a NaN operand yields NaN) instead of picking the other operand, so a bad
//! number shows instead of hiding.

/// The larger of `a` and `b`; NaN if either is NaN.
#[inline]
pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// The smaller of `a` and `b`; NaN if either is NaN.
#[inline]
pub fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// `value` kept between `min` and `max`; NaN stays NaN.
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

/// The nearest integer, a half rounding up (towards +infinity): -2.5 is -2.
#[inline]
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

#[inline]
pub fn rad_to_deg(radians: f64) -> f64 {
    radians * RAD2DEG
}

#[inline]
pub fn js_acos(x: f64) -> f64 {
    x.acos()
}

#[inline]
pub fn js_asin(x: f64) -> f64 {
    x.asin()
}

#[inline]
pub fn js_atan(x: f64) -> f64 {
    x.atan()
}

#[inline]
pub fn js_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

#[inline]
pub fn js_sin(x: f64) -> f64 {
    x.sin()
}

#[inline]
pub fn js_cos(x: f64) -> f64 {
    x.cos()
}

#[inline]
pub fn js_exp(x: f64) -> f64 {
    x.exp()
}

#[inline]
pub fn js_tan(x: f64) -> f64 {
    x.tan()
}

#[inline]
pub fn js_log(x: f64) -> f64 {
    x.ln()
}

#[inline]
pub fn js_cbrt(x: f64) -> f64 {
    x.cbrt()
}

#[inline]
pub fn js_pow(x: f64, y: f64) -> f64 {
    x.powf(y)
}

#[inline]
pub fn deg_to_rad(degrees: f64) -> f64 {
    degrees * DEG2RAD
}

/// The length of the vector `values`: NaN if one is NaN (unless one is
/// infinite, then infinity).
pub fn js_hypot(values: &[f64]) -> f64 {
    if values.iter().any(|v| v.is_infinite()) {
        return f64::INFINITY;
    }
    values.iter().map(|v| v * v).sum::<f64>().sqrt()
}

/// `x` rounded to `digits` decimals.
pub fn js_to_fixed_number(x: f64, digits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let scale = 10f64.powi(digits as i32);
    (x * scale).round() / scale
}

/// `x` written with `digits` decimals.
pub fn js_to_fixed(x: f64, digits: usize) -> String {
    format!("{x:.digits$}")
}
