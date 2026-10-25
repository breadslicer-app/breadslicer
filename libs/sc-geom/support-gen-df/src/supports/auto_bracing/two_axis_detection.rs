//! DragonFruit `src/supports/autoBracing/twoAxisDetection.ts`.

use std::f64::consts::PI;

use crate::three::math_utils::js_min;

/// An axis angle folded into `[0, π)`.
pub fn normalize_axis_angle_rad(angle_rad: f64) -> f64 {
    let mut normalized = angle_rad % PI;
    if normalized < 0.0 {
        normalized += PI;
    }
    normalized
}

/// The angle between two axes, in degrees (0 … 90).
pub fn axis_separation_deg(a_rad: f64, b_rad: f64) -> f64 {
    let diff = (a_rad - b_rad).abs();
    (js_min(diff, PI - diff) * 180.0) / PI
}

pub fn has_qualified_two_axis_bracing(axes: &[f64], min_axis_separation_deg: f64) -> bool {
    for i in 0..axes.len() {
        for j in i + 1..axes.len() {
            if axis_separation_deg(axes[i], axes[j]) >= min_axis_separation_deg {
                return true;
            }
        }
    }
    false
}

pub fn additional_axes_needed_for_two_axis_bracing(
    axes: &[f64],
    min_axis_separation_deg: f64,
) -> u8 {
    if has_qualified_two_axis_bracing(axes, min_axis_separation_deg) {
        return 0;
    }
    if axes.is_empty() { 2 } else { 1 }
}
