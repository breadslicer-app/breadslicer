//! DragonFruit `src/supports/PlacementLogic/Grid/gridMath.ts`.

use crate::three::math_utils::js_round;

pub fn snap_to_grid_index(value_mm: f64, spacing_mm: f64) -> f64 {
    if spacing_mm <= 0.0 {
        return 0.0;
    }
    let scaled = value_mm / spacing_mm;
    if scaled == 0.0 {
        return 0.0;
    }
    // Math.sign(scaled) * Math.round(Math.abs(scaled))
    let sign = if scaled > 0.0 {
        1.0
    } else if scaled < 0.0 {
        -1.0
    } else {
        scaled
    };
    sign * js_round(scaled.abs())
}

/// `${gx},${gy}`: integers print as integers, as JavaScript does.
pub fn grid_node_key_from_xy(x_mm: f64, y_mm: f64, spacing_mm: f64) -> String {
    let gx = snap_to_grid_index(x_mm, spacing_mm);
    let gy = snap_to_grid_index(y_mm, spacing_mm);
    format!("{},{}", js_number_to_string(gx), js_number_to_string(gy))
}

/// A grid index as `String(number)` writes it (indices are integers or NaN).
fn js_number_to_string(value: f64) -> String {
    if value == 0.0 {
        "0".to_owned()
    } else if value.is_nan() {
        "NaN".to_owned()
    } else {
        format!("{value}")
    }
}

/// The node's XY. Each part reads as `Number(part)`: a missing part is NaN,
/// an empty one 0, anything unparsable NaN.
pub fn grid_snapped_xy_from_key(key: &str, spacing_mm: f64) -> (f64, f64) {
    let mut parts = key.split(',');
    let parse = |part: Option<&str>| match part.map(str::trim) {
        None => f64::NAN,
        Some("") => 0.0,
        Some(p) => p.parse::<f64>().unwrap_or(f64::NAN),
    };
    let gx = parse(parts.next());
    let gy = parse(parts.next());
    (gx * spacing_mm, gy * spacing_mm)
}
