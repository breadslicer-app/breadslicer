//! The layout of a kickstand's column, and the sizes it takes from the
//! support settings.

use serde::{Deserialize, Serialize};

use super::types::KickstandPlacementLayout;
use bread_support_geom::geom::math_utils::{clamp, js_max, js_min};
use bread_support_model::constants::JOINT_DIAMETER_OFFSET_MM;
use bread_support_model::numeric_constraint::NumericConstraint;
use bread_support_model::settings::types::SupportSettings;

/// Height of the first column joint, as a share of the rise to the host.
pub const FIRST_JOINT_HEIGHT_RATIO: NumericConstraint =
    NumericConstraint::new(0.1, 0.75, 0.01, 0.35);
/// Height of the second column joint, as a share of the rise to the host.
pub const SECOND_JOINT_HEIGHT_RATIO: NumericConstraint =
    NumericConstraint::new(0.3, 0.95, 0.01, 0.72);
/// The least distance between two column joints, in mm.
pub const MIN_JOINT_SPACING_MM: NumericConstraint = NumericConstraint::new(0.1, 5.0, 0.1, 0.6);
/// The least distance kept between the top column joint and the host, in mm.
pub const MIN_TERMINAL_CLEARANCE_MM: NumericConstraint =
    NumericConstraint::new(0.1, 5.0, 0.1, 0.25);

/// Layout values to use instead of the defaults; each is brought into range.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KickstandLayoutOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_joint_height_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_joint_height_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_joint_spacing_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_terminal_clearance_mm: Option<f64>,
}

/// `value`, or the default when unset, brought into the range.
fn within(value: Option<f64>, range: NumericConstraint) -> f64 {
    clamp(value.unwrap_or(range.default_value), range.min, range.max)
}

/// The column layout: the defaults with any overrides, each in range, and the
/// second joint kept at least a step above the first.
pub fn resolve_kickstand_layout(
    overrides: Option<&KickstandLayoutOverrides>,
) -> KickstandPlacementLayout {
    let o = overrides.copied().unwrap_or_default();
    let first = within(o.first_joint_height_ratio, FIRST_JOINT_HEIGHT_RATIO);
    let second = within(o.second_joint_height_ratio, SECOND_JOINT_HEIGHT_RATIO);
    let lowest_second = js_min(
        SECOND_JOINT_HEIGHT_RATIO.max,
        first + SECOND_JOINT_HEIGHT_RATIO.step,
    );
    KickstandPlacementLayout {
        first_joint_height_ratio: first,
        second_joint_height_ratio: js_max(second, lowest_second),
        min_joint_spacing_mm: within(o.min_joint_spacing_mm, MIN_JOINT_SPACING_MM),
        min_terminal_clearance_mm: within(o.min_terminal_clearance_mm, MIN_TERMINAL_CLEARANCE_MM),
    }
}

/// The plate root a kickstand stands on, as `(diameter, disk height, cone
/// height)`: the same root a trunk gets.
pub fn get_kickstand_root_profile(settings: &SupportSettings) -> (f64, f64, f64) {
    let roots = &settings.roots;
    (
        roots.diameter_mm,
        roots.disk_height_mm,
        roots.cone_height_mm,
    )
}

/// The knot a kickstand makes on its host is a joint's width wider than the
/// host there.
pub fn get_kickstand_knot_diameter_mm(host_diameter_mm: f64) -> f64 {
    js_max(0.001, host_diameter_mm) + JOINT_DIAMETER_OFFSET_MM
}
