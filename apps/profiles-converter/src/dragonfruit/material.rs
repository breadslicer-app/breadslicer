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
//! there does it state the bottom layers' own motion, the pauses around
//! a layer and the light power. A record with no presets converts to a
//! template with an empty per-printer map.

use std::collections::BTreeMap;
use std::fmt;

use sc_print_primitives::{
    InvalidProfileId, LengthOutOfRange, LightPwm, LightPwmOutOfRange, MaterialId, NegativeDuration,
    NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin,
    PositiveFiniteSec, ScaleCompensation, ScaleCompensationPercent,
    ScaleCompensationPercentOutOfBound, SecOutOfRange, SpeedMmPerMinOutOfRange,
};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialPhysical, MaterialProfile, SecondTravelStage,
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
/// second distance of 0 when the machine moves in one go. The pauses
/// and the light power are stated here alone.
#[derive(Debug, Deserialize)]
struct RawOverride {
    #[serde(rename = "liftDistanceMm", default)]
    lift_distance_mm: Option<f64>,
    #[serde(rename = "liftDistance2Mm", default)]
    lift_distance_2_mm: Option<f64>,
    #[serde(rename = "liftSpeedMmMin", default)]
    lift_speed_mm_min: Option<f64>,
    #[serde(rename = "liftSpeed2MmMin", default)]
    lift_speed_2_mm_min: Option<f64>,
    #[serde(rename = "retractSpeedMmMin", default)]
    retract_speed_mm_min: Option<f64>,
    #[serde(rename = "retractSpeed2MmMin", default)]
    retract_speed_2_mm_min: Option<f64>,
    #[serde(rename = "bottomLiftDistanceMm", default)]
    bottom_lift_distance_mm: Option<f64>,
    #[serde(rename = "bottomLiftDistance2Mm", default)]
    bottom_lift_distance_2_mm: Option<f64>,
    #[serde(rename = "bottomLiftSpeedMmMin", default)]
    bottom_lift_speed_mm_min: Option<f64>,
    #[serde(rename = "bottomLiftSpeed2MmMin", default)]
    bottom_lift_speed_2_mm_min: Option<f64>,
    #[serde(rename = "bottomRetractSpeedMmMin", default)]
    bottom_retract_speed_mm_min: Option<f64>,
    #[serde(rename = "bottomRetractSpeed2MmMin", default)]
    bottom_retract_speed_2_mm_min: Option<f64>,
    #[serde(rename = "waitTimeBeforeCureSec", default)]
    wait_time_before_cure_sec: Option<f64>,
    #[serde(rename = "waitTimeAfterCureSec", default)]
    wait_time_after_cure_sec: Option<f64>,
    #[serde(rename = "waitTimeAfterLiftSec", default)]
    wait_time_after_lift_sec: Option<f64>,
    #[serde(rename = "projectorPwmPercent", default)]
    projector_pwm_percent: Option<f64>,
}

/// One quantity's numbers as the source states them: the 2 lift stages
/// and the 2 retract speeds, in the source's own order.
struct RawStages {
    lift_distance: Option<f64>,
    lift_distance_2: Option<f64>,
    lift_speed: Option<f64>,
    lift_speed_2: Option<f64>,
    retract_speed: Option<f64>,
    retract_speed_2: Option<f64>,
}

impl RawOverride {
    /// The numbers for the layers printed at the normal exposure.
    fn normal_stages(&self) -> RawStages {
        RawStages {
            lift_distance: self.lift_distance_mm,
            lift_distance_2: self.lift_distance_2_mm,
            lift_speed: self.lift_speed_mm_min,
            lift_speed_2: self.lift_speed_2_mm_min,
            retract_speed: self.retract_speed_mm_min,
            retract_speed_2: self.retract_speed_2_mm_min,
        }
    }

    /// The numbers for the bottom layers.
    fn bottom_stages(&self) -> RawStages {
        RawStages {
            lift_distance: self.bottom_lift_distance_mm,
            lift_distance_2: self.bottom_lift_distance_2_mm,
            lift_speed: self.bottom_lift_speed_mm_min,
            lift_speed_2: self.bottom_lift_speed_2_mm_min,
            retract_speed: self.bottom_retract_speed_mm_min,
            retract_speed_2: self.bottom_retract_speed_2_mm_min,
        }
    }
}

/// The source keys one quantity's numbers come from, so a rejected value
/// names the key it was written under.
#[derive(Clone, Copy)]
struct StageKeys {
    distance: &'static str,
    lift_speed: &'static str,
    retract_speed: &'static str,
}

const NORMAL_KEYS: StageKeys = StageKeys {
    distance: "liftDistanceMm",
    lift_speed: "liftSpeedMmMin",
    retract_speed: "retractSpeedMmMin",
};

const BOTTOM_KEYS: StageKeys = StageKeys {
    distance: "bottomLiftDistanceMm",
    lift_speed: "bottomLiftSpeedMmMin",
    retract_speed: "bottomRetractSpeedMmMin",
};

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
    /// The record gives a container a second travel stage to cross but
    /// no speed to cross it at.
    SplitWithoutItsSpeeds {
        id: String,
        field: &'static str,
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
    Wait {
        id: String,
        field: &'static str,
        source: NegativeDuration,
    },
    LightPower {
        id: String,
        field: &'static str,
        source: LightPwmOutOfRange,
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
                write!(
                    f,
                    "material {id:?}: its brand and name spell no id: {source}"
                )
            }
            Self::EmptyField { id, field } => {
                write!(f, "material {id:?}: field {field:?} is empty")
            }
            Self::UnknownPreset { id, preset } => write!(
                f,
                "material {id:?}: printer preset {preset:?} is not among the printers being converted",
            ),
            Self::SplitWithoutItsSpeeds { id, field } => write!(
                f,
                "material {id:?}: a second travel stage to cross, but no {field:?} to cross it at",
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
            Self::Wait { id, field, source } => {
                write!(f, "material {id:?}: field {field:?}: {source}")
            }
            Self::LightPower { id, field, source } => {
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
            Self::Wait { source, .. } => Some(source),
            Self::LightPower { source, .. } => Some(source),
            Self::ScaleCompensation { source, .. } => Some(source),
            Self::EmptyField { .. } => None,
            Self::UnknownPreset { .. } => None,
            Self::SplitWithoutItsSpeeds { .. } => None,
        }
    }
}

/// The outcome of mapping a material record: the domain profile, and the
/// printing advice the source glued onto its name, which no profile of
/// ours records.
pub(crate) struct MappedMaterial {
    pub profile: MaterialProfile,
    pub advice: Option<String>,
}

/// Convert one DragonFruit material record into a [`MaterialProfile`],
/// keying its calibration blocks to the machines `presets` names.
///
/// The profile is named after the resin, so the records of one resin
/// converge on one profile with a block per machine.
///
/// The record points at printers by the source's preset ids, which carry
/// the output container the preset writes; ours name the machine alone.
/// So a record whose presets are absent from `presets` cannot be mapped,
/// rather than being keyed to an id no printer has.
pub(crate) fn map_material(
    raw: RawMaterial,
    presets: &PresetIndex,
) -> Result<MappedMaterial, MaterialMapError> {
    let raw_id = raw.template_id.clone();

    let brand = non_empty(&raw.brand, &raw_id, "brand")?;
    let (resin, advice) = split_advice(&raw.name);
    let name = non_empty(resin, &raw_id, "name")?;
    let family = non_empty(&raw.resin_family, &raw_id, "resinFamily")?;
    let id = material_id_of(brand.as_str(), name.as_str()).map_err(|source| {
        MaterialMapError::InvalidId {
            id: raw_id.clone(),
            source,
        }
    })?;

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
        let over = raw.local_settings_by_output.get(&entry.container);
        let motion = Motion::of(&raw, over, entry.splits_travel, &raw_id)?;
        per_printer.insert(
            CalibrationTarget::from(entry.printer.clone()),
            with_pauses_and_light(motion.into_block(shared), over, &raw_id)?,
        );
    }

    Ok(MappedMaterial {
        advice: advice.map(str::to_owned),
        profile: MaterialProfile {
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
        },
    })
}

/// How the platform moves over a layer, for one machine.
///
/// The record states this twice: once flat at its top level, and once
/// per container under `localSettingsByOutput`, where the bottom layers
/// get their own set. The container's is the one a machine follows.
struct Motion {
    normal: Stages,
    bottom: Stages,
}

/// One quantity as a calibration block carries it: the move, plus the
/// second stage when the machine splits its travel.
#[derive(Default)]
struct Stages {
    lift_distance_mm: Option<PositiveFiniteMm>,
    lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    second: Option<SecondTravelStage>,
}

impl Motion {
    /// Read the motion for the container `over` applies to, falling back
    /// to the record's flat fields when the record states none for it.
    ///
    /// `splits_travel` picks which of the source's 2 views is taken: the
    /// machine's own split, or the whole move in one stage.
    fn of(
        raw: &RawMaterial,
        over: Option<&RawOverride>,
        splits_travel: bool,
        id: &str,
    ) -> Result<Self, MaterialMapError> {
        let Some(over) = over else {
            return Ok(Self {
                normal: Stages {
                    lift_distance_mm: opt_mm(raw.lift_distance_mm, id, NORMAL_KEYS.distance)?,
                    lift_speed_mm_min: opt_speed(
                        raw.lift_speed_mm_min,
                        id,
                        NORMAL_KEYS.lift_speed,
                    )?,
                    retract_speed_mm_min: opt_speed(
                        raw.retract_speed_mm_min,
                        id,
                        NORMAL_KEYS.retract_speed,
                    )?,
                    second: None,
                },
                bottom: Stages::default(),
            });
        };
        Ok(Self {
            normal: Stages::of(over.normal_stages(), NORMAL_KEYS, splits_travel, id)?,
            bottom: Stages::of(over.bottom_stages(), BOTTOM_KEYS, splits_travel, id)?,
        })
    }

    /// Put this motion into `block`, which carries everything a machine
    /// does not change.
    fn into_block(self, block: MaterialPerPrinter) -> MaterialPerPrinter {
        MaterialPerPrinter {
            lift_distance_mm: self.normal.lift_distance_mm,
            lift_speed_mm_min: self.normal.lift_speed_mm_min,
            retract_speed_mm_min: self.normal.retract_speed_mm_min,
            second_travel: self.normal.second,
            bottom_lift_distance_mm: self.bottom.lift_distance_mm,
            bottom_lift_speed_mm_min: self.bottom.lift_speed_mm_min,
            bottom_retract_speed_mm_min: self.bottom.retract_speed_mm_min,
            bottom_second_travel: self.bottom.second,
            ..block
        }
    }
}

impl Stages {
    /// Resolve one quantity's numbers into what a block carries.
    ///
    /// A machine that splits keeps the source's 2 stages, and its
    /// retract speeds cross over: the source's first speed covers the
    /// second stage's distance on the way down, so it belongs to the
    /// stage, while its second is the final approach. A machine that
    /// does not split gets the whole rise in one, which is the 2
    /// distances added, as the record's own flat field is.
    fn of(
        raw: RawStages,
        keys: StageKeys,
        splits_travel: bool,
        id: &str,
    ) -> Result<Self, MaterialMapError> {
        let second = if splits_travel {
            second_stage(&raw, keys, id)?
        } else {
            None
        };
        let Some(second) = second else {
            return Ok(Self {
                lift_distance_mm: opt_mm(
                    total(raw.lift_distance, raw.lift_distance_2),
                    id,
                    keys.distance,
                )?,
                lift_speed_mm_min: opt_speed(raw.lift_speed, id, keys.lift_speed)?,
                retract_speed_mm_min: opt_speed(raw.retract_speed, id, keys.retract_speed)?,
                second: None,
            });
        };
        Ok(Self {
            lift_distance_mm: opt_mm(raw.lift_distance, id, keys.distance)?,
            lift_speed_mm_min: opt_speed(raw.lift_speed, id, keys.lift_speed)?,
            retract_speed_mm_min: opt_speed(raw.retract_speed_2, id, keys.retract_speed)?,
            second: Some(second),
        })
    }
}

/// Put the pauses around a layer and the light power into `block`.
///
/// The record states these per container and nowhere else, so a machine
/// whose container the record says nothing about takes none of them. The
/// bottom layers get their own only where the source states them, which
/// it never does for these.
fn with_pauses_and_light(
    block: MaterialPerPrinter,
    over: Option<&RawOverride>,
    id: &str,
) -> Result<MaterialPerPrinter, MaterialMapError> {
    let Some(over) = over else {
        return Ok(block);
    };
    Ok(MaterialPerPrinter {
        wait_before_cure_sec: opt_wait(
            over.wait_time_before_cure_sec,
            id,
            "waitTimeBeforeCureSec",
        )?,
        wait_after_cure_sec: opt_wait(over.wait_time_after_cure_sec, id, "waitTimeAfterCureSec")?,
        wait_after_lift_sec: opt_wait(over.wait_time_after_lift_sec, id, "waitTimeAfterLiftSec")?,
        light_pwm: opt_light_pwm(over.projector_pwm_percent, id, "projectorPwmPercent")?,
        ..block
    })
}

/// Build the second stage the record states, or `None` when its distance
/// is 0, which is how a machine that moves in one go is written.
///
/// A positive second distance with no speed to cross it is a record that
/// splits the travel without saying how, so it is refused rather than
/// quietly flattened.
fn second_stage(
    raw: &RawStages,
    keys: StageKeys,
    id: &str,
) -> Result<Option<SecondTravelStage>, MaterialMapError> {
    let Some(distance) = raw.lift_distance_2.filter(|value| *value > 0.0) else {
        return Ok(None);
    };
    let missing = |field: &'static str| MaterialMapError::SplitWithoutItsSpeeds {
        id: id.to_owned(),
        field,
    };
    let lift_speed = raw.lift_speed_2.ok_or_else(|| missing(keys.lift_speed))?;
    let retract_speed = raw
        .retract_speed
        .ok_or_else(|| missing(keys.retract_speed))?;
    Ok(Some(SecondTravelStage {
        lift_distance_mm: mm(distance, id, keys.distance)?,
        lift_speed_mm_min: speed(lift_speed, id, keys.lift_speed)?,
        retract_speed_mm_min: speed(retract_speed, id, keys.retract_speed)?,
    }))
}

/// Add a quantity's 2 stages, treating an absent stage as 0 and an
/// absent pair as no value at all.
fn total(first: Option<f64>, second: Option<f64>) -> Option<f64> {
    match (first, second) {
        (None, None) => None,
        (first, second) => Some(first.unwrap_or_default() + second.unwrap_or_default()),
    }
}

/// Name a resin the way a printer profile names a machine: from what
/// the record calls it, never from the key the source files it under.
///
/// The source files one record per machine a resin is tuned for, so its
/// key carries the machine and cannot identify the resin.
fn material_id_of(brand: &str, name: &str) -> Result<MaterialId, InvalidProfileId> {
    let parts = format!("{brand} {name}");
    let slug = parts
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("-");
    MaterialId::new(&slug)
}

/// Split a source name into what names the resin and the printing
/// advice the source glues onto it for some machines.
///
/// The source attaches advice with a hyphen, `Blu Nylon Black-(Use Extra
/// Heavy support)`, and attaches a colour or a version with a space,
/// `Blu (Lava Black)`, which is what tells 2 resins apart.
fn split_advice(name: &str) -> (&str, Option<&str>) {
    let trimmed = name.trim_end();
    let Some(rest) = trimmed.strip_suffix(')') else {
        return (name, None);
    };
    let Some(open) = rest.rfind("-(") else {
        return (name, None);
    };
    let named = rest[..open].trim_end();
    let advice = rest[open + 2..].trim();
    if named.is_empty() || advice.is_empty() {
        return (name, None);
    }
    (named, Some(advice))
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

/// Map a millimetre value, reporting an out-of-range one.
fn mm(value: f64, id: &str, field: &'static str) -> Result<PositiveFiniteMm, MaterialMapError> {
    PositiveFiniteMm::new(value).map_err(|source| MaterialMapError::Length {
        id: id.to_owned(),
        field,
        source,
    })
}

/// Map an optional millimetre value, reporting an out-of-range one.
fn opt_mm(
    value: Option<f64>,
    id: &str,
    field: &'static str,
) -> Result<Option<PositiveFiniteMm>, MaterialMapError> {
    value.map(|raw| mm(raw, id, field)).transpose()
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

/// Map an optional pause, reporting one that is not a duration.
fn opt_wait(
    value: Option<f64>,
    id: &str,
    field: &'static str,
) -> Result<Option<NonNegativeFiniteSec>, MaterialMapError> {
    value
        .map(|raw| {
            NonNegativeFiniteSec::new(raw).map_err(|source| MaterialMapError::Wait {
                id: id.to_owned(),
                field,
                source,
            })
        })
        .transpose()
}

/// Map an optional light power, reporting one outside the panel's range.
fn opt_light_pwm(
    value: Option<f64>,
    id: &str,
    field: &'static str,
) -> Result<Option<LightPwm>, MaterialMapError> {
    value
        .map(|raw| {
            LightPwm::new(raw).map_err(|source| MaterialMapError::LightPower {
                id: id.to_owned(),
                field,
                source,
            })
        })
        .transpose()
}

/// Map a speed value, reporting an out-of-range one.
fn speed(
    value: f64,
    id: &str,
    field: &'static str,
) -> Result<PositiveFiniteMmPerMin, MaterialMapError> {
    PositiveFiniteMmPerMin::new(value).map_err(|source| MaterialMapError::Speed {
        id: id.to_owned(),
        field,
        source,
    })
}

/// Map an optional speed value, reporting an out-of-range one.
fn opt_speed(
    value: Option<f64>,
    id: &str,
    field: &'static str,
) -> Result<Option<PositiveFiniteMmPerMin>, MaterialMapError> {
    value.map(|raw| speed(raw, id, field)).transpose()
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
            splits_travel: false,
        }
    }

    /// The same preset, for a machine our library says splits its
    /// travel.
    fn splitting_preset(id: &str, container: &str) -> Preset {
        Preset {
            splits_travel: true,
            ..preset(id, container)
        }
    }

    fn map(json: &str) -> Result<MaterialProfile, MaterialMapError> {
        map_with(json, &presets())
    }

    fn map_with(json: &str, presets: &PresetIndex) -> Result<MaterialProfile, MaterialMapError> {
        map_material(serde_json::from_str(json).unwrap(), presets).map(|mapped| mapped.profile)
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
            MaterialId::new("testresin-standard-405").unwrap(),
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
                "bottomLiftSpeedMmMin": 40.0, "bottomLiftSpeed2MmMin": 220.0,
                "bottomRetractSpeedMmMin": 210.0, "bottomRetractSpeed2MmMin": 50.0,
                "waitTimeBeforeCureSec": 0.5, "waitTimeAfterCureSec": 2.0,
                "waitTimeAfterLiftSec": 0.0, "projectorPwmPercent": 100
            },
            ".ctb": {
                "liftDistanceMm": 8.0, "liftDistance2Mm": 0.0,
                "liftSpeedMmMin": 50.0, "retractSpeedMmMin": 200.0,
                "bottomLiftDistanceMm": 8.0, "bottomLiftDistance2Mm": 0.0,
                "bottomLiftSpeedMmMin": 35.0, "bottomRetractSpeedMmMin": 180.0,
                "waitTimeBeforeCureSec": 1.5, "waitTimeAfterCureSec": 3.0,
                "waitTimeAfterLiftSec": 0.25, "projectorPwmPercent": 80
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
    fn a_machine_that_splits_its_travel_keeps_the_2_stages() {
        let mut presets = presets();
        presets.insert("printer-a-goo", splitting_preset("printer-a", ".goo"));

        let profile = map_with(WITH_OVERRIDES, &presets).unwrap();
        let block = block_of(&profile, "printer-a");

        // Up: the slow peel, then the rest of the rise.
        assert_eq!(block.lift_distance_mm.map(|v| v.value()), Some(3.0));
        assert_eq!(block.lift_speed_mm_min.map(|v| v.value()), Some(45.0));
        let second = block.second_travel.unwrap();
        assert_eq!(second.lift_distance_mm.value(), 4.0);
        assert_eq!(second.lift_speed_mm_min.value(), 240.0);
        // Down: the second stage's distance first, at the speed the
        // source writes first, then the peel at the one it writes
        // second. The 2 cross over between their model and ours.
        assert_eq!(second.retract_speed_mm_min.value(), 240.0);
        assert_eq!(block.retract_speed_mm_min.map(|v| v.value()), Some(60.0));

        let bottom = block.bottom_second_travel.unwrap();
        assert_eq!(block.bottom_lift_distance_mm.map(|v| v.value()), Some(3.0));
        assert_eq!(bottom.lift_distance_mm.value(), 4.0);
        assert_eq!(bottom.lift_speed_mm_min.value(), 220.0);
        assert_eq!(bottom.retract_speed_mm_min.value(), 210.0);
        assert_eq!(
            block.bottom_retract_speed_mm_min.map(|v| v.value()),
            Some(50.0)
        );
    }

    #[test]
    fn a_machine_that_does_not_split_gets_no_stage_at_all() {
        let profile = map(WITH_OVERRIDES).unwrap();
        let block = block_of(&profile, "printer-a");

        assert_eq!(block.lift_distance_mm.map(|v| v.value()), Some(7.0));
        assert_eq!(block.second_travel, None);
        assert_eq!(block.bottom_second_travel, None);
    }

    #[test]
    fn a_split_the_record_states_no_speed_for_is_refused() {
        let json = r#"{
            "templateId": "half-split", "profileVersion": 1,
            "name": "Half", "brand": "TestResin", "resinFamily": "standard",
            "validForPresets": ["printer-a-goo"],
            "liftDistanceMm": 7.0, "liftSpeedMmMin": 45.0, "retractSpeedMmMin": 240.0,
            "localSettingsByOutput": {
                ".goo": {
                    "liftDistanceMm": 3.0, "liftDistance2Mm": 4.0,
                    "liftSpeedMmMin": 45.0, "retractSpeedMmMin": 240.0
                }
            }
        }"#;
        let mut presets = presets();
        presets.insert("printer-a-goo", splitting_preset("printer-a", ".goo"));

        match map_with(json, &presets).unwrap_err() {
            MaterialMapError::SplitWithoutItsSpeeds { field, .. } => {
                assert_eq!(field, "liftSpeedMmMin");
            }
            other => panic!("expected SplitWithoutItsSpeeds, got {other}"),
        }
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
    fn each_machine_takes_the_pauses_and_light_power_of_its_container() {
        let profile = map(WITH_OVERRIDES).unwrap();

        let goo = block_of(&profile, "printer-a");
        assert_eq!(goo.wait_before_cure_sec.map(|v| v.value()), Some(0.5));
        assert_eq!(goo.wait_after_cure_sec.map(|v| v.value()), Some(2.0));
        assert_eq!(goo.wait_after_lift_sec.map(|v| v.value()), Some(0.0));
        assert_eq!(goo.light_pwm.map(|v| v.percent()), Some(100.0));

        let ctb = block_of(&profile, "printer-b");
        assert_eq!(ctb.wait_before_cure_sec.map(|v| v.value()), Some(1.5));
        assert_eq!(ctb.wait_after_cure_sec.map(|v| v.value()), Some(3.0));
        assert_eq!(ctb.wait_after_lift_sec.map(|v| v.value()), Some(0.25));
        assert_eq!(ctb.light_pwm.map(|v| v.percent()), Some(80.0));
    }

    /// The source states one set of pauses and one light power per
    /// container, with nothing said about the bottom layers.
    #[test]
    fn the_bottom_layers_get_no_pauses_and_no_light_power_of_their_own() {
        let profile = map(WITH_OVERRIDES).unwrap();
        let goo = block_of(&profile, "printer-a");

        assert_eq!(goo.bottom_wait_before_cure_sec, None);
        assert_eq!(goo.bottom_wait_after_cure_sec, None);
        assert_eq!(goo.bottom_wait_after_lift_sec, None);
        assert_eq!(goo.bottom_light_pwm, None);
    }

    /// The flat fields carry no pause and no light power, so a container
    /// the record says nothing about has none to take.
    #[test]
    fn a_container_the_record_says_nothing_about_gets_no_pauses() {
        let profile = map(WITH_OVERRIDES).unwrap();
        let pwmx = block_of(&profile, "printer-c");

        assert_eq!(pwmx.wait_before_cure_sec, None);
        assert_eq!(pwmx.wait_after_cure_sec, None);
        assert_eq!(pwmx.wait_after_lift_sec, None);
        assert_eq!(pwmx.light_pwm, None);
    }

    #[test]
    fn a_negative_pause_is_rejected() {
        let json = r#"{
            "templateId": "resin", "profileVersion": 1,
            "name": "Resin", "brand": "Default", "resinFamily": "standard",
            "validForPresets": ["printer-a-goo"],
            "localSettingsByOutput": {
                ".goo": { "waitTimeAfterCureSec": -1.0 }
            }
        }"#;
        assert!(matches!(
            map(json).unwrap_err(),
            MaterialMapError::Wait {
                field: "waitTimeAfterCureSec",
                ..
            },
        ));
    }

    #[test]
    fn a_light_power_beyond_full_is_rejected() {
        let json = r#"{
            "templateId": "resin", "profileVersion": 1,
            "name": "Resin", "brand": "Default", "resinFamily": "standard",
            "validForPresets": ["printer-a-goo"],
            "localSettingsByOutput": {
                ".goo": { "projectorPwmPercent": 150 }
            }
        }"#;
        assert!(matches!(
            map(json).unwrap_err(),
            MaterialMapError::LightPower {
                field: "projectorPwmPercent",
                ..
            },
        ));
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

    /// The source files a resin under one id per machine it is tuned
    /// for. That is their filing, not the resin's identity: ours names
    /// the resin, the way a printer id names the machine.
    #[test]
    fn a_resin_is_identified_by_its_brand_and_name() {
        let json = r#"{
            "templateId": "sirayatech-blu-obsidian-black-v3-saturn-4", "profileVersion": 1,
            "name": "Blu (Obsidian Black/V3)", "brand": "Siraya Tech", "resinFamily": "standard"
        }"#;
        let profile = map(json).unwrap();

        assert_eq!(
            profile.identity.id.as_str(),
            "siraya-tech-blu-obsidian-black-v3",
        );
    }

    /// The source glues printing advice to the name on some machines.
    /// It names no resin, so it names no profile either.
    #[test]
    fn advice_glued_to_the_name_does_not_make_a_second_resin() {
        let record = |name: &str| {
            format!(
                r#"{{
                    "templateId": "key", "profileVersion": 1,
                    "name": "{name}", "brand": "Siraya Tech", "resinFamily": "standard"
                }}"#
            )
        };
        let plain = map(&record("Blu Nylon Black")).unwrap();
        let advised = map(&record("Blu Nylon Black-(Use Extra Heavy support)")).unwrap();

        assert_eq!(plain.identity.id, advised.identity.id);
        assert_eq!(plain.identity.id.as_str(), "siraya-tech-blu-nylon-black");
    }

    /// A parenthetical the source attaches with a space is the colour or
    /// the version, which is what makes 2 resins 2 resins.
    #[test]
    fn a_colour_stays_part_of_the_resin_it_names() {
        let record = |name: &str| {
            format!(
                r#"{{
                    "templateId": "key", "profileVersion": 1,
                    "name": "{name}", "brand": "Siraya Tech", "resinFamily": "standard"
                }}"#
            )
        };
        let black = map(&record("Blu (Lava Black)")).unwrap();
        let grey = map(&record("Blu (Lava Grey)")).unwrap();

        assert_ne!(black.identity.id, grey.identity.id);
        assert_eq!(black.identity.id.as_str(), "siraya-tech-blu-lava-black");
    }

    #[test]
    fn a_brand_and_name_that_spell_no_id_are_rejected() {
        let json = r#"{
            "templateId": "resin", "profileVersion": 1,
            "name": "???", "brand": "!!!", "resinFamily": "standard"
        }"#;
        assert!(matches!(
            map(json).unwrap_err(),
            MaterialMapError::InvalidId { .. },
        ));
    }

    /// A record whose filing key breaks our grammar still converts:
    /// nothing of ours is derived from it.
    #[test]
    fn a_filing_key_we_could_not_use_as_an_id_is_no_obstacle() {
        let json = r#"{
            "templateId": "Bad_Id", "profileVersion": 1,
            "name": "Bad", "brand": "Default", "resinFamily": "standard"
        }"#;
        let profile = map(json).unwrap();

        assert_eq!(profile.identity.id.as_str(), "default-bad");
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
