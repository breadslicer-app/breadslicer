//! Brace directions in the plan view. A brace's axis is the angle of its
//! horizontal run folded into a half turn, so a brace from A to B and one
//! from B to A share it; a shaft held along two axes far enough apart cannot
//! sway in any direction.

use std::f64::consts::PI;

use support_geom::geom::math_utils::js_min;

/// `angle_rad` folded into `[0, π)`.
pub fn normalize_axis_angle_rad(angle_rad: f64) -> f64 {
    let folded = angle_rad % PI;
    if folded < 0.0 { folded + PI } else { folded }
}

/// The angle between two axes, in degrees, from 0 (parallel) to 90 (square).
pub fn axis_separation_deg(a_rad: f64, b_rad: f64) -> f64 {
    let apart = (a_rad - b_rad).abs();
    (js_min(apart, PI - apart) * 180.0) / PI
}

/// True when two of `axes` are at least `min_axis_separation_deg` apart.
pub fn has_qualified_two_axis_bracing(axes: &[f64], min_axis_separation_deg: f64) -> bool {
    axes.iter().enumerate().any(|(i, &a)| {
        axes[i + 1..]
            .iter()
            .any(|&b| axis_separation_deg(a, b) >= min_axis_separation_deg)
    })
}

/// How many more axes a shaft held along `axes` needs before it is held
/// along two: none, one (it has some, all too close together) or two.
pub fn additional_axes_needed_for_two_axis_bracing(
    axes: &[f64],
    min_axis_separation_deg: f64,
) -> u8 {
    match axes {
        _ if has_qualified_two_axis_bracing(axes, min_axis_separation_deg) => 0,
        [] => 2,
        _ => 1,
    }
}
