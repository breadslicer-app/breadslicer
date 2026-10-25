//! The `NumericConstraint` / `clampNumeric` / `precisionFromStep` helpers that
//! DragonFruit declares identically in `autoSupport/settings.ts` and
//! `autoBracing/settings.ts`; one copy here, used by both.

use crate::three::math_utils::{js_max, js_min, js_round, js_to_fixed_number};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericConstraint {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub default_value: f64,
    pub integer: bool,
}

impl NumericConstraint {
    pub const fn new(min: f64, max: f64, step: f64, default_value: f64) -> Self {
        NumericConstraint {
            min,
            max,
            step,
            default_value,
            integer: false,
        }
    }

    pub const fn integer(min: f64, max: f64, step: f64, default_value: f64) -> Self {
        NumericConstraint {
            min,
            max,
            step,
            default_value,
            integer: true,
        }
    }
}

/// `precisionFromStep`: the decimals in `String(step)`.
fn precision_from_step(step: f64) -> usize {
    // JavaScript's shortest round-trip form, as Rust's `{}` Display also gives
    // for these step values (none needs exponent notation).
    let text = format!("{step}");
    text.split('.').nth(1).map_or(0, str::len)
}

/// `clampNumeric(value, constraint)`: a non-finite (or absent) value takes the
/// default; then clamp, snap to the step grid and round to the step's decimals.
pub fn clamp_numeric(value: Option<f64>, constraint: NumericConstraint) -> f64 {
    let raw = value
        .filter(|v| v.is_finite())
        .unwrap_or(constraint.default_value);

    let clamped = js_min(constraint.max, js_max(constraint.min, raw));

    if constraint.integer {
        return js_round(clamped);
    }

    let steps_from_min = js_round((clamped - constraint.min) / constraint.step);
    let stepped = constraint.min + steps_from_min * constraint.step;
    let precision = precision_from_step(constraint.step);
    let rounded = js_to_fixed_number(stepped, precision);

    js_min(constraint.max, js_max(constraint.min, rounded))
}
