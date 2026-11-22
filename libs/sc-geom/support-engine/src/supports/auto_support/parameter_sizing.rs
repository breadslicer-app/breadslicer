//! DragonFruit `src/supports/autoSupport/parameterSizing.ts`.
//!
//! Empirical sizing (locked: no physics pretense). The band is BORROWED from a
//! Support Studio preset: `autoSupport.sizingPreset` names it and the run
//! resolves that id through [`SizingBands`] (a deleted preset falls back to the
//! factory `structure` band). The merged-cluster tail, the height factor and
//! the model factor (print scale × mass per support) ride on top of the band,
//! all floored at ×1 so the light end is untouched.
//!
//! DragonFruit resolves preset ids against its preset store, or against the
//! table a worker run is handed (`setResolvedSizingBands`). The port has no
//! store: the caller hands the table, and [`SizingBands::factory`] is what a
//! store holding only the factory presets resolves to. Live settings
//! (`getSettings()`) are passed in.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::settings::{SizingBand, SizingPreset};
use super::types::CandidatePoint;
use super::zone_rules::ZoneRules;
use crate::supports::settings::types::SupportSettings;
use crate::geom::math_utils::{js_min, js_pow};

/// Per-support sizing overrides; `None` keeps the settings value.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shaft_diameter_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip_contact_diameter_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip_body_diameter_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip_length_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip_penetration_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots_diameter_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots_disk_height_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots_cone_height_mm: Option<f64>,
}

/// The band table a run resolves preset ids against: Support Studio preset id
/// to the seven numbers the sizing paths read.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SizingBands(pub IndexMap<String, SizingBand>);

impl SizingBands {
    /// What a preset store holding only the factory presets resolves to.
    ///
    /// Read from the presets themselves (`Settings/presets.ts`), not from the
    /// `SIZING_BANDS` "mirror": the factory `structure` preset spreads
    /// `createDefaultSettings().tip`, so its tip penetration is the default
    /// (0.1 mm) where the mirror says 0. `detail` and `anchor` state theirs.
    pub fn factory() -> Self {
        let structure = SizingBand {
            tip_penetration_mm: SupportSettings::default().tip.penetration_mm,
            ..SizingPreset::Structure.band()
        };
        SizingBands(IndexMap::from([
            ("detail".to_owned(), SizingPreset::Detail.band()),
            ("structure".to_owned(), structure),
            ("anchor".to_owned(), SizingPreset::Anchor.band()),
        ]))
    }

    /// The table with every size of `rules` added under its zone id
    /// (SoulCrafted, #360).
    pub fn with_zone_sizes(mut self, rules: &ZoneRules) -> Self {
        for (i, size) in rules.sizes.iter().enumerate() {
            self.0.insert(rules.band_id(i), size.band());
        }
        self
    }

    /// `resolveSizingBand(id)`: the table's band, else the factory `structure`
    /// band. Never fails: a user can delete a preset an auto-support block
    /// still names, and the run has to size with something.
    pub fn resolve(&self, id: Option<&str>) -> SizingBand {
        id.and_then(|id| self.0.get(id))
            .copied()
            .unwrap_or(SizingPreset::Structure.band())
    }

    /// `activeSizingBand()`: the preset the live settings name.
    pub fn active(&self, settings: &SupportSettings) -> SizingBand {
        self.resolve(Some(&settings.auto_support.sizing_preset))
    }

}

// Model-scale sizing: three bounded power laws on top of the band, each
// floored at ×1 and monotone in its own input.

/// Model extent (mm, bbox diagonal) at or below which supports are at band.
pub const SIZE_REFERENCE_MM: f64 = 60.0;
pub const SIZE_EXPONENT: f64 = 0.3;

/// Resin grams per support at or below which supports are at band.
pub const SHARE_REFERENCE_G: f64 = 0.6;
pub const SHARE_EXPONENT: f64 = 0.25;

/// Resin density (g/mm³) — 1.1 g/cm³.
pub const RESIN_DENSITY_G_PER_MM3: f64 = 0.0011;

fn power_factor(ratio: f64, exponent: f64, max_factor: f64) -> f64 {
    if !(ratio > 1.0) {
        return 1.0;
    }
    js_min(max_factor, js_pow(ratio, exponent))
}

/// The run-level terms the sizing reads, and what they resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSizingFactors {
    /// Model extent (mm) the size term read.
    pub size_mm: f64,
    /// Resin grams per support the load term read.
    pub load_share_g: f64,
    /// Geometric scale of the print, ×1 … the configured size cap.
    pub size_factor: f64,
    /// Mass one support carries, ×1 … the configured load cap.
    pub load_factor: f64,
    /// sizeFactor × loadFactor — the run-level multiplier on shaft and roots.
    pub trunk_scale: f64,
}

/// Context passed from the orchestrator for model-level sizing.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSizingContext {
    /// Estimated model volume (mm³, from the mesh — exact tetrahedron sum).
    pub model_volume_mm3: f64,
    /// Model top Z (world mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_z_max_mm: Option<f64>,
    /// Total number of candidates being placed.
    pub total_candidates: f64,
    /// Model extent in mm (world-frame bbox diagonal) — the size term input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_size_mm: Option<f64>,
}

/// Resolve the run-level sizing terms. Absent context returns all ones, so
/// sizing is exactly the band; `autoSupport.modelScaleEnabled === false` pins
/// every factor to ×1 the same way.
pub fn model_sizing_factors(
    ctx: Option<&ModelSizingContext>,
    settings: &SupportSettings,
) -> ModelSizingFactors {
    let auto = &settings.auto_support;
    let enabled = auto.model_scale_enabled;
    let size_mm = ctx.and_then(|c| c.model_size_mm).unwrap_or(0.0);
    let share_g = match ctx {
        Some(c) if c.total_candidates > 0.0 => {
            (c.model_volume_mm3 * RESIN_DENSITY_G_PER_MM3) / c.total_candidates
        }
        _ => 0.0,
    };
    let size_factor = if enabled && size_mm > 0.0 {
        power_factor(
            size_mm / SIZE_REFERENCE_MM,
            SIZE_EXPONENT,
            auto.model_size_factor_cap,
        )
    } else {
        1.0
    };
    let load_factor = if enabled && share_g > 0.0 {
        power_factor(
            share_g / SHARE_REFERENCE_G,
            SHARE_EXPONENT,
            auto.model_load_factor_cap,
        )
    } else {
        1.0
    };
    ModelSizingFactors {
        size_mm,
        load_share_g: share_g,
        size_factor,
        load_factor,
        trunk_scale: size_factor * load_factor,
    }
}

/// The tier a supported area (mm²) falls in, for the forest ledger and the
/// load budget relative weighting. Sizing does NOT read it.
pub fn preset_for_area(area_mm2: f64) -> SizingPreset {
    if area_mm2 <= 0.15 {
        SizingPreset::Detail
    } else if area_mm2 <= 0.5 {
        SizingPreset::Structure
    } else {
        SizingPreset::Anchor
    }
}

/// The sizes a candidate is drawn with: its zone size exactly as written
/// (#360), the root widened by the adaptive base when the rules have one
/// (#471).
pub fn size_parameters(
    candidate: &CandidatePoint,
    settings: &SupportSettings,
    bands: &SizingBands,
) -> SizeOverrides {
    let band = bands.active(settings);
    let rules = &settings.auto_support.zone_rules;
    let base = if rules.adaptive_base {
        super::zone_rules::adaptive_base_factor(candidate.tip_pos.z)
    } else {
        1.0
    };
    SizeOverrides {
        shaft_diameter_mm: Some(band.shaft_diameter_mm),
        tip_contact_diameter_mm: Some(band.tip_contact_diameter_mm),
        tip_body_diameter_mm: Some(band.shaft_diameter_mm),
        tip_length_mm: Some(band.tip_length_mm),
        tip_penetration_mm: Some(band.tip_penetration_mm),
        roots_diameter_mm: Some(band.root_diameter_mm * base),
        roots_disk_height_mm: Some(band.root_disk_height_mm),
        roots_cone_height_mm: Some(band.root_cone_height_mm),
    }
}
