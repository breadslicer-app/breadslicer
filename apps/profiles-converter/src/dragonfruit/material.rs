/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mapping of a DragonFruit material record into a [`MaterialProfile`].
//!
//! The record is deserialised into [`RawMaterial`], a thin mirror of
//! DragonFruit's schema, then [`map_material`] constructs our domain
//! type. DragonFruit keeps the exposure and motion settings on the
//! material itself; we write one calibration block per entry in
//! `validForPresets`. The exposures are the same for every machine, the
//! motion is not: the record states it per output container, and only
//! there does it state the bottom layers' own. A record with no presets
//! converts to a template with an empty per-printer map.

use std::collections::BTreeMap;
use std::fmt;

use sc_print_primitives::{
    InvalidProfileId, LengthOutOfRange, MaterialId, NonEmptyString, PositiveFiniteMm,
    PositiveFiniteMmPerMin, PositiveFiniteSec, ScaleCompensation, ScaleCompensationPercent,
    ScaleCompensationPercentOutOfBound, SecOutOfRange, SpeedMmPerMinOutOfRange,
};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialPhysical, MaterialProfile,
};
use semver::Version;
use serde::Deserialize;

use crate::dragonfruit::preset_index::PresetIndex;

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
    #[serde(rename = "localSettingsByOutput", default)]
    local_settings_by_output: BTreeMap<String, RawOverride>,
}

/// A DragonFruit per-container settings override, keyed in the record by
/// the container it applies to.
///
/// The record's flat fields carry one number per quantity; these carry
/// the machine's own split of it, a first stage and a second, with a
/// second distance of 0 when the machine moves in one go.
#[derive(Debug, Deserialize)]
struct RawOverride {
    #[serde(rename = "liftDistanceMm", default)]
    lift_distance_mm: Option<f64>,
    #[serde(rename = "liftDistance2Mm", default)]
    lift_distance_2_mm: Option<f64>,
    #[serde(rename = "liftSpeedMmMin", default)]
    lift_speed_mm_min: Option<f64>,
    #[serde(rename = "retractSpeedMmMin", default)]
    retract_speed_mm_min: Option<f64>,
    #[serde(rename = "bottomLiftDistanceMm", default)]
    bottom_lift_distance_mm: Option<f64>,
    #[serde(rename = "bottomLiftDistance2Mm", default)]
    bottom_lift_distance_2_mm: Option<f64>,
    #[serde(rename = "bottomLiftSpeedMmMin", default)]
    bottom_lift_speed_mm_min: Option<f64>,
    #[serde(rename = "bottomRetractSpeedMmMin", default)]
    bottom_retract_speed_mm_min: Option<f64>,
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
    /// The record names a printer preset no printer record of the same
    /// run carries, so which machine it means is unknown.
    UnknownPreset {
        id: String,
        preset: String,
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
            Self::UnknownPreset { id, preset } => write!(
                f,
                "material {id:?}: printer preset {preset:?} is not among the printers being converted",
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
            Self::Length { source, .. } => Some(source),
            Self::Exposure { source, .. } => Some(source),
            Self::Speed { source, .. } => Some(source),
            Self::ScaleCompensation { source, .. } => Some(source),
            Self::EmptyField { .. } => None,
            Self::UnknownPreset { .. } => None,
        }
    }
}

/// Convert one DragonFruit material record into a [`MaterialProfile`],
/// keying its calibration blocks to the machines `presets` names.
///
/// The record points at printers by the source's preset ids, which carry
/// the output container the preset writes; ours name the machine alone.
/// So a record whose presets are absent from `presets` cannot be mapped,
/// rather than being keyed to an id no printer has.
pub(crate) fn map_material(
    raw: RawMaterial,
    presets: &PresetIndex,
) -> Result<MaterialProfile, MaterialMapError> {
    let id = MaterialId::new(&raw.template_id).map_err(|source| MaterialMapError::InvalidId {
        id: raw.template_id.clone(),
        source,
    })?;
    let raw_id = raw.template_id.clone();

    let brand = non_empty(&raw.brand, &raw_id, "brand")?;
    let name = non_empty(&raw.name, &raw_id, "name")?;
    let family = non_empty(&raw.resin_family, &raw_id, "resinFamily")?;

    let intent = MaterialIntent {
        target_energy_density_mj_per_cm2: None,
        target_layer_height_mm: opt_mm(raw.layer_height_mm, &raw_id, "layerHeightMm")?,
    };

    let scale_compensation_pct = match &raw.scale_compensation_pct {
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
    // material rather than the printer. The exposures are the same
    // whatever the machine; the motion is not, so it is resolved per
    // preset below. bottom_layer_count is kept both here and in the
    // defaults.
    let shared = MaterialPerPrinter {
        normal_exposure_sec: opt_sec(raw.normal_exposure_sec, &raw_id, "normalExposureSec")?,
        bottom_exposure_sec: opt_sec(raw.bottom_exposure_sec, &raw_id, "bottomExposureSec")?,
        bottom_layer_count: raw.bottom_layer_count,
        wait_before_cure_sec: None,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: None,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: None,
        lift_speed_mm_min: None,
        retract_speed_mm_min: None,
        bottom_lift_distance_mm: None,
        bottom_lift_speed_mm_min: None,
        bottom_retract_speed_mm_min: None,
        light_pwm: None,
        bottom_light_pwm: None,
        second_travel: None,
        bottom_second_travel: None,
    };

    let mut per_printer = BTreeMap::new();
    for preset in &raw.valid_for_presets {
        let entry = presets
            .get(preset)
            .ok_or_else(|| MaterialMapError::UnknownPreset {
                id: raw_id.clone(),
                preset: preset.clone(),
            })?;
        let motion = Motion::of(
            &raw,
            raw.local_settings_by_output.get(&entry.container),
            &raw_id,
        )?;
        per_printer.insert(
            CalibrationTarget::from(entry.printer.clone()),
            motion.into_block(shared),
        );
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
        // The source format publishes no densities.
        physical: MaterialPhysical::default(),
        per_printer,
    })
}

/// How the platform moves over a layer, for one machine.
///
/// The record states this twice: once flat at its top level, and once
/// per container under `localSettingsByOutput`, where the bottom layers
/// get their own set. The container's is the one a machine follows.
struct Motion {
    lift_distance_mm: Option<PositiveFiniteMm>,
    lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    bottom_lift_distance_mm: Option<PositiveFiniteMm>,
    bottom_lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    bottom_retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
}

impl Motion {
    /// Read the motion for the container `over` applies to, falling back
    /// to the record's flat fields when the record states none for it.
    ///
    /// A distance here is the whole rise. The source splits it into the
    /// slow peel and the fast remainder, which add up: its own flat
    /// `liftDistanceMm` equals the sum of the 2, and a machine that moves
    /// in one go writes a second distance of 0.
    fn of(
        raw: &RawMaterial,
        over: Option<&RawOverride>,
        id: &str,
    ) -> Result<Self, MaterialMapError> {
        let Some(over) = over else {
            return Ok(Self {
                lift_distance_mm: opt_mm(raw.lift_distance_mm, id, "liftDistanceMm")?,
                lift_speed_mm_min: opt_speed(raw.lift_speed_mm_min, id, "liftSpeedMmMin")?,
                retract_speed_mm_min: opt_speed(raw.retract_speed_mm_min, id, "retractSpeedMmMin")?,
                bottom_lift_distance_mm: None,
                bottom_lift_speed_mm_min: None,
                bottom_retract_speed_mm_min: None,
            });
        };
        Ok(Self {
            lift_distance_mm: opt_mm(
                total(over.lift_distance_mm, over.lift_distance_2_mm),
                id,
                "liftDistanceMm",
            )?,
            lift_speed_mm_min: opt_speed(over.lift_speed_mm_min, id, "liftSpeedMmMin")?,
            retract_speed_mm_min: opt_speed(over.retract_speed_mm_min, id, "retractSpeedMmMin")?,
            bottom_lift_distance_mm: opt_mm(
                total(over.bottom_lift_distance_mm, over.bottom_lift_distance_2_mm),
                id,
                "bottomLiftDistanceMm",
            )?,
            bottom_lift_speed_mm_min: opt_speed(
                over.bottom_lift_speed_mm_min,
                id,
                "bottomLiftSpeedMmMin",
            )?,
            bottom_retract_speed_mm_min: opt_speed(
                over.bottom_retract_speed_mm_min,
                id,
                "bottomRetractSpeedMmMin",
            )?,
        })
    }

    /// Put this motion into `block`, which carries everything a machine
    /// does not change.
    fn into_block(self, block: MaterialPerPrinter) -> MaterialPerPrinter {
        MaterialPerPrinter {
            lift_distance_mm: self.lift_distance_mm,
            lift_speed_mm_min: self.lift_speed_mm_min,
            retract_speed_mm_min: self.retract_speed_mm_min,
            bottom_lift_distance_mm: self.bottom_lift_distance_mm,
            bottom_lift_speed_mm_min: self.bottom_lift_speed_mm_min,
            bottom_retract_speed_mm_min: self.bottom_retract_speed_mm_min,
            ..block
        }
    }
}

/// Add a quantity's 2 stages, treating an absent stage as 0 and an
/// absent pair as no value at all.
fn total(first: Option<f64>, second: Option<f64>) -> Option<f64> {
    match (first, second) {
        (None, None) => None,
        (first, second) => Some(first.unwrap_or_default() + second.unwrap_or_default()),
    }
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
fn map_scale(raw: &RawScaleCompensation, id: &str) -> Result<ScaleCompensation, MaterialMapError> {
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
    use sc_print_primitives::PrinterId;

    use super::*;
    use crate::dragonfruit::preset_index::Preset;

    /// The machines the fixtures' presets name, as a run's printer
    /// records describe them: one preset per container it writes,
    /// resolving to the machine alone.
    fn presets() -> PresetIndex {
        let mut presets = PresetIndex::default();
        presets.insert("printer-a-goo", preset("printer-a", ".goo"));
        presets.insert("printer-b-ctb", preset("printer-b", ".ctb"));
        presets.insert("printer-c-pwmx", preset("printer-c", ".pwmx"));
        presets
    }

    fn preset(id: &str, container: &str) -> Preset {
        Preset {
            printer: PrinterId::new(id).unwrap(),
            container: container.to_owned(),
        }
    }

    fn map(json: &str) -> Result<MaterialProfile, MaterialMapError> {
        map_material(serde_json::from_str(json).unwrap(), &presets())
    }

    const FULL: &str = r#"{
        "templateId": "standard-405",
        "profileVersion": 3,
        "name": "Standard 405",
        "brand": "TestResin",
        "resinFamily": "standard",
        "validForPresets": ["printer-a-goo", "printer-b-ctb"],
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

    /// A record as the source ships one: the flat fields carry the whole
    /// rise, and each container it is calibrated for carries the split
    /// the machine follows, plus the bottom layers' own set. `.ctb` here
    /// is a machine that moves in one go, written as a second stage of 0.
    const WITH_OVERRIDES: &str = r#"{
        "templateId": "split-405", "profileVersion": 1,
        "name": "Split", "brand": "TestResin", "resinFamily": "standard",
        "validForPresets": ["printer-a-goo", "printer-b-ctb", "printer-c-pwmx"],
        "layerHeightMm": 0.05, "normalExposureSec": 2.0, "bottomExposureSec": 25.0,
        "bottomLayerCount": 5,
        "liftDistanceMm": 7.0, "liftSpeedMmMin": 45.0, "retractSpeedMmMin": 240.0,
        "localSettingsByOutput": {
            ".goo": {
                "liftDistanceMm": 3.0, "liftDistance2Mm": 4.0,
                "liftSpeedMmMin": 45.0, "liftSpeed2MmMin": 240.0,
                "retractSpeedMmMin": 240.0, "retractSpeed2MmMin": 60.0,
                "bottomLiftDistanceMm": 3.0, "bottomLiftDistance2Mm": 4.0,
                "bottomLiftSpeedMmMin": 40.0, "bottomRetractSpeedMmMin": 210.0
            },
            ".ctb": {
                "liftDistanceMm": 8.0, "liftDistance2Mm": 0.0,
                "liftSpeedMmMin": 50.0, "retractSpeedMmMin": 200.0,
                "bottomLiftDistanceMm": 8.0, "bottomLiftDistance2Mm": 0.0,
                "bottomLiftSpeedMmMin": 35.0, "bottomRetractSpeedMmMin": 180.0
            }
        }
    }"#;

    fn block_of(profile: &MaterialProfile, id: &str) -> MaterialPerPrinter {
        profile.per_printer[&CalibrationTarget::from(PrinterId::new(id).unwrap())]
    }

    #[test]
    fn each_machine_takes_the_block_of_the_container_it_writes() {
        let profile = map(WITH_OVERRIDES).unwrap();

        let goo = block_of(&profile, "printer-a");
        assert_eq!(goo.lift_distance_mm.map(|v| v.value()), Some(7.0));
        assert_eq!(goo.lift_speed_mm_min.map(|v| v.value()), Some(45.0));
        assert_eq!(goo.retract_speed_mm_min.map(|v| v.value()), Some(240.0));
        assert_eq!(goo.bottom_lift_distance_mm.map(|v| v.value()), Some(7.0));
        assert_eq!(goo.bottom_lift_speed_mm_min.map(|v| v.value()), Some(40.0));
        assert_eq!(
            goo.bottom_retract_speed_mm_min.map(|v| v.value()),
            Some(210.0)
        );

        let ctb = block_of(&profile, "printer-b");
        assert_eq!(ctb.lift_distance_mm.map(|v| v.value()), Some(8.0));
        assert_eq!(ctb.lift_speed_mm_min.map(|v| v.value()), Some(50.0));
        assert_eq!(ctb.retract_speed_mm_min.map(|v| v.value()), Some(200.0));
        assert_eq!(ctb.bottom_lift_distance_mm.map(|v| v.value()), Some(8.0));
        assert_eq!(ctb.bottom_lift_speed_mm_min.map(|v| v.value()), Some(35.0));
        assert_eq!(
            ctb.bottom_retract_speed_mm_min.map(|v| v.value()),
            Some(180.0)
        );
    }

    #[test]
    fn a_container_the_record_says_nothing_about_falls_back_to_the_flat_fields() {
        let profile = map(WITH_OVERRIDES).unwrap();

        let pwmx = block_of(&profile, "printer-c");
        assert_eq!(pwmx.lift_distance_mm.map(|v| v.value()), Some(7.0));
        assert_eq!(pwmx.lift_speed_mm_min.map(|v| v.value()), Some(45.0));
        assert_eq!(pwmx.retract_speed_mm_min.map(|v| v.value()), Some(240.0));
        // The flat fields state no bottom motion at all.
        assert_eq!(pwmx.bottom_lift_distance_mm, None);
        assert_eq!(pwmx.bottom_lift_speed_mm_min, None);
        assert_eq!(pwmx.bottom_retract_speed_mm_min, None);
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
    fn a_preset_no_printer_record_covers_is_rejected() {
        let json = r#"{
            "templateId": "resin", "profileVersion": 1,
            "name": "Resin", "brand": "Default", "resinFamily": "standard",
            "validForPresets": ["printer-z-goo"]
        }"#;
        match map(json).unwrap_err() {
            MaterialMapError::UnknownPreset { preset, .. } => assert_eq!(preset, "printer-z-goo"),
            other => panic!("expected UnknownPreset, got {other}"),
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
