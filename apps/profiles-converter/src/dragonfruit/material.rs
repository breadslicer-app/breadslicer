/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mapping of a DragonFruit material record into a [`MaterialProfile`].
//!
//! The record is deserialised into [`RawMaterial`], a thin mirror of
//! DragonFruit's schema, then [`map_material`] constructs our domain
//! type. DragonFruit keeps the exposure and motion settings on the
//! material itself; we apply them per printer, once for each entry in
//! `validForPresets`. A record with no presets converts to a template
//! with an empty per-printer map.

use std::collections::BTreeMap;
use std::fmt;

use sc_print_primitives::{
    InvalidProfileId, LengthOutOfRange, MaterialId, NonEmptyString, PositiveFiniteMm,
    PositiveFiniteMmPerMin, PositiveFiniteSec, PrinterId, ScaleCompensation,
    ScaleCompensationPercent, ScaleCompensationPercentOutOfBound, SecOutOfRange,
    SpeedMmPerMinOutOfRange,
};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialProfile,
};
use semver::Version;
use serde::Deserialize;

/// A DragonFruit material record, as it appears in the source JSON.
/// Only the fields we map are declared; the rest are ignored.
#[derive(Debug, Deserialize)]
pub(crate) struct RawMaterial {
    #[serde(rename = "templateId")]
    template_id: String,
    #[serde(rename = "profileVersion")]
    profile_version: u64,
    name: String,
    brand: String,
    #[serde(rename = "resinFamily")]
    resin_family: String,
    #[serde(rename = "validForPresets", default)]
    valid_for_presets: Vec<String>,
    #[serde(rename = "scaleCompensationPct", default)]
    scale_compensation_pct: Option<RawScaleCompensation>,
    #[serde(rename = "layerHeightMm", default)]
    layer_height_mm: Option<f64>,
    #[serde(rename = "normalExposureSec", default)]
    normal_exposure_sec: Option<f64>,
    #[serde(rename = "bottomExposureSec", default)]
    bottom_exposure_sec: Option<f64>,
    #[serde(rename = "bottomLayerCount", default)]
    bottom_layer_count: Option<u32>,
    #[serde(rename = "liftDistanceMm", default)]
    lift_distance_mm: Option<f64>,
    #[serde(rename = "liftSpeedMmMin", default)]
    lift_speed_mm_min: Option<f64>,
    #[serde(rename = "retractSpeedMmMin", default)]
    retract_speed_mm_min: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RawScaleCompensation {
    x: f64,
    y: f64,
    z: f64,
}

/// Everything that can go wrong mapping one DragonFruit material record.
#[derive(Debug)]
pub enum MaterialMapError {
    InvalidId {
        id: String,
        source: InvalidProfileId,
    },
    EmptyField {
        id: String,
        field: &'static str,
    },
    InvalidPresetId {
        id: String,
        preset: String,
        source: InvalidProfileId,
    },
    Length {
        id: String,
        field: &'static str,
        source: LengthOutOfRange,
    },
    Exposure {
        id: String,
        field: &'static str,
        source: SecOutOfRange,
    },
    Speed {
        id: String,
        field: &'static str,
        source: SpeedMmPerMinOutOfRange,
    },
    ScaleCompensation {
        id: String,
        axis: &'static str,
        source: ScaleCompensationPercentOutOfBound,
    },
}

impl fmt::Display for MaterialMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId { id, source } => {
                write!(f, "material {id:?}: invalid template id: {source}")
            }
            Self::EmptyField { id, field } => {
                write!(f, "material {id:?}: field {field:?} is empty")
            }
            Self::InvalidPresetId { id, preset, source } => write!(
                f,
                "material {id:?}: invalid printer preset {preset:?}: {source}",
            ),
            Self::Length { id, field, source } => {
                write!(f, "material {id:?}: field {field:?}: {source}")
            }
            Self::Exposure { id, field, source } => {
                write!(f, "material {id:?}: field {field:?}: {source}")
            }
            Self::Speed { id, field, source } => {
                write!(f, "material {id:?}: field {field:?}: {source}")
            }
            Self::ScaleCompensation { id, axis, source } => write!(
                f,
                "material {id:?}: scale compensation {axis} axis: {source}",
            ),
        }
    }
}

impl std::error::Error for MaterialMapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidId { source, .. } => Some(source),
            Self::InvalidPresetId { source, .. } => Some(source),
            Self::Length { source, .. } => Some(source),
            Self::Exposure { source, .. } => Some(source),
            Self::Speed { source, .. } => Some(source),
            Self::ScaleCompensation { source, .. } => Some(source),
            Self::EmptyField { .. } => None,
        }
    }
}

/// Convert one DragonFruit material record into a [`MaterialProfile`].
pub(crate) fn map_material(raw: RawMaterial) -> Result<MaterialProfile, MaterialMapError> {
    let id = MaterialId::new(&raw.template_id).map_err(|source| MaterialMapError::InvalidId {
        id: raw.template_id.clone(),
        source,
    })?;
    let raw_id = raw.template_id;

    let brand = non_empty(&raw.brand, &raw_id, "brand")?;
    let name = non_empty(&raw.name, &raw_id, "name")?;
    let family = non_empty(&raw.resin_family, &raw_id, "resinFamily")?;

    let intent = MaterialIntent {
        target_energy_density_mj_per_cm2: None,
        target_layer_height_mm: opt_mm(raw.layer_height_mm, &raw_id, "layerHeightMm")?,
    };

    let scale_compensation_pct = match raw.scale_compensation_pct {
        Some(scale) => Some(map_scale(scale, &raw_id)?),
        None => None,
    };
    let defaults = MaterialDefaults {
        scale_compensation_pct,
        bottom_layer_count: raw.bottom_layer_count,
        wait_before_cure_sec: None,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        light_pwm: None,
        bottom_light_pwm: None,
    };

    // DragonFruit carries the exposure and motion settings on the
    // material; we apply the same block to every printer it is valid
    // for. bottom_layer_count is kept both here and in the defaults.
    let per_printer_settings = MaterialPerPrinter {
        normal_exposure_sec: opt_sec(raw.normal_exposure_sec, &raw_id, "normalExposureSec")?,
        bottom_exposure_sec: opt_sec(raw.bottom_exposure_sec, &raw_id, "bottomExposureSec")?,
        bottom_layer_count: raw.bottom_layer_count,
        wait_before_cure_sec: None,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: None,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: opt_mm(raw.lift_distance_mm, &raw_id, "liftDistanceMm")?,
        lift_speed_mm_min: opt_speed(raw.lift_speed_mm_min, &raw_id, "liftSpeedMmMin")?,
        retract_speed_mm_min: opt_speed(raw.retract_speed_mm_min, &raw_id, "retractSpeedMmMin")?,
        light_pwm: None,
        bottom_light_pwm: None,
    };

    let mut per_printer = BTreeMap::new();
    for preset in &raw.valid_for_presets {
        let printer_id =
            PrinterId::new(preset).map_err(|source| MaterialMapError::InvalidPresetId {
                id: raw_id.clone(),
                preset: preset.clone(),
                source,
            })?;
        per_printer.insert(CalibrationTarget::from(printer_id), per_printer_settings);
    }

    Ok(MaterialProfile {
        extension: false,
        identity: MaterialIdentity {
            id,
            brand,
            family,
            name,
            profile_version: Version::new(raw.profile_version, 0, 0),
        },
        intent,
        defaults,
        per_printer,
    })
}

/// Build a non-empty string for `field`, or report it empty.
fn non_empty(
    value: &str,
    id: &str,
    field: &'static str,
) -> Result<NonEmptyString, MaterialMapError> {
    NonEmptyString::new(value).map_err(|_| MaterialMapError::EmptyField {
        id: id.to_owned(),
        field,
    })
}

/// Map an optional millimetre value, reporting an out-of-range one.
fn opt_mm(
    value: Option<f64>,
    id: &str,
    field: &'static str,
) -> Result<Option<PositiveFiniteMm>, MaterialMapError> {
    value
        .map(|raw| {
            PositiveFiniteMm::new(raw).map_err(|source| MaterialMapError::Length {
                id: id.to_owned(),
                field,
                source,
            })
        })
        .transpose()
}

/// Map an optional exposure-seconds value, reporting an out-of-range one.
fn opt_sec(
    value: Option<f64>,
    id: &str,
    field: &'static str,
) -> Result<Option<PositiveFiniteSec>, MaterialMapError> {
    value
        .map(|raw| {
            PositiveFiniteSec::new(raw).map_err(|source| MaterialMapError::Exposure {
                id: id.to_owned(),
                field,
                source,
            })
        })
        .transpose()
}

/// Map an optional speed value, reporting an out-of-range one.
fn opt_speed(
    value: Option<f64>,
    id: &str,
    field: &'static str,
) -> Result<Option<PositiveFiniteMmPerMin>, MaterialMapError> {
    value
        .map(|raw| {
            PositiveFiniteMmPerMin::new(raw).map_err(|source| MaterialMapError::Speed {
                id: id.to_owned(),
                field,
                source,
            })
        })
        .transpose()
}

/// Map the three scale-compensation axes.
fn map_scale(raw: RawScaleCompensation, id: &str) -> Result<ScaleCompensation, MaterialMapError> {
    let axis = |value: f64, axis: &'static str| {
        ScaleCompensationPercent::new(value).map_err(|source| MaterialMapError::ScaleCompensation {
            id: id.to_owned(),
            axis,
            source,
        })
    };
    Ok(ScaleCompensation {
        x: axis(raw.x, "x")?,
        y: axis(raw.y, "y")?,
        z: axis(raw.z, "z")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(json: &str) -> Result<MaterialProfile, MaterialMapError> {
        map_material(serde_json::from_str(json).unwrap())
    }

    const FULL: &str = r#"{
        "templateId": "standard-405",
        "profileVersion": 3,
        "name": "Standard 405",
        "brand": "TestResin",
        "resinFamily": "standard",
        "validForPresets": ["printer-a", "printer-b"],
        "currencyCode": "USD", "bottlePrice": 24.99, "bottleCapacityMl": 1000,
        "scaleCompensationPct": { "x": 0.5, "y": -0.5, "z": 1.0 },
        "layerHeightMm": 0.05,
        "normalExposureSec": 2.5,
        "bottomExposureSec": 30.0,
        "bottomLayerCount": 5,
        "liftDistanceMm": 6.0,
        "liftSpeedMmMin": 65.0,
        "retractSpeedMmMin": 150.0
    }"#;

    #[test]
    fn maps_identity_intent_and_defaults() {
        let profile = map(FULL).unwrap();
        assert_eq!(
            profile.identity.id,
            MaterialId::new("standard-405").unwrap()
        );
        assert_eq!(profile.identity.brand.as_str(), "TestResin");
        assert_eq!(profile.identity.family.as_str(), "standard");
        assert_eq!(profile.identity.name.as_str(), "Standard 405");
        assert_eq!(profile.identity.profile_version, Version::new(3, 0, 0));

        assert_eq!(
            profile.intent.target_layer_height_mm,
            Some(PositiveFiniteMm::new(0.05).unwrap()),
        );
        assert_eq!(profile.defaults.bottom_layer_count, Some(5));
        assert_eq!(
            profile.defaults.scale_compensation_pct,
            Some(ScaleCompensation {
                x: ScaleCompensationPercent::new(0.5).unwrap(),
                y: ScaleCompensationPercent::new(-0.5).unwrap(),
                z: ScaleCompensationPercent::new(1.0).unwrap(),
            }),
        );
    }

    #[test]
    fn applies_settings_to_every_valid_preset() {
        let profile = map(FULL).unwrap();
        let keys: Vec<&str> = profile
            .per_printer
            .keys()
            .map(CalibrationTarget::as_str)
            .collect();
        assert_eq!(keys, vec!["printer-a", "printer-b"]);

        let entry =
            &profile.per_printer[&CalibrationTarget::from(PrinterId::new("printer-a").unwrap())];
        assert_eq!(
            entry.normal_exposure_sec,
            Some(PositiveFiniteSec::new(2.5).unwrap()),
        );
        assert_eq!(
            entry.bottom_exposure_sec,
            Some(PositiveFiniteSec::new(30.0).unwrap()),
        );
        // bottom_layer_count is written in the per-printer block too.
        assert_eq!(entry.bottom_layer_count, Some(5));
        assert_eq!(
            entry.lift_distance_mm,
            Some(PositiveFiniteMm::new(6.0).unwrap()),
        );
        assert_eq!(
            entry.lift_speed_mm_min,
            Some(PositiveFiniteMmPerMin::new(65.0).unwrap()),
        );
        assert_eq!(
            entry.retract_speed_mm_min,
            Some(PositiveFiniteMmPerMin::new(150.0).unwrap()),
        );
    }

    #[test]
    fn a_material_without_presets_has_an_empty_per_printer_map() {
        let json = r#"{
            "templateId": "generic-405", "profileVersion": 1,
            "name": "Generic", "brand": "Default", "resinFamily": "standard",
            "layerHeightMm": 0.05, "normalExposureSec": 2.4, "bottomExposureSec": 26.0,
            "bottomLayerCount": 5, "liftDistanceMm": 6.0, "liftSpeedMmMin": 65.0,
            "retractSpeedMmMin": 150.0
        }"#;
        let profile = map(json).unwrap();
        assert!(profile.per_printer.is_empty());
    }

    #[test]
    fn an_invalid_template_id_is_rejected() {
        let json = r#"{
            "templateId": "Bad_Id", "profileVersion": 1,
            "name": "Bad", "brand": "Default", "resinFamily": "standard"
        }"#;
        assert!(matches!(
            map(json).unwrap_err(),
            MaterialMapError::InvalidId { .. },
        ));
    }

    #[test]
    fn an_invalid_preset_id_is_rejected() {
        let json = r#"{
            "templateId": "resin", "profileVersion": 1,
            "name": "Resin", "brand": "Default", "resinFamily": "standard",
            "validForPresets": ["Bad_Preset"]
        }"#;
        match map(json).unwrap_err() {
            MaterialMapError::InvalidPresetId { preset, .. } => assert_eq!(preset, "Bad_Preset"),
            other => panic!("expected InvalidPresetId, got {other}"),
        }
    }

    #[test]
    fn a_non_positive_layer_height_is_rejected() {
        let json = r#"{
            "templateId": "resin", "profileVersion": 1,
            "name": "Resin", "brand": "Default", "resinFamily": "standard",
            "layerHeightMm": 0.0
        }"#;
        assert!(matches!(
            map(json).unwrap_err(),
            MaterialMapError::Length {
                field: "layerHeightMm",
                ..
            },
        ));
    }
}
