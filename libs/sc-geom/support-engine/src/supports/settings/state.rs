//! DragonFruit `src/supports/Settings/state.ts`, the normalization part.
//!
//! DragonFruit keeps one live settings object in module state, normalizes
//! every write through `mergeWithDefaults`/`clampProfileFields`, and every
//! builder reads it with `getSettings()`. Here the caller owns the value:
//! `set_settings` returns what DragonFruit's store would then hold, and the
//! builders take `&SupportSettings`. The per-group `update*` writers, the
//! listeners and `localStorage` persistence are the GUI's.

use super::defaults::*;
use super::types::{RoutingAlgorithm, SupportSettings};
use crate::supports::auto_bracing::settings::normalize_auto_bracing_settings;
use crate::supports::auto_support::settings::normalize_auto_support_settings;
use crate::three::math_utils::{js_max, js_min};

/// Clamp a field to its `SUPPORT_PROFILE_LIMITS` range; non-finite falls back.
fn clamp_to_range(value: Option<f64>, fallback: f64, range: FieldLimit) -> f64 {
    let value = value.filter(|v| v.is_finite()).unwrap_or(fallback);
    js_min(range.max, js_max(range.min, value))
}

/// Apply the General tab's field limits to a profile set. Runs on every write —
/// including presets, imported scenes and plugin calls — so no path can hand a
/// negative diameter or length to the geometry builders.
pub fn clamp_profile_fields(settings: &SupportSettings) -> SupportSettings {
    use support_profile_limits::{roots, shaft, tip};
    let mut out = settings.clone();
    out.tip.contact_diameter_mm = clamp_to_range(
        Some(settings.tip.contact_diameter_mm),
        DEFAULT_TIP_CONTACT_DIAMETER_MM,
        tip::CONTACT_DIAMETER_MM,
    );
    out.tip.length_mm = clamp_to_range(
        Some(settings.tip.length_mm),
        DEFAULT_TIP_LENGTH_MM,
        tip::LENGTH_MM,
    );
    // 30 mirrors the fallback the General tab uses when the offset is unset.
    out.tip.adaptive_cone_angle_offset_deg = Some(clamp_to_range(
        settings.tip.adaptive_cone_angle_offset_deg,
        30.0,
        tip::ADAPTIVE_CONE_ANGLE_OFFSET_DEG,
    ));
    out.shaft.diameter_mm = clamp_to_range(
        Some(settings.shaft.diameter_mm),
        DEFAULT_SHAFT_DIAMETER_MM,
        shaft::DIAMETER_MM,
    );
    out.roots.diameter_mm = clamp_to_range(
        Some(settings.roots.diameter_mm),
        DEFAULT_ROOTS_DIAMETER_MM,
        roots::DIAMETER_MM,
    );
    out.roots.disk_height_mm = clamp_to_range(
        Some(settings.roots.disk_height_mm),
        DEFAULT_ROOTS_DISK_HEIGHT_MM,
        roots::DISK_HEIGHT_MM,
    );
    out.roots.cone_height_mm = clamp_to_range(
        Some(settings.roots.cone_height_mm),
        DEFAULT_ROOTS_CONE_HEIGHT_MM,
        roots::CONE_HEIGHT_MM,
    );
    out
}

/// `mergeWithDefaults` of the store: absent fields already took their defaults
/// at deserialization; this applies the coercions and clamps.
fn merge_with_defaults(settings: &SupportSettings) -> SupportSettings {
    let mut merged = settings.clone();
    merged.shaft.routing_algorithm = Some(
        if settings.shaft.routing_algorithm == Some(RoutingAlgorithm::Astar) {
            RoutingAlgorithm::Astar
        } else {
            RoutingAlgorithm::Potential
        },
    );
    let grid_defaults = super::types::GridSettings::default();
    let finite_or = |v: f64, fallback: f64| if v.is_finite() { v } else { fallback };
    merged.grid.spacing_mm = finite_or(settings.grid.spacing_mm, grid_defaults.spacing_mm);
    merged.grid.min_branch_angle_deg = finite_or(
        settings.grid.min_branch_angle_deg,
        grid_defaults.min_branch_angle_deg,
    );
    merged.grid.attach_search_step_mm = finite_or(
        settings.grid.attach_search_step_mm,
        grid_defaults.attach_search_step_mm,
    );
    merged.grid.min_routed_trunk_angle_deg = finite_or(
        settings.grid.min_routed_trunk_angle_deg,
        grid_defaults.min_routed_trunk_angle_deg,
    );
    merged.auto_bracing = normalize_auto_bracing_settings(&settings.auto_bracing);
    merged.auto_support = normalize_auto_support_settings(&settings.auto_support);
    clamp_profile_fields(&merged)
}

/// `setSettings(settings)`: the settings the store holds after the write.
pub fn set_settings(settings: &SupportSettings) -> SupportSettings {
    let mut merged = merge_with_defaults(settings);
    // Clamp contact cone body diameter so it never exceeds the trunk diameter
    if merged.tip.body_diameter_mm > merged.shaft.diameter_mm {
        merged.tip.body_diameter_mm = merged.shaft.diameter_mm;
    }
    merged
}
