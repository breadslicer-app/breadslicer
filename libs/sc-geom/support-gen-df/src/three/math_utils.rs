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
pub fn js_exp(x: f64) -> f64 {
    libm::exp(x)
}

#[inline]
pub fn js_tan(x: f64) -> f64 {
    libm::tan(x)
}

/// `MathUtils.degToRad`.
#[inline]
pub fn deg_to_rad(degrees: f64) -> f64 {
    degrees * DEG2RAD
}

/// `Math.hypot(...values)` as V8 computes it (`builtins/math.tq` `MathHypot`):
/// normalized to the largest magnitude, then Kahan-summed. Not
/// `sqrt(x*x + y*y)`, which rounds differently.
pub fn js_hypot(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut one_arg_is_nan = false;
    let mut max = 0.0_f64;
    for value in values {
        if value.is_nan() {
            one_arg_is_nan = true;
        } else if value.abs() > max {
            max = value.abs();
        }
    }
    if max == f64::INFINITY {
        return f64::INFINITY;
    } else if one_arg_is_nan {
        return f64::NAN;
    } else if max == 0.0 {
        return 0.0;
    }

    // Kahan summation to avoid rounding errors.
    // Normalize the numbers to the largest one to avoid overflow.
    let mut sum = 0.0_f64;
    let mut compensation = 0.0_f64;
    for value in values {
        let n = value.abs() / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}

/// `Number(x.toFixed(digits))` for `|x| < 1e21`.
///
/// `toFixed` sets the sign aside, picks the integer `n` nearest
/// `|x| * 10^digits` using the exact binary value, and on an exact tie the
/// larger `n` (away from zero). Rust's `{:.N}`
/// formats the same exact value but breaks a tie to even, so the rounding is
/// done here on the exact decimal expansion (`{:.N+40}` is exact far past
/// where a tie could hide for a double below 1e21).
pub fn js_to_fixed_number(x: f64, digits: usize) -> f64 {
    if !x.is_finite() || x.abs() >= 1e21 {
        return x;
    }
    let wide = format!("{:.*}", digits + 40, x.abs());
    let (kept, tail) = wide.split_at(wide.len() - 40);
    let first = tail.as_bytes()[0];
    let rest_zero = tail[1..].bytes().all(|b| b == b'0');
    let round_up_magnitude = if first > b'5' || (first == b'5' && !rest_zero) {
        true
    } else if first == b'5' {
        // An exact tie takes the larger n. `toFixed` rounds the magnitude (the
        // sign is set aside first), so a tie goes away from zero either way.
        true
    } else {
        false
    };
    let mut kept: Vec<u8> = kept.trim_end_matches('.').bytes().collect();
    if round_up_magnitude {
        // decimal increment of the last kept digit
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                break;
            }
            i -= 1;
            match kept[i] {
                b'.' => continue,
                b'9' => kept[i] = b'0',
                d => {
                    kept[i] = d + 1;
                    break;
                }
            }
        }
    }
    let magnitude: f64 = String::from_utf8(kept)
        .expect("ascii")
        .parse()
        .expect("decimal parses");
    if x < 0.0 { -magnitude } else { magnitude }
}
