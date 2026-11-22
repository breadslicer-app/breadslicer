//! The auto-support settings, and the size bands a run draws from.

use serde::{Deserialize, Serialize};

use super::constants::MIN_LEAF_FAN_RADIUS_MM;
use super::zone_rules::ZoneRules;
use crate::numeric_constraint::{NumericConstraint, clamp_numeric};

/// The seven numbers a support's tip, shaft and root are drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizingBand {
    pub shaft_diameter_mm: f64,
    pub tip_contact_diameter_mm: f64,
    pub tip_length_mm: f64,
    pub tip_penetration_mm: f64,
    pub root_diameter_mm: f64,
    pub root_disk_height_mm: f64,
    pub root_cone_height_mm: f64,
}

/// The three base sizes: what a region counts as by its area, and the band
/// an id that names no size falls back to (`structure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SizingPreset {
    Detail,
    Structure,
    Anchor,
}

impl SizingPreset {
    pub const fn as_str(self) -> &'static str {
        match self {
            SizingPreset::Detail => "detail",
            SizingPreset::Structure => "structure",
            SizingPreset::Anchor => "anchor",
        }
    }

    /// The base size's band.
    pub const fn band(self) -> SizingBand {
        let (shaft, tip, root) = match self {
            SizingPreset::Detail => (0.8, 0.22, 2.0),
            SizingPreset::Structure => (1.0, 0.28, 2.0),
            SizingPreset::Anchor => (1.4, 0.4, 2.3),
        };
        SizingBand {
            shaft_diameter_mm: shaft,
            tip_contact_diameter_mm: tip,
            tip_length_mm: 2.5,
            tip_penetration_mm: 0.0,
            root_diameter_mm: root,
            root_disk_height_mm: 0.5,
            root_cone_height_mm: 1.0,
        }
    }
}

/// How auto-support places.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoSupportSettings {
    /// The most leaves and branches one trunk carries.
    pub max_attachments_per_trunk: f64,
    /// The area each support holds, mm² (the load budget reads it).
    pub area_per_support_mm2: f64,
    /// Faces flatter than this from horizontal (degrees) are overhangs.
    pub overhang_self_support_angle_deg: f64,
    /// The id of the band supports are drawn from (a zone size's, or a base
    /// size); one that names nothing falls back to `structure`.
    pub sizing_preset: String,
    /// How far from a shaft a leaf may hang, mm.
    pub leaf_fan_radius_mm: f64,
    /// The most a hung leaf leans from vertical, degrees.
    pub leaf_fan_max_angle_deg: f64,
    /// No braces.
    pub debug_skip_auto_bracing: bool,
    /// The least shaft a member takes, as a share of its host's (measured).
    pub member_host_shaft_ratio: f64,
    /// Whether the model-wide size terms apply (the zone rules turn them off).
    pub model_scale_enabled: bool,
    /// The size term's cap.
    pub model_size_factor_cap: f64,
    /// The load term's cap.
    pub model_load_factor_cap: f64,
    /// The zone rules (#360): every support's size and spacing by the zone
    /// it lands in, each size drawn as written. Miniatures' by default.
    pub zone_rules: ZoneRules,
}

impl Default for AutoSupportSettings {
    fn default() -> Self {
        create_default_auto_support_settings()
    }
}

/// The bounds of the numbers above.
pub mod auto_support_constraints {
    use super::MIN_LEAF_FAN_RADIUS_MM;
    use crate::numeric_constraint::NumericConstraint;

    pub const MAX_ATTACHMENTS_PER_TRUNK: NumericConstraint =
        NumericConstraint::integer(2.0, 50.0, 1.0, 12.0);
    pub const AREA_PER_SUPPORT_MM2: NumericConstraint =
        NumericConstraint::new(1.0, 30.0, 0.5, 10.0);
    pub const OVERHANG_SELF_SUPPORT_ANGLE_DEG: NumericConstraint =
        NumericConstraint::new(20.0, 75.0, 5.0, 45.0);
    pub const LEAF_FAN_RADIUS_MM: NumericConstraint =
        NumericConstraint::new(MIN_LEAF_FAN_RADIUS_MM, 15.0, 0.5, MIN_LEAF_FAN_RADIUS_MM);
    pub const LEAF_FAN_MAX_ANGLE_DEG: NumericConstraint =
        NumericConstraint::integer(20.0, 80.0, 5.0, 45.0);
    pub const MEMBER_HOST_SHAFT_RATIO: NumericConstraint =
        NumericConstraint::new(0.5, 1.0, 0.05, 0.7);
    pub const MODEL_SIZE_FACTOR_CAP: NumericConstraint =
        NumericConstraint::new(1.0, 2.0, 0.05, 1.45);
    pub const MODEL_LOAD_FACTOR_CAP: NumericConstraint =
        NumericConstraint::new(1.0, 2.0, 0.05, 1.3);
}

pub fn create_default_auto_support_settings() -> AutoSupportSettings {
    use auto_support_constraints as c;
    AutoSupportSettings {
        max_attachments_per_trunk: c::MAX_ATTACHMENTS_PER_TRUNK.default_value,
        area_per_support_mm2: c::AREA_PER_SUPPORT_MM2.default_value,
        overhang_self_support_angle_deg: c::OVERHANG_SELF_SUPPORT_ANGLE_DEG.default_value,
        sizing_preset: SizingPreset::Structure.as_str().to_owned(),
        leaf_fan_radius_mm: c::LEAF_FAN_RADIUS_MM.default_value,
        leaf_fan_max_angle_deg: c::LEAF_FAN_MAX_ANGLE_DEG.default_value,
        member_host_shaft_ratio: c::MEMBER_HOST_SHAFT_RATIO.default_value,
        model_scale_enabled: true,
        model_size_factor_cap: c::MODEL_SIZE_FACTOR_CAP.default_value,
        model_load_factor_cap: c::MODEL_LOAD_FACTOR_CAP.default_value,
        debug_skip_auto_bracing: false,
        zone_rules: super::presets::minis_30().rules,
    }
}

/// The settings with every number in its bounds, and an empty band id made
/// the default.
pub fn normalize_auto_support_settings(source: &AutoSupportSettings) -> AutoSupportSettings {
    use auto_support_constraints as c;
    let clamp = |value: f64, bounds: NumericConstraint| clamp_numeric(Some(value), bounds);
    AutoSupportSettings {
        max_attachments_per_trunk: clamp(
            source.max_attachments_per_trunk,
            c::MAX_ATTACHMENTS_PER_TRUNK,
        ),
        area_per_support_mm2: clamp(source.area_per_support_mm2, c::AREA_PER_SUPPORT_MM2),
        overhang_self_support_angle_deg: clamp(
            source.overhang_self_support_angle_deg,
            c::OVERHANG_SELF_SUPPORT_ANGLE_DEG,
        ),
        sizing_preset: if source.sizing_preset.trim().is_empty() {
            SizingPreset::Structure.as_str().to_owned()
        } else {
            source.sizing_preset.clone()
        },
        leaf_fan_radius_mm: clamp(source.leaf_fan_radius_mm, c::LEAF_FAN_RADIUS_MM),
        leaf_fan_max_angle_deg: clamp(source.leaf_fan_max_angle_deg, c::LEAF_FAN_MAX_ANGLE_DEG),
        member_host_shaft_ratio: clamp(source.member_host_shaft_ratio, c::MEMBER_HOST_SHAFT_RATIO),
        model_scale_enabled: source.model_scale_enabled,
        model_size_factor_cap: clamp(source.model_size_factor_cap, c::MODEL_SIZE_FACTOR_CAP),
        model_load_factor_cap: clamp(source.model_load_factor_cap, c::MODEL_LOAD_FACTOR_CAP),
        debug_skip_auto_bracing: source.debug_skip_auto_bracing,
        zone_rules: source.zone_rules.clamped(),
    }
}
