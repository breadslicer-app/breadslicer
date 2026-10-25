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

/// `Math.round`: the nearest integer, ties toward +infinity (`Math.round(-2.5)`
/// is -2, where `f64::round` gives -3). Exact for every f64, including
/// 0.49999999999999994, which `floor(x + 0.5)` would round up.
#[inline]
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    let rounded = if x - floor >= 0.5 { floor + 1.0 } else { floor };
    // A negative input that rounds to zero keeps its sign, as in JavaScript.
    if rounded == 0.0 && x.is_sign_negative() {
        -0.0
    } else {
        rounded
    }
}

/// `MathUtils.radToDeg`.
#[inline]
pub fn rad_to_deg(radians: f64) -> f64 {
    radians * RAD2DEG
}

// JavaScript's transcendental functions, as V8 computes them, never the
// platform libm: `Math.sin`/`Math.cos` are glibc's (correctly rounded, see
// `crate::js_math`), the rest V8's fdlibm port, which `libm` (musl's fdlibm
// port) matches. A few ulps of an angle become many ulps of a position.

#[inline]
pub fn js_acos(x: f64) -> f64 {
    libm::acos(x)
}

#[inline]
pub fn js_asin(x: f64) -> f64 {
    libm::asin(x)
}

#[inline]
pub fn js_atan(x: f64) -> f64 {
    libm::atan(x)
}

#[inline]
pub fn js_atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

#[inline]
pub fn js_sin(x: f64) -> f64 {
    crate::js_math::sin(x)
}

#[inline]
pub fn js_cos(x: f64) -> f64 {
    crate::js_math::cos(x)
}

#[inline]
pub fn js_tan(x: f64) -> f64 {
    libm::tan(x)
}
