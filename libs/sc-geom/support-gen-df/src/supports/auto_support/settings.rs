//! DragonFruit `src/supports/autoSupport/settings.ts`.

use serde::{Deserialize, Serialize};

use super::constants::MIN_LEAF_FAN_RADIUS_MM;
use super::zone_rules::ZoneRules;
use crate::supports::numeric_constraint::{NumericConstraint, clamp_numeric};

/// The seven numbers a run sizes every shaft, tip and root from — a Support
/// Studio preset's tip, shaft and roots, read as one band.
///
/// They are not the auto-support block's own data: `autoSupport.sizingPreset`
/// names the Support Studio preset the run borrows them from, and the engine
/// resolves that id on every run. The coupling is deliberate — see the backlog
/// entry — so editing a manual preset changes what auto-support prints.
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

/// The three factory Support Studio presets, whose bands `SIZING_BANDS` mirrors.
/// `presetForArea` still names the tier a supported area falls in for the forest
/// ledger, and the load budget weighs a support's cross-section against them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SizingPreset {
    Detail,
    Structure,
    Anchor,
}

impl SizingPreset {
    pub const ALL: [SizingPreset; 3] = [
        SizingPreset::Detail,
        SizingPreset::Structure,
        SizingPreset::Anchor,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SizingPreset::Detail => "detail",
            SizingPreset::Structure => "structure",
            SizingPreset::Anchor => "anchor",
        }
    }

    /// The factory bands, mirroring what the three factory Support Studio presets
    /// carry (detail / structure / anchor). Two consumers: the auto-support built-in
    /// presets name these tiers, and the *fallback* `activeSizingBand()` uses when the
    /// id in the settings names no preset at all (`structure` is the baseline).
    pub const fn band(self) -> SizingBand {
        match self {
            SizingPreset::Detail => SizingBand {
                shaft_diameter_mm: 0.8,
                tip_contact_diameter_mm: 0.22,
                tip_length_mm: 2.5,
                tip_penetration_mm: 0.0,
                root_diameter_mm: 2.0,
                root_disk_height_mm: 0.5,
                root_cone_height_mm: 1.0,
            },
            SizingPreset::Structure => SizingBand {
                shaft_diameter_mm: 1.0,
                tip_contact_diameter_mm: 0.28,
                tip_length_mm: 2.5,
                tip_penetration_mm: 0.0,
                root_diameter_mm: 2.0,
                root_disk_height_mm: 0.5,
                root_cone_height_mm: 1.0,
            },
            SizingPreset::Anchor => SizingBand {
                shaft_diameter_mm: 1.4,
                tip_contact_diameter_mm: 0.4,
                tip_length_mm: 2.5,
                tip_penetration_mm: 0.0,
                root_diameter_mm: 2.3,
                root_disk_height_mm: 0.5,
                root_cone_height_mm: 1.0,
            },
        }
    }
}

/// The auto-support settings block. Field documentation is DragonFruit's;
/// see `autoSupport/settings.ts` for the calibration notes on each knob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoSupportSettings {
    /// SoulCrafted (#445): a run's fans read only the shafts in their reach. They
    /// take the same hosts; a refusal no longer tells how far the nearest one
    /// out of reach stands, which the port's parity fixtures read.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub near_fans: bool,
    /// SoulCrafted (#464): a branch an island point ran up from far below,
    /// with nothing hanging from it, is hung instead from the shaft nearest
    /// its tip when that is shorter, with the same contact: a long branch
    /// for a speck is more lever than hold.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rehang_island_branches: bool,
    /// SoulCrafted (#500): a candidate's tip is resolved on its own surface,
    /// never on whatever hangs above it (see `resolve_surface_normal`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tips_on_own_surface: bool,
    pub enabled: bool,
    pub min_island_area_mm2: f64,
    pub tip_influence_radius_mm: f64,
    pub prioritize_intersection: bool,
    /// Max combined leaves + branches that can attach to a single trunk.
    pub max_attachments_per_trunk: f64,
    /// Projected surface area each grid support carries (mm²) — the density knob.
    /// Grid spacing = √areaPerSupportMm2.
    pub area_per_support_mm2: f64,
    /// Overhang regions at or above this projected area (mm²) get a density
    /// grid; smaller regions get a single support.
    pub grid_area_threshold_mm2: f64,
    /// Surface angle from horizontal (deg) at and below which a face counts
    /// as an overhang needing supports. 0° = flat ceiling; the resin standard
    /// is 45°. Steeper faces are considered self-supporting.
    pub overhang_self_support_angle_deg: f64,
    /// Master multiplier over the preset sizing bands (shaft/tip/root).
    pub size_scale: f64,
    /// Grid spacing multiplier on flat ceilings (0°) — the densest case.
    pub flat_density_boost: f64,
    /// Grid spacing multiplier at the self-support angle — the sparsest case.
    pub slope_relax_factor: f64,
    /// Area-scaling exponent for flat-region density: spacing ∝ (threshold/area)^exp
    /// on the flat end. 0 disables area scaling.
    pub suction_area_exponent: f64,
    /// Percentage of each region's projected footprint the auto grid must
    /// cover before gap-fill stops (75–100).
    pub coverage_target_percent: f64,
    /// The Support Studio preset whose tip, shaft and roots numbers size this run.
    /// An open id — the three factory presets plus any preset the user made — and a
    /// value that no longer resolves (a deleted preset) falls back to the factory
    /// structure band rather than failing the run.
    pub sizing_preset: String,
    /// Leaf fanning: max horizontal reach from a trunk shaft (mm).
    pub leaf_fan_radius_mm: f64,
    /// Leaf fanning: max angle from vertical for a fan leaf (deg).
    pub leaf_fan_max_angle_deg: f64,
    /// Debug: color supports by placement origin in the scene instead of the model color.
    pub debug_support_origin_colors: bool,
    pub debug_skip_auto_bracing: bool,
    /// Add stabilization anchors for poses bearing on a point/edge (default on).
    pub stabilization_enabled: bool,
    /// Ring mesh minima (the first point of a section) with reinforcement
    /// contacts on their own flank (default on).
    pub minima_reinforcement_enabled: bool,
    /// Fraction of the local free width a contact tip may occupy so the
    /// rendered disc fits the feature it lands on. **Calibration, not a
    /// preference** (0.9 measured). Read by `applyContactTipCap`.
    pub tip_contact_margin_scale: f64,
    /// Least shaft diameter a hosted member takes, as a fraction of the host
    /// shaft it sprouts from. **Calibration, not a preference** (0.7 measured).
    /// Read by `memberShaftDiameterMm`.
    pub member_host_shaft_ratio: f64,
    /// Master switch for the run-level model-scale sizing (size × load ×
    /// height factors). Off pins every one of them to ×1.
    pub model_scale_enabled: bool,
    /// Ceiling of the run-level size factor. **Calibration, not a preference.**
    pub model_size_factor_cap: f64,
    /// Ceiling of the run-level load factor. **Calibration, not a preference.**
    pub model_load_factor_cap: f64,
    /// Ceiling of the height factor. **Calibration, not a preference.**
    pub height_factor_cap: f64,
    /// SoulCrafted (#360): a zone preset. When set, the run picks every
    /// support's size and spacing by zone and draws each size exactly
    /// (no model scaling, no tip floor); the density, sizing tier and
    /// overhang knobs above are not read for placement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone_rules: Option<ZoneRules>,
}

impl Default for AutoSupportSettings {
    fn default() -> Self {
        create_default_auto_support_settings()
    }
}

/// The block's diagnostic keys: switches a user flips to look *at* a run
/// (color supports by origin, skip bracing to read the forest), not run policy.
pub const DIAGNOSTIC_AUTO_SUPPORT_KEYS: [&str; 2] =
    ["debugSupportOriginColors", "debugSkipAutoBracing"];

pub mod auto_support_constraints {
    use super::MIN_LEAF_FAN_RADIUS_MM;
    use crate::supports::numeric_constraint::NumericConstraint;

    pub const MIN_ISLAND_AREA_MM2: NumericConstraint =
        NumericConstraint::new(0.01, 10.0, 0.01, 0.02);
    pub const TIP_INFLUENCE_RADIUS_MM: NumericConstraint =
        NumericConstraint::new(0.1, 10.0, 0.1, 0.5);
    pub const MAX_ATTACHMENTS_PER_TRUNK: NumericConstraint =
        NumericConstraint::integer(2.0, 50.0, 1.0, 12.0);
    pub const AREA_PER_SUPPORT_MM2: NumericConstraint =
        NumericConstraint::new(1.0, 30.0, 0.5, 10.0);
    pub const GRID_AREA_THRESHOLD_MM2: NumericConstraint =
        NumericConstraint::new(5.0, 200.0, 5.0, 25.0);
    pub const OVERHANG_SELF_SUPPORT_ANGLE_DEG: NumericConstraint =
        NumericConstraint::new(20.0, 75.0, 5.0, 45.0);
    pub const SIZE_SCALE: NumericConstraint = NumericConstraint::new(0.5, 2.0, 0.05, 1.0);
    pub const FLAT_DENSITY_BOOST: NumericConstraint = NumericConstraint::new(0.5, 1.0, 0.05, 0.7);
    pub const SLOPE_RELAX_FACTOR: NumericConstraint = NumericConstraint::new(1.0, 2.0, 0.1, 1.3);
    pub const SUCTION_AREA_EXPONENT: NumericConstraint =
        NumericConstraint::new(0.0, 0.4, 0.05, 0.15);
    pub const COVERAGE_TARGET_PERCENT: NumericConstraint =
        NumericConstraint::integer(75.0, 100.0, 5.0, 95.0);
    pub const LEAF_FAN_RADIUS_MM: NumericConstraint =
        NumericConstraint::new(MIN_LEAF_FAN_RADIUS_MM, 15.0, 0.5, MIN_LEAF_FAN_RADIUS_MM);
    pub const LEAF_FAN_MAX_ANGLE_DEG: NumericConstraint =
        NumericConstraint::integer(20.0, 80.0, 5.0, 45.0);
    pub const TIP_CONTACT_MARGIN_SCALE: NumericConstraint =
        NumericConstraint::new(0.4, 1.0, 0.05, 0.9);
    pub const MEMBER_HOST_SHAFT_RATIO: NumericConstraint =
        NumericConstraint::new(0.5, 1.0, 0.05, 0.7);
    pub const MODEL_SIZE_FACTOR_CAP: NumericConstraint =
        NumericConstraint::new(1.0, 2.0, 0.05, 1.45);
    pub const MODEL_LOAD_FACTOR_CAP: NumericConstraint =
        NumericConstraint::new(1.0, 2.0, 0.05, 1.3);
    pub const HEIGHT_FACTOR_CAP: NumericConstraint = NumericConstraint::new(1.0, 2.0, 0.05, 1.35);
}

pub fn create_default_auto_support_settings() -> AutoSupportSettings {
    use auto_support_constraints as c;
    AutoSupportSettings {
        near_fans: false,
        rehang_island_branches: false,
        tips_on_own_surface: false,
        enabled: true,
        min_island_area_mm2: c::MIN_ISLAND_AREA_MM2.default_value,
        tip_influence_radius_mm: c::TIP_INFLUENCE_RADIUS_MM.default_value,
        prioritize_intersection: false,
        max_attachments_per_trunk: c::MAX_ATTACHMENTS_PER_TRUNK.default_value,
        area_per_support_mm2: c::AREA_PER_SUPPORT_MM2.default_value,
        grid_area_threshold_mm2: c::GRID_AREA_THRESHOLD_MM2.default_value,
        overhang_self_support_angle_deg: c::OVERHANG_SELF_SUPPORT_ANGLE_DEG.default_value,
        size_scale: c::SIZE_SCALE.default_value,
        flat_density_boost: c::FLAT_DENSITY_BOOST.default_value,
        slope_relax_factor: c::SLOPE_RELAX_FACTOR.default_value,
        suction_area_exponent: c::SUCTION_AREA_EXPONENT.default_value,
        coverage_target_percent: c::COVERAGE_TARGET_PERCENT.default_value,
        sizing_preset: SizingPreset::Structure.as_str().to_owned(),
        leaf_fan_radius_mm: c::LEAF_FAN_RADIUS_MM.default_value,
        leaf_fan_max_angle_deg: c::LEAF_FAN_MAX_ANGLE_DEG.default_value,
        tip_contact_margin_scale: c::TIP_CONTACT_MARGIN_SCALE.default_value,
        member_host_shaft_ratio: c::MEMBER_HOST_SHAFT_RATIO.default_value,
        model_scale_enabled: true,
        model_size_factor_cap: c::MODEL_SIZE_FACTOR_CAP.default_value,
        model_load_factor_cap: c::MODEL_LOAD_FACTOR_CAP.default_value,
        height_factor_cap: c::HEIGHT_FACTOR_CAP.default_value,
        debug_support_origin_colors: false,
        debug_skip_auto_bracing: false,
        stabilization_enabled: true,
        minima_reinforcement_enabled: true,
        zone_rules: None,
    }
}

/// The tier a block names: any non-empty string is an id to resolve at run time
/// (a user's own preset is as valid as a factory one, and a deleted one is a
/// fallback rather than an error), so the only repair here is the default.
fn normalize_sizing_preset(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

/// `normalizeAutoSupportSettings`. Every field of a Rust value is present and
/// typed, so the type coercions of the TypeScript reduce to the numeric clamps
/// and the preset id repair. The legacy `sizingBand` migration
/// (`migrateLegacySizingPreset`) belongs to reading a stored payload and is
/// done by `migrate_legacy_sizing_preset` on the JSON.
pub fn normalize_auto_support_settings(source: &AutoSupportSettings) -> AutoSupportSettings {
    use auto_support_constraints as c;
    let defaults = create_default_auto_support_settings();
    let clamp = |value: f64, constraint: NumericConstraint| clamp_numeric(Some(value), constraint);
    AutoSupportSettings {
        near_fans: source.near_fans,
        rehang_island_branches: source.rehang_island_branches,
        tips_on_own_surface: source.tips_on_own_surface,
        enabled: source.enabled,
        min_island_area_mm2: clamp(source.min_island_area_mm2, c::MIN_ISLAND_AREA_MM2),
        tip_influence_radius_mm: clamp(source.tip_influence_radius_mm, c::TIP_INFLUENCE_RADIUS_MM),
        prioritize_intersection: source.prioritize_intersection,
        max_attachments_per_trunk: clamp(
            source.max_attachments_per_trunk,
            c::MAX_ATTACHMENTS_PER_TRUNK,
        ),
        area_per_support_mm2: clamp(source.area_per_support_mm2, c::AREA_PER_SUPPORT_MM2),
        grid_area_threshold_mm2: clamp(source.grid_area_threshold_mm2, c::GRID_AREA_THRESHOLD_MM2),
        overhang_self_support_angle_deg: clamp(
            source.overhang_self_support_angle_deg,
            c::OVERHANG_SELF_SUPPORT_ANGLE_DEG,
        ),
        size_scale: clamp(source.size_scale, c::SIZE_SCALE),
        flat_density_boost: clamp(source.flat_density_boost, c::FLAT_DENSITY_BOOST),
        slope_relax_factor: clamp(source.slope_relax_factor, c::SLOPE_RELAX_FACTOR),
        suction_area_exponent: clamp(source.suction_area_exponent, c::SUCTION_AREA_EXPONENT),
        coverage_target_percent: clamp(source.coverage_target_percent, c::COVERAGE_TARGET_PERCENT),
        sizing_preset: normalize_sizing_preset(&source.sizing_preset, &defaults.sizing_preset),
        leaf_fan_radius_mm: clamp(source.leaf_fan_radius_mm, c::LEAF_FAN_RADIUS_MM),
        leaf_fan_max_angle_deg: clamp(source.leaf_fan_max_angle_deg, c::LEAF_FAN_MAX_ANGLE_DEG),
        tip_contact_margin_scale: clamp(
            source.tip_contact_margin_scale,
            c::TIP_CONTACT_MARGIN_SCALE,
        ),
        member_host_shaft_ratio: clamp(source.member_host_shaft_ratio, c::MEMBER_HOST_SHAFT_RATIO),
        model_scale_enabled: source.model_scale_enabled,
        model_size_factor_cap: clamp(source.model_size_factor_cap, c::MODEL_SIZE_FACTOR_CAP),
        model_load_factor_cap: clamp(source.model_load_factor_cap, c::MODEL_LOAD_FACTOR_CAP),
        height_factor_cap: clamp(source.height_factor_cap, c::HEIGHT_FACTOR_CAP),
        debug_support_origin_colors: source.debug_support_origin_colors,
        debug_skip_auto_bracing: source.debug_skip_auto_bracing,
        stabilization_enabled: source.stabilization_enabled,
        minima_reinforcement_enabled: source.minima_reinforcement_enabled,
        zone_rules: source.zone_rules.as_ref().map(ZoneRules::clamped),
    }
}

/// The factory preset a band's seven numbers are, or `structure` when they match none.
fn sizing_preset_for_band(value: &serde_json::Value) -> SizingPreset {
    let Some(band) = value.as_object() else {
        return SizingPreset::Structure;
    };
    let matches = |preset: SizingPreset| {
        let expected = serde_json::to_value(preset.band()).expect("band serializes");
        let expected = expected.as_object().expect("band is an object");
        expected
            .iter()
            .all(|(field, number)| band.get(field).and_then(|v| v.as_f64()) == number.as_f64())
    };
    SizingPreset::ALL
        .into_iter()
        .find(|&p| matches(p))
        .unwrap_or(SizingPreset::Structure)
}

/// Rewrite a stored auto-support block off the removed `sizingBand` object: the
/// obsolete key is dropped and the factory preset whose band it matched is written
/// in its place (`structure` for a band the factory never shipped). Unknown keys
/// pass through untouched, because a preset payload carries keys this build cannot
/// name and must keep them.
pub fn migrate_legacy_sizing_preset(payload: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(sizing_band) = payload.shift_remove("sizingBand") {
        payload.insert(
            "sizingPreset".to_owned(),
            serde_json::Value::from(sizing_preset_for_band(&sizing_band).as_str()),
        );
    }
}
