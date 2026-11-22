//! The sizes supports are drawn with: the table of size bands a run reads
//! (the three base sizes and every zone size of its rules), the size a
//! candidate takes, and the model-wide terms the run report shows.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::settings::{SizingBand, SizingPreset};
use super::types::CandidatePoint;
use super::zone_rules::ZoneRules;
use crate::geom::math_utils::{js_min, js_pow};
use crate::supports::settings::types::SupportSettings;

/// A support's own sizes; `None` keeps the settings' value.
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

/// The size bands by id: the seven numbers a support's size is drawn from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SizingBands(pub IndexMap<String, SizingBand>);

impl SizingBands {
    /// The three base sizes (`structure` keeps the default tip penetration).
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

    /// The table with every size of `rules` added under its zone id (#360).
    pub fn with_zone_sizes(mut self, rules: &ZoneRules) -> Self {
        for (i, size) in rules.sizes.iter().enumerate() {
            self.0.insert(rules.band_id(i), size.band());
        }
        self
    }

    /// The band of `id`, else the base `structure` band.
    pub fn resolve(&self, id: Option<&str>) -> SizingBand {
        id.and_then(|id| self.0.get(id))
            .copied()
            .unwrap_or(SizingPreset::Structure.band())
    }

    /// The band the settings name.
    pub fn active(&self, settings: &SupportSettings) -> SizingBand {
        self.resolve(Some(&settings.auto_support.sizing_preset))
    }
}

/// The model size (box diagonal, mm) up to which a model takes no size term.
pub const SIZE_REFERENCE_MM: f64 = 60.0;
pub const SIZE_EXPONENT: f64 = 0.3;

/// The resin per support (g) up to which a model takes no load term.
pub const SHARE_REFERENCE_G: f64 = 0.6;
pub const SHARE_EXPONENT: f64 = 0.25;

/// Resin density, g/mm³ (1.1 g/cm³).
pub const RESIN_DENSITY_G_PER_MM3: f64 = 0.0011;

fn power_factor(ratio: f64, exponent: f64, max_factor: f64) -> f64 {
    if !(ratio > 1.0) {
        return 1.0;
    }
    js_min(max_factor, js_pow(ratio, exponent))
}

/// The model-wide size terms and what they came from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSizingFactors {
    /// The model's size, mm.
    pub size_mm: f64,
    /// The resin each support carries, g.
    pub load_share_g: f64,
    /// The size term, from 1 to its cap.
    pub size_factor: f64,
    /// The load term, from 1 to its cap.
    pub load_factor: f64,
    /// The two together.
    pub trunk_scale: f64,
}

/// What the model-wide terms are worked out from.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSizingContext {
    /// The model's volume, mm³.
    pub model_volume_mm3: f64,
    /// The model's top, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_z_max_mm: Option<f64>,
    /// How many candidates are placed.
    pub total_candidates: f64,
    /// The model's size (box diagonal), mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_size_mm: Option<f64>,
}

/// The model-wide terms: each a power law of its input past its reference,
/// capped, and 1 without a context or with model scaling off.
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

/// The base size a region of `area_mm2` counts as, for the run's records and
/// the load budget (not for drawing).
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
