//! DragonFruit `src/supports/autoSupport/parameterSizing.ts`, the part the
//! builders call. The empirical sizing (`sizeParameters`, the model factors,
//! the active band) is ported with the auto-support pass.

use serde::{Deserialize, Serialize};

use crate::supports::settings::types::SupportSettings;

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

/// Merge sizing overrides into a settings snapshot. The settingsCodeHex
/// stamped on a placed support must describe the geometry ACTUALLY built
/// (tier band after overrides), not the global band — otherwise Support
/// Studio loads the wrong parameters for the selected support and any edit
/// clobbers the sized geometry. `None` returns the settings as they are.
pub fn apply_sizing_overrides_to_settings(
    settings: &SupportSettings,
    overrides: Option<&SizeOverrides>,
) -> SupportSettings {
    let Some(overrides) = overrides else {
        return settings.clone();
    };
    let mut out = settings.clone();
    out.shaft.diameter_mm = overrides
        .shaft_diameter_mm
        .unwrap_or(settings.shaft.diameter_mm);
    out.tip.contact_diameter_mm = overrides
        .tip_contact_diameter_mm
        .unwrap_or(settings.tip.contact_diameter_mm);
    out.tip.body_diameter_mm = overrides
        .tip_body_diameter_mm
        .unwrap_or(settings.tip.body_diameter_mm);
    out.tip.length_mm = overrides.tip_length_mm.unwrap_or(settings.tip.length_mm);
    out.tip.penetration_mm = overrides
        .tip_penetration_mm
        .unwrap_or(settings.tip.penetration_mm);
    out.roots.diameter_mm = overrides
        .roots_diameter_mm
        .unwrap_or(settings.roots.diameter_mm);
    out.roots.disk_height_mm = overrides
        .roots_disk_height_mm
        .unwrap_or(settings.roots.disk_height_mm);
    out.roots.cone_height_mm = overrides
        .roots_cone_height_mm
        .unwrap_or(settings.roots.cone_height_mm);
    out
}
