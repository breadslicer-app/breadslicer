//! DragonFruit `src/utils/math.ts`: the numeric helpers (the viewport helper
//! is the GUI's).

use crate::three::math_utils::{js_max, js_min, js_round, js_to_fixed_number};

/// Constrain `value` to [`min`, `max`]. Resolves an inverted range (min > max)
/// in favour of `min`. (Not three's `MathUtils.clamp`, which favours `max`.)
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    js_min(max, js_max(min, value))
}

/// Snap `value` to a grid of `cell_size`, returning the integer cell index.
pub fn quantize_to_cell(value: f64, cell_size: f64) -> f64 {
    js_round(value / cell_size)
}

/// Round `value` to `scale` steps per unit (e.g. scale 1e5 keeps 5 decimals).
pub fn quantize_to_scale(value: f64, scale: f64) -> f64 {
    js_round(value * scale) / scale
}

/// `Number(value.toFixed(decimals))`: nearest, ties away from zero, on the
/// double's exact value.
pub fn round(value: f64, decimals: usize) -> f64 {
    js_to_fixed_number(value, decimals)
}
