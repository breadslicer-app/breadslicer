//! The range, step and default of a numeric setting, and a value brought
//! into it.

use crate::geom::math_utils::{js_round, js_to_fixed_number};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericConstraint {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub default_value: f64,
    /// Whole numbers only.
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

/// `value` in the range (the default when it is missing or not finite),
/// on a step from the minimum, with as many decimals as the step has.
pub fn clamp_numeric(value: Option<f64>, c: NumericConstraint) -> f64 {
    let within = |v: f64| v.max(c.min).min(c.max);
    let v = within(value.filter(|v| v.is_finite()).unwrap_or(c.default_value));
    if c.integer {
        return js_round(v);
    }
    let on_step = c.min + js_round((v - c.min) / c.step) * c.step;
    let decimals = format!("{}", c.step).split('.').nth(1).map_or(0, str::len);
    within(js_to_fixed_number(on_step, decimals))
}
