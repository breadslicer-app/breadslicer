//! The sizes supports are drawn with: the table of size bands a run reads
//! (the three base sizes and every zone size of its rules), the size a
//! candidate takes, and the model-wide terms the run report shows.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::settings::{SizingBand, SizingPreset};
use super::types::CandidatePoint;
use super::zone_rules::ZoneRules;
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

/// Resin density, g/mm³ (1.1 g/cm³).
pub const RESIN_DENSITY_G_PER_MM3: f64 = 0.0011;

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
