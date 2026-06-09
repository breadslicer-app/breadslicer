/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

#![allow(dead_code)]

//! Composition of a printer profile and a material profile into a
//! set of picked values.
//!
//! This module picks calibrated values from the material's
//! per-printer block, falls back to material-level defaults where
//! the per-printer entry did not supply a value, and derives from
//! material intent where the prior two layers fell through and the
//! intent has the inputs needed.
//!
//! Per-field coverage:
//!
//! - `layer_height_mm`: intent passthrough only
//!   (`target_layer_height_mm`).
//! - `normal_exposure_sec`: per-printer; or derived from
//!   `target_energy_density_mj_per_cm2` divided by the printer's
//!   declared `light_intensity_mw_per_cm2`.
//! - `bottom_exposure_sec`: per-printer only. No automatic
//!   derivation; bottom exposure varies too widely per material
//!   for a generic formula.
//! - `bottom_layer_count`: per-printer; or material default.
//! - `lift_distance_mm`, `lift_speed_mm_min`,
//!   `retract_speed_mm_min`: per-printer only.
//! - `scale_compensation_pct`: material default only. No
//!   `OverrideRecord` is emitted for this field today since the
//!   compound shape does not fit the scalar `final_value: f64`
//!   field on the record; the `DefaultUsed` warning still fires.
//!
//! Inputs that are silently incomplete (e.g. an intent that names
//! a target energy density but a printer that does not declare its
//! light intensity) cause the corresponding derivation to skip
//! without a warning; the field is left unset, deferring to the
//! override layer or to the assembly-time required-field check.

use sc_print_primitives::{
    PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, ScaleCompensation,
};

use crate::material_defaults::MaterialDefaults;
use crate::material_intent::MaterialIntent;
use crate::material_per_printer::MaterialPerPrinter;
use crate::material_profile::MaterialProfile;
use crate::override_record::OverrideRecord;
use crate::policy::{
    AUTOSNAP_WARNING_THRESHOLD, ApplyResult, policies_for, relative_delta_exceeds,
};
use crate::printer_constraints::PrinterConstraints;
use crate::printer_profile::PrinterProfile;
use crate::resolve_error::ResolveError;
use crate::resolve_warning::{IntentSource, ResolveWarning};
use crate::resolved_field::ResolvedField;
use crate::value_source::ValueSource;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PickedValues {
    pub layer_height_mm: Option<PositiveFiniteMm>,
    pub normal_exposure_sec: Option<PositiveFiniteSec>,
    pub bottom_exposure_sec: Option<PositiveFiniteSec>,
    pub bottom_layer_count: Option<u32>,
    pub lift_distance_mm: Option<PositiveFiniteMm>,
    pub lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub scale_compensation_pct: Option<ScaleCompensation>,
}

pub(crate) fn pick_and_derive_material(
    printer: &PrinterProfile,
    material: &MaterialProfile,
) -> (PickedValues, Vec<ResolveWarning>, Vec<OverrideRecord>) {
    let mut picked = PickedValues::default();
    let mut warnings = Vec::new();
    let mut records = Vec::new();

    if let Some(per_printer) = material.per_printer.get(&printer.identity.id) {
        apply_per_printer(per_printer, &mut picked, &mut records);
    }
    apply_defaults(&material.defaults, &mut picked, &mut warnings, &mut records);
    apply_intent_derivation(
        &material.intent,
        &printer.constraints,
        &mut picked,
        &mut warnings,
        &mut records,
    );

    (picked, warnings, records)
}

fn apply_per_printer(
    per_printer: &MaterialPerPrinter,
    picked: &mut PickedValues,
    records: &mut Vec<OverrideRecord>,
) {
    if let Some(value) = per_printer.normal_exposure_sec {
        picked.normal_exposure_sec = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::NormalExposureSec,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.bottom_exposure_sec {
        picked.bottom_exposure_sec = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomExposureSec,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.bottom_layer_count {
        picked.bottom_layer_count = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomLayerCount,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: f64::from(value),
        });
    }
    if let Some(value) = per_printer.lift_distance_mm {
        picked.lift_distance_mm = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::LiftDistanceMm,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.lift_speed_mm_min {
        picked.lift_speed_mm_min = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::LiftSpeedMmMin,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.retract_speed_mm_min {
        picked.retract_speed_mm_min = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::RetractSpeedMmMin,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
}

fn apply_defaults(
    defaults: &MaterialDefaults,
    picked: &mut PickedValues,
    warnings: &mut Vec<ResolveWarning>,
    records: &mut Vec<OverrideRecord>,
) {
    if picked.bottom_layer_count.is_none()
        && let Some(value) = defaults.bottom_layer_count
    {
        picked.bottom_layer_count = Some(value);
        warnings.push(ResolveWarning::DefaultUsed {
            field: ResolvedField::BottomLayerCount,
        });
        records.push(OverrideRecord {
            field: ResolvedField::BottomLayerCount,
            source: ValueSource::Default,
            prior_value: None,
            final_value: f64::from(value),
        });
    }
    if picked.scale_compensation_pct.is_none()
        && let Some(value) = defaults.scale_compensation_pct
    {
        picked.scale_compensation_pct = Some(value);
        warnings.push(ResolveWarning::DefaultUsed {
            field: ResolvedField::ScaleCompensationPct,
        });
    }
}

fn apply_intent_derivation(
    intent: &MaterialIntent,
    constraints: &PrinterConstraints,
    picked: &mut PickedValues,
    warnings: &mut Vec<ResolveWarning>,
    records: &mut Vec<OverrideRecord>,
) {
    if picked.layer_height_mm.is_none()
        && let Some(value) = intent.target_layer_height_mm
    {
        picked.layer_height_mm = Some(value);
        warnings.push(ResolveWarning::DerivedFromIntent {
            field: ResolvedField::LayerHeightMm,
            source: IntentSource::TargetLayerHeight,
        });
        records.push(OverrideRecord {
            field: ResolvedField::LayerHeightMm,
            source: ValueSource::Derived,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if picked.normal_exposure_sec.is_none()
        && let (Some(energy_density), Some(light_intensity)) = (
            intent.target_energy_density_mj_per_cm2,
            constraints.light_intensity_mw_per_cm2,
        )
        && let Ok(typed) = PositiveFiniteSec::new(energy_density.value() / light_intensity.value())
    {
        picked.normal_exposure_sec = Some(typed);
        warnings.push(ResolveWarning::DerivedFromIntent {
            field: ResolvedField::NormalExposureSec,
            source: IntentSource::TargetEnergyDensity,
        });
        records.push(OverrideRecord {
            field: ResolvedField::NormalExposureSec,
            source: ValueSource::Derived,
            prior_value: None,
            final_value: typed.value(),
        });
    }
}

pub(crate) fn apply_constraints(
    picked: &mut PickedValues,
    constraints: &PrinterConstraints,
    warnings: &mut Vec<ResolveWarning>,
) -> Result<(), ResolveError> {
    if let Some(value) = picked.layer_height_mm {
        let snapped = apply_field_policies(
            ResolvedField::LayerHeightMm,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.layer_height_mm = Some(
            PositiveFiniteMm::new(snapped).expect("snapped layer height stays positive-finite"),
        );
    }
    if let Some(value) = picked.lift_distance_mm {
        let snapped = apply_field_policies(
            ResolvedField::LiftDistanceMm,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.lift_distance_mm = Some(
            PositiveFiniteMm::new(snapped).expect("snapped lift distance stays positive-finite"),
        );
    }
    if let Some(value) = picked.lift_speed_mm_min {
        let snapped = apply_field_policies(
            ResolvedField::LiftSpeedMmMin,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.lift_speed_mm_min = Some(
            PositiveFiniteMmPerMin::new(snapped).expect("snapped lift speed stays positive-finite"),
        );
    }
    if let Some(value) = picked.retract_speed_mm_min {
        let snapped = apply_field_policies(
            ResolvedField::RetractSpeedMmMin,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.retract_speed_mm_min = Some(
            PositiveFiniteMmPerMin::new(snapped)
                .expect("snapped retract speed stays positive-finite"),
        );
    }
    Ok(())
}

fn apply_field_policies(
    field: ResolvedField,
    initial: f64,
    constraints: &PrinterConstraints,
    warnings: &mut Vec<ResolveWarning>,
) -> Result<f64, ResolveError> {
    let mut current = initial;
    for policy in policies_for(field, constraints) {
        match policy.apply(current) {
            ApplyResult::Unchanged => {}
            ApplyResult::Snapped { snapped, reason } => {
                if relative_delta_exceeds(current, snapped, AUTOSNAP_WARNING_THRESHOLD) {
                    warnings.push(ResolveWarning::Autosnap {
                        field,
                        requested: current,
                        snapped,
                        reason,
                    });
                }
                current = snapped;
            }
            ApplyResult::RejectedBelow { minimum } => {
                return Err(ResolveError::RejectedBelowMinimum {
                    field,
                    value: current,
                    minimum,
                });
            }
        }
    }
    Ok(current)
}
