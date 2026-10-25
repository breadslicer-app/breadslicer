//! DragonFruit `src/supports/SupportTypes/Kickstand/kickstandSettings.ts`.
//!
//! Settings are passed in (`getSettings()`).

use serde::{Deserialize, Serialize};

use super::types::KickstandPlacementLayout;
use crate::supports::constants::JOINT_DIAMETER_OFFSET_MM;
use crate::supports::settings::types::SupportSettings;
use crate::three::math_utils::{clamp, js_max, js_min};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericConstraint {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub default_value: f64,
}

pub struct KickstandLayoutConstraints {
    pub first_joint_height_ratio: NumericConstraint,
    pub second_joint_height_ratio: NumericConstraint,
    pub min_joint_spacing_mm: NumericConstraint,
    pub min_terminal_clearance_mm: NumericConstraint,
}

pub const KICKSTAND_LAYOUT_CONSTRAINTS: KickstandLayoutConstraints = KickstandLayoutConstraints {
    first_joint_height_ratio: NumericConstraint {
        min: 0.1,
        max: 0.75,
        step: 0.01,
        default_value: 0.35,
    },
    second_joint_height_ratio: NumericConstraint {
        min: 0.3,
        max: 0.95,
        step: 0.01,
        default_value: 0.72,
    },
    min_joint_spacing_mm: NumericConstraint {
        min: 0.1,
        max: 5.0,
        step: 0.1,
        default_value: 0.6,
    },
    min_terminal_clearance_mm: NumericConstraint {
        min: 0.1,
        max: 5.0,
        step: 0.1,
        default_value: 0.25,
    },
};

/// `Partial<KickstandPlacementLayout>`.
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

fn clamp_constraint(value: f64, constraint: NumericConstraint) -> f64 {
    clamp(value, constraint.min, constraint.max)
}

pub fn resolve_kickstand_layout(
    overrides: Option<&KickstandLayoutOverrides>,
) -> KickstandPlacementLayout {
    let c = &KICKSTAND_LAYOUT_CONSTRAINTS;
    let pick = |value: Option<f64>, constraint: NumericConstraint| {
        clamp_constraint(value.unwrap_or(constraint.default_value), constraint)
    };
    let first_joint_height_ratio = pick(
        overrides.and_then(|o| o.first_joint_height_ratio),
        c.first_joint_height_ratio,
    );
    let second_joint_height_ratio_raw = pick(
        overrides.and_then(|o| o.second_joint_height_ratio),
        c.second_joint_height_ratio,
    );
    let min_joint_spacing_mm = pick(
        overrides.and_then(|o| o.min_joint_spacing_mm),
        c.min_joint_spacing_mm,
    );
    let min_terminal_clearance_mm = pick(
        overrides.and_then(|o| o.min_terminal_clearance_mm),
        c.min_terminal_clearance_mm,
    );

    // Keep second joint above first joint in ratio space.
    let min_second = js_min(
        c.second_joint_height_ratio.max,
        first_joint_height_ratio + c.second_joint_height_ratio.step,
    );

    KickstandPlacementLayout {
        first_joint_height_ratio,
        second_joint_height_ratio: js_max(second_joint_height_ratio_raw, min_second),
        min_joint_spacing_mm,
        min_terminal_clearance_mm,
    }
}

pub fn get_kickstand_body_diameter_mm(settings: &SupportSettings) -> f64 {
    settings.shaft.diameter_mm
}

/// `{ diameter, diskHeight, coneHeight }` of a kickstand's plate root.
pub fn get_kickstand_root_profile(settings: &SupportSettings) -> (f64, f64, f64) {
    (
        settings.roots.diameter_mm,
        settings.roots.disk_height_mm,
        settings.roots.cone_height_mm,
    )
}

pub fn get_kickstand_knot_diameter_mm(host_diameter_mm: f64) -> f64 {
    js_max(0.001, host_diameter_mm) + JOINT_DIAMETER_OFFSET_MM
}

pub const KICKSTAND_DEFAULT_HOST_OFFSET_MM: f64 = 4.0;

pub fn get_kickstand_placement_offset_mm() -> f64 {
    KICKSTAND_DEFAULT_HOST_OFFSET_MM
}
