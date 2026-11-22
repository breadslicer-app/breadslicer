//! The pillars' grid: snapping a position to its node, and the text key a
//! node is known by (`"gx,gy"`, its indices on the grid).

use support_geom::geom::math_utils::js_round;

/// The grid index nearest `value_mm` at `spacing_mm`, halves rounded away
/// from zero; 0 for a spacing that is not positive.
pub fn snap_to_grid_index(value_mm: f64, spacing_mm: f64) -> f64 {
    if spacing_mm <= 0.0 {
        return 0.0;
    }
    let scaled = value_mm / spacing_mm;
    if scaled > 0.0 {
        js_round(scaled)
    } else if scaled < 0.0 {
        -js_round(-scaled)
    } else {
        // Zero (of either sign) and NaN stay as they are.
        if scaled == 0.0 { 0.0 } else { scaled }
    }
}

/// An index as written in a node key: whole numbers without a decimal
/// point, `NaN` and `±Infinity` by name.
fn index_text(value: f64) -> String {
    if value == 0.0 {
        "0".to_owned()
    } else if value.is_nan() {
        "NaN".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        format!("{value}")
    }
}

/// The key of the node at indices (`gx`, `gy`).
pub(crate) fn node_key(gx: f64, gy: f64) -> String {
    format!("{},{}", index_text(gx), index_text(gy))
}

/// The indices a node key names: a missing index reads NaN, an empty one 0,
/// one that is not a number NaN.
pub(crate) fn key_indices(key: &str) -> (f64, f64) {
    let mut parts = key.split(',');
    let mut next = || match parts.next().map(str::trim) {
        None => f64::NAN,
        Some("") => 0.0,
        Some(p) => p.parse::<f64>().unwrap_or(f64::NAN),
    };
    let gx = next();
    (gx, next())
}

/// The key of the node nearest (`x_mm`, `y_mm`).
pub fn grid_node_key_from_xy(x_mm: f64, y_mm: f64, spacing_mm: f64) -> String {
    node_key(
        snap_to_grid_index(x_mm, spacing_mm),
        snap_to_grid_index(y_mm, spacing_mm),
    )
}

/// Where the node of `key` stands, mm.
pub fn grid_snapped_xy_from_key(key: &str, spacing_mm: f64) -> (f64, f64) {
    let (gx, gy) = key_indices(key);
    (gx * spacing_mm, gy * spacing_mm)
}
