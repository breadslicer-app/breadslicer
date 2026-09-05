/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Composition of a printer profile, an effective material, and a
//! set of per-print scene overrides into a [`ResolvedPrintConfig`].
//!
//! The public entry point is [`resolve`]. The pipeline runs the
//! following layers:
//!
//! - Pick calibrated values from the material's per-printer block
//!   matched to the printer's id, else from its `generic-machine`
//!   block (recorded as a warning), then fall back to material-level
//!   defaults for fields that have defaults coverage
//!   (`bottom_layer_count`, `scale_compensation_pct`, the two light
//!   PWMs and the three normal-layer wait times; the bottom waits
//!   and the bottom motion are calibrated per printer or not at
//!   all).
//! - Derive from material intent where the prior layer left a
//!   field unset and the intent has the inputs needed. Today only
//!   `layer_height_mm` (intent passthrough from
//!   `target_layer_height_mm`) and `normal_exposure_sec` (derived
//!   from `target_energy_density_mj_per_cm2` divided by the
//!   printer's declared `light_intensity_mw_per_cm2` at the picked
//!   `light_pwm` duty) are derivable.
//! - Validate each picked value against the printer's constraints:
//!   snap-to-multiple, snap-into-range, snap-at-max, or
//!   reject-below-min per the policy table. Snaps that exceed the
//!   warning threshold emit an `Autosnap` warning.
//! - Apply per-print scene overrides. Each override overwrites
//!   the picked value, emits an `OverrideRecord` with the prior
//!   value captured, and re-runs the per-field policy chain so
//!   that the override is itself constrained.
//! - Assemble the resolved configuration. Any required field
//!   still unset surfaces as
//!   [`ResolveError::MissingRequiredField`]; a wait still unset
//!   resolves to zero instead, which is what a resin naming no
//!   pause is asking for, a bottom wait or bottom motion still unset
//!   resolves to its normal-layer counterpart, and a light PWM still
//!   unset resolves to full power.
//!
//! `bottom_exposure_sec` has no automatic derivation; it must be
//! supplied via calibration or override. Inputs that are silently
//! incomplete (e.g. an intent that names a target energy density
//! but a printer that does not declare its light intensity) cause
//! the corresponding derivation to skip without a warning; the
//! field is left unset, deferring to the override layer or to the
//! assembly-time required-field check.

use sc_print_primitives::{
    LightPwm, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
    ScaleCompensation,
};

use crate::effective_material::EffectiveMaterial;
use crate::material_defaults::MaterialDefaults;
use crate::material_intent::MaterialIntent;
use crate::material_per_printer::MaterialPerPrinter;
use crate::material_resolved::MaterialResolved;
use crate::override_record::OverrideRecord;
use crate::policy::{
    AUTOSNAP_WARNING_THRESHOLD, ApplyResult, ConstraintPolicy, policies_for, relative_delta_exceeds,
};
use crate::printer_constraints::PrinterConstraints;
use crate::printer_profile::PrinterProfile;
use crate::resolve_error::ResolveError;
use crate::resolve_warning::{IntentSource, ResolveWarning};
use crate::resolved_field::ResolvedField;
use crate::resolved_print_config::ResolvedPrintConfig;
use crate::scene_overrides::SceneOverrides;
use crate::value_source::ValueSource;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PickedValues {
    pub layer_height_mm: Option<PositiveFiniteMm>,
    pub normal_exposure_sec: Option<PositiveFiniteSec>,
    pub bottom_exposure_sec: Option<PositiveFiniteSec>,
    pub bottom_layer_count: Option<u32>,
    pub wait_before_cure_sec: Option<NonNegativeFiniteSec>,
    pub wait_after_cure_sec: Option<NonNegativeFiniteSec>,
    pub wait_after_lift_sec: Option<NonNegativeFiniteSec>,
    pub bottom_wait_before_cure_sec: Option<NonNegativeFiniteSec>,
    pub bottom_wait_after_cure_sec: Option<NonNegativeFiniteSec>,
    pub bottom_wait_after_lift_sec: Option<NonNegativeFiniteSec>,
    pub lift_distance_mm: Option<PositiveFiniteMm>,
    pub lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub bottom_lift_distance_mm: Option<PositiveFiniteMm>,
    pub bottom_lift_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub bottom_retract_speed_mm_min: Option<PositiveFiniteMmPerMin>,
    pub light_pwm: Option<LightPwm>,
    pub bottom_light_pwm: Option<LightPwm>,
    pub scale_compensation_pct: Option<ScaleCompensation>,
}

pub(crate) fn pick_and_derive_material(
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
) -> (PickedValues, Vec<ResolveWarning>, Vec<OverrideRecord>) {
    let mut picked = PickedValues::default();
    let mut warnings = Vec::new();
    let mut records = Vec::new();

    if let Some((key, block)) = material.calibration_for(&printer.identity.id) {
        apply_per_printer(block, &mut picked, &mut records);
        if key.printer_id() != &printer.identity.id {
            warnings.push(ResolveWarning::GenericCalibrationUsed);
        }
    }
    apply_defaults(
        material.defaults(),
        &mut picked,
        &mut warnings,
        &mut records,
    );
    apply_intent_derivation(
        material.intent(),
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
    if let Some(value) = per_printer.wait_before_cure_sec {
        picked.wait_before_cure_sec = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::WaitBeforeCureSec,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.wait_after_cure_sec {
        picked.wait_after_cure_sec = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::WaitAfterCureSec,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.wait_after_lift_sec {
        picked.wait_after_lift_sec = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::WaitAfterLiftSec,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.bottom_wait_before_cure_sec {
        picked.bottom_wait_before_cure_sec = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomWaitBeforeCureSec,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.bottom_wait_after_cure_sec {
        picked.bottom_wait_after_cure_sec = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomWaitAfterCureSec,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.bottom_wait_after_lift_sec {
        picked.bottom_wait_after_lift_sec = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomWaitAfterLiftSec,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
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
    if let Some(value) = per_printer.bottom_lift_distance_mm {
        picked.bottom_lift_distance_mm = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomLiftDistanceMm,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.bottom_lift_speed_mm_min {
        picked.bottom_lift_speed_mm_min = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomLiftSpeedMmMin,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.bottom_retract_speed_mm_min {
        picked.bottom_retract_speed_mm_min = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomRetractSpeedMmMin,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if let Some(value) = per_printer.light_pwm {
        picked.light_pwm = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::LightPwm,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.percent(),
        });
    }
    if let Some(value) = per_printer.bottom_light_pwm {
        picked.bottom_light_pwm = Some(value);
        records.push(OverrideRecord {
            field: ResolvedField::BottomLightPwm,
            source: ValueSource::Calibrated,
            prior_value: None,
            final_value: value.percent(),
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
    if picked.wait_before_cure_sec.is_none()
        && let Some(value) = defaults.wait_before_cure_sec
    {
        picked.wait_before_cure_sec = Some(value);
        warnings.push(ResolveWarning::DefaultUsed {
            field: ResolvedField::WaitBeforeCureSec,
        });
        records.push(OverrideRecord {
            field: ResolvedField::WaitBeforeCureSec,
            source: ValueSource::Default,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if picked.wait_after_cure_sec.is_none()
        && let Some(value) = defaults.wait_after_cure_sec
    {
        picked.wait_after_cure_sec = Some(value);
        warnings.push(ResolveWarning::DefaultUsed {
            field: ResolvedField::WaitAfterCureSec,
        });
        records.push(OverrideRecord {
            field: ResolvedField::WaitAfterCureSec,
            source: ValueSource::Default,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if picked.wait_after_lift_sec.is_none()
        && let Some(value) = defaults.wait_after_lift_sec
    {
        picked.wait_after_lift_sec = Some(value);
        warnings.push(ResolveWarning::DefaultUsed {
            field: ResolvedField::WaitAfterLiftSec,
        });
        records.push(OverrideRecord {
            field: ResolvedField::WaitAfterLiftSec,
            source: ValueSource::Default,
            prior_value: None,
            final_value: value.value(),
        });
    }
    if picked.light_pwm.is_none()
        && let Some(value) = defaults.light_pwm
    {
        picked.light_pwm = Some(value);
        warnings.push(ResolveWarning::DefaultUsed {
            field: ResolvedField::LightPwm,
        });
        records.push(OverrideRecord {
            field: ResolvedField::LightPwm,
            source: ValueSource::Default,
            prior_value: None,
            final_value: value.percent(),
        });
    }
    if picked.bottom_light_pwm.is_none()
        && let Some(value) = defaults.bottom_light_pwm
    {
        picked.bottom_light_pwm = Some(value);
        warnings.push(ResolveWarning::DefaultUsed {
            field: ResolvedField::BottomLightPwm,
        });
        records.push(OverrideRecord {
            field: ResolvedField::BottomLightPwm,
            source: ValueSource::Default,
            prior_value: None,
            final_value: value.percent(),
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
        // A duty cycle scales average optical power, so the panel
        // delivers the intensity the printer declares only at full
        // power. A duty of zero divides to a non-finite exposure and
        // leaves the field unset, which the required-field check
        // then reports.
        && let Ok(typed) = PositiveFiniteSec::new(
            energy_density.value()
                / (light_intensity.value() * picked.light_pwm.unwrap_or(LightPwm::FULL).duty()),
        )
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
    if let Some(value) = picked.bottom_lift_distance_mm {
        let snapped = apply_field_policies(
            ResolvedField::BottomLiftDistanceMm,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.bottom_lift_distance_mm = Some(
            PositiveFiniteMm::new(snapped)
                .expect("snapped bottom lift distance stays positive-finite"),
        );
    }
    if let Some(value) = picked.bottom_lift_speed_mm_min {
        let snapped = apply_field_policies(
            ResolvedField::BottomLiftSpeedMmMin,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.bottom_lift_speed_mm_min = Some(
            PositiveFiniteMmPerMin::new(snapped)
                .expect("snapped bottom lift speed stays positive-finite"),
        );
    }
    if let Some(value) = picked.bottom_retract_speed_mm_min {
        let snapped = apply_field_policies(
            ResolvedField::BottomRetractSpeedMmMin,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.bottom_retract_speed_mm_min = Some(
            PositiveFiniteMmPerMin::new(snapped)
                .expect("snapped bottom retract speed stays positive-finite"),
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

pub(crate) fn apply_scene_overrides(
    overrides: &SceneOverrides,
    picked: &mut PickedValues,
    constraints: &PrinterConstraints,
    warnings: &mut Vec<ResolveWarning>,
    records: &mut Vec<OverrideRecord>,
) -> Result<(), ResolveError> {
    if let Some(value) = overrides.layer_height_mm {
        let prior = picked.layer_height_mm.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::LayerHeightMm,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
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
    if let Some(value) = overrides.normal_exposure_sec {
        let prior = picked.normal_exposure_sec.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::NormalExposureSec,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        picked.normal_exposure_sec = Some(value);
    }
    if let Some(value) = overrides.bottom_exposure_sec {
        let prior = picked.bottom_exposure_sec.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::BottomExposureSec,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        picked.bottom_exposure_sec = Some(value);
    }
    if let Some(value) = overrides.bottom_layer_count {
        let prior = picked.bottom_layer_count.map(f64::from);
        records.push(OverrideRecord {
            field: ResolvedField::BottomLayerCount,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: f64::from(value),
        });
        picked.bottom_layer_count = Some(value);
    }
    if let Some(value) = overrides.wait_before_cure_sec {
        let prior = picked.wait_before_cure_sec.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::WaitBeforeCureSec,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        picked.wait_before_cure_sec = Some(value);
    }
    if let Some(value) = overrides.wait_after_cure_sec {
        let prior = picked.wait_after_cure_sec.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::WaitAfterCureSec,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        picked.wait_after_cure_sec = Some(value);
    }
    if let Some(value) = overrides.wait_after_lift_sec {
        let prior = picked.wait_after_lift_sec.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::WaitAfterLiftSec,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        picked.wait_after_lift_sec = Some(value);
    }
    if let Some(value) = overrides.bottom_wait_before_cure_sec {
        let prior = picked.bottom_wait_before_cure_sec.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::BottomWaitBeforeCureSec,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        picked.bottom_wait_before_cure_sec = Some(value);
    }
    if let Some(value) = overrides.bottom_wait_after_cure_sec {
        let prior = picked.bottom_wait_after_cure_sec.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::BottomWaitAfterCureSec,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        picked.bottom_wait_after_cure_sec = Some(value);
    }
    if let Some(value) = overrides.bottom_wait_after_lift_sec {
        let prior = picked.bottom_wait_after_lift_sec.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::BottomWaitAfterLiftSec,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        picked.bottom_wait_after_lift_sec = Some(value);
    }
    if let Some(value) = overrides.lift_distance_mm {
        let prior = picked.lift_distance_mm.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::LiftDistanceMm,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
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
    if let Some(value) = overrides.lift_speed_mm_min {
        let prior = picked.lift_speed_mm_min.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::LiftSpeedMmMin,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
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
    if let Some(value) = overrides.retract_speed_mm_min {
        let prior = picked.retract_speed_mm_min.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::RetractSpeedMmMin,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
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
    if let Some(value) = overrides.bottom_lift_distance_mm {
        let prior = picked.bottom_lift_distance_mm.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::BottomLiftDistanceMm,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        let snapped = apply_field_policies(
            ResolvedField::BottomLiftDistanceMm,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.bottom_lift_distance_mm = Some(
            PositiveFiniteMm::new(snapped)
                .expect("snapped bottom lift distance stays positive-finite"),
        );
    }
    if let Some(value) = overrides.bottom_lift_speed_mm_min {
        let prior = picked.bottom_lift_speed_mm_min.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::BottomLiftSpeedMmMin,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        let snapped = apply_field_policies(
            ResolvedField::BottomLiftSpeedMmMin,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.bottom_lift_speed_mm_min = Some(
            PositiveFiniteMmPerMin::new(snapped)
                .expect("snapped bottom lift speed stays positive-finite"),
        );
    }
    if let Some(value) = overrides.bottom_retract_speed_mm_min {
        let prior = picked.bottom_retract_speed_mm_min.map(|v| v.value());
        records.push(OverrideRecord {
            field: ResolvedField::BottomRetractSpeedMmMin,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.value(),
        });
        let snapped = apply_field_policies(
            ResolvedField::BottomRetractSpeedMmMin,
            value.value(),
            constraints,
            warnings,
        )?;
        picked.bottom_retract_speed_mm_min = Some(
            PositiveFiniteMmPerMin::new(snapped)
                .expect("snapped bottom retract speed stays positive-finite"),
        );
    }
    if let Some(value) = overrides.light_pwm {
        let prior = picked.light_pwm.map(|v| v.percent());
        records.push(OverrideRecord {
            field: ResolvedField::LightPwm,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.percent(),
        });
        picked.light_pwm = Some(value);
    }
    if let Some(value) = overrides.bottom_light_pwm {
        let prior = picked.bottom_light_pwm.map(|v| v.percent());
        records.push(OverrideRecord {
            field: ResolvedField::BottomLightPwm,
            source: ValueSource::Override,
            prior_value: prior,
            final_value: value.percent(),
        });
        picked.bottom_light_pwm = Some(value);
    }
    if let Some(value) = overrides.scale_compensation_pct {
        // Compound field: no OverrideRecord is emitted because the
        // scalar `final_value` field cannot represent the three
        // per-axis values.
        picked.scale_compensation_pct = Some(value);
    }
    Ok(())
}

fn assemble(
    printer: PrinterProfile,
    material: &EffectiveMaterial,
    picked: PickedValues,
    overrides_applied: Vec<OverrideRecord>,
    warnings: Vec<ResolveWarning>,
) -> Result<ResolvedPrintConfig, ResolveError> {
    let layer_height_mm = picked
        .layer_height_mm
        .ok_or(ResolveError::MissingRequiredField {
            field: ResolvedField::LayerHeightMm,
        })?;
    let normal_exposure_sec =
        picked
            .normal_exposure_sec
            .ok_or(ResolveError::MissingRequiredField {
                field: ResolvedField::NormalExposureSec,
            })?;
    let bottom_exposure_sec =
        picked
            .bottom_exposure_sec
            .ok_or(ResolveError::MissingRequiredField {
                field: ResolvedField::BottomExposureSec,
            })?;
    let bottom_layer_count =
        picked
            .bottom_layer_count
            .ok_or(ResolveError::MissingRequiredField {
                field: ResolvedField::BottomLayerCount,
            })?;
    let lift_distance_mm = picked
        .lift_distance_mm
        .ok_or(ResolveError::MissingRequiredField {
            field: ResolvedField::LiftDistanceMm,
        })?;
    let lift_speed_mm_min = picked
        .lift_speed_mm_min
        .ok_or(ResolveError::MissingRequiredField {
            field: ResolvedField::LiftSpeedMmMin,
        })?;
    let retract_speed_mm_min =
        picked
            .retract_speed_mm_min
            .ok_or(ResolveError::MissingRequiredField {
                field: ResolvedField::RetractSpeedMmMin,
            })?;
    let scale_compensation_pct =
        picked
            .scale_compensation_pct
            .ok_or(ResolveError::MissingRequiredField {
                field: ResolvedField::ScaleCompensationPct,
            })?;
    let wait_before_cure_sec = picked
        .wait_before_cure_sec
        .unwrap_or(NonNegativeFiniteSec::ZERO);
    let wait_after_cure_sec = picked
        .wait_after_cure_sec
        .unwrap_or(NonNegativeFiniteSec::ZERO);
    let wait_after_lift_sec = picked
        .wait_after_lift_sec
        .unwrap_or(NonNegativeFiniteSec::ZERO);

    Ok(ResolvedPrintConfig {
        printer,
        material_resolved: MaterialResolved {
            identity: material.identity().clone(),
            layer_height_mm,
            normal_exposure_sec,
            bottom_exposure_sec,
            bottom_layer_count,
            wait_before_cure_sec,
            wait_after_cure_sec,
            wait_after_lift_sec,
            bottom_wait_before_cure_sec: picked
                .bottom_wait_before_cure_sec
                .unwrap_or(wait_before_cure_sec),
            bottom_wait_after_cure_sec: picked
                .bottom_wait_after_cure_sec
                .unwrap_or(wait_after_cure_sec),
            bottom_wait_after_lift_sec: picked
                .bottom_wait_after_lift_sec
                .unwrap_or(wait_after_lift_sec),
            lift_distance_mm,
            lift_speed_mm_min,
            retract_speed_mm_min,
            bottom_lift_distance_mm: picked.bottom_lift_distance_mm.unwrap_or(lift_distance_mm),
            bottom_lift_speed_mm_min: picked.bottom_lift_speed_mm_min.unwrap_or(lift_speed_mm_min),
            bottom_retract_speed_mm_min: picked
                .bottom_retract_speed_mm_min
                .unwrap_or(retract_speed_mm_min),
            light_pwm: picked.light_pwm.unwrap_or(LightPwm::FULL),
            bottom_light_pwm: picked.bottom_light_pwm.unwrap_or(LightPwm::FULL),
            scale_compensation_pct,
        },
        overrides_applied,
        warnings,
    })
}

/// Compose a printer profile, an effective material, and a set of
/// per-print scene overrides into a fully-resolved print
/// configuration.
///
/// See the module docstring for the pipeline steps. Errors with
/// [`ResolveError::RejectedBelowMinimum`] when a constrained value
/// falls below the printer's safety minimum, or with
/// [`ResolveError::MissingRequiredField`] when no resolution layer
/// supplied a required value.
pub fn resolve(
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    overrides: &SceneOverrides,
) -> Result<ResolvedPrintConfig, ResolveError> {
    let (mut picked, mut warnings, mut records) = pick_and_derive_material(printer, material);
    apply_constraints(&mut picked, &printer.constraints, &mut warnings)?;
    apply_scene_overrides(
        overrides,
        &mut picked,
        &printer.constraints,
        &mut warnings,
        &mut records,
    )?;
    assemble(printer.clone(), material, picked, records, warnings)
}

/// Resolve only the layer height, the sole material parameter geometry
/// slicing needs. Runs the same intent derivation and constraint
/// snapping as [`resolve`], but does not require the print parameters
/// (exposure, lift, speeds) to resolve. This lets a scene be sliced
/// into geometry with any printer + material pairing, leaving the full
/// print configuration for the SLA output step.
///
/// A present `requested` value is a per-print choice that replaces
/// the profile-supplied height and runs the same constraint policy
/// chain, mirroring the layer-height arm of [`resolve`]'s scene
/// overrides; with a request present, the profiles do not need to
/// supply a layer height at all.
///
/// Errors with [`ResolveError::RejectedBelowMinimum`] when the layer
/// height violates a printer constraint, or with
/// [`ResolveError::MissingRequiredField`] when `requested` is absent
/// and neither the material's intent nor a default supplies a layer
/// height.
pub fn resolve_layer_height(
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    requested: Option<PositiveFiniteMm>,
) -> Result<PositiveFiniteMm, ResolveError> {
    let (mut picked, mut warnings, _records) = pick_and_derive_material(printer, material);
    apply_constraints(&mut picked, &printer.constraints, &mut warnings)?;
    if let Some(value) = requested {
        let snapped = apply_field_policies(
            ResolvedField::LayerHeightMm,
            value.value(),
            &printer.constraints,
            &mut warnings,
        )?;
        return Ok(
            PositiveFiniteMm::new(snapped).expect("snapped layer height stays positive-finite")
        );
    }
    picked
        .layer_height_mm
        .ok_or(ResolveError::MissingRequiredField {
            field: ResolvedField::LayerHeightMm,
        })
}

/// Quantise `value` onto the printer's z-step grid, when the printer
/// declares one. Total: without a declared z step the value passes
/// through unchanged, and a value below half a step snaps up to one
/// step, the same policy the layer-height resolution applies.
///
/// This is for per-print heights that must land on the motion grid
/// WITHOUT running the full layer-height constraint chain. The
/// first-layer height is the deliberate case: the declared
/// layer-height range describes normal-layer curing and does not
/// bind a first layer, which legitimately runs thicker under its
/// longer exposure, while the z-step grid is a property of the
/// motion system and binds every layer boundary.
pub fn snap_to_z_step(
    constraints: &PrinterConstraints,
    value: PositiveFiniteMm,
) -> PositiveFiniteMm {
    let Some(step) = constraints.z_step_mm else {
        return value;
    };
    let policy = ConstraintPolicy::SnapToMultiple { step: step.value() };
    match policy.apply(value.value()) {
        ApplyResult::Snapped { snapped, .. } => {
            PositiveFiniteMm::new(snapped).expect("snapped height stays positive-finite")
        }
        // SnapToMultiple never rejects; both remaining arms keep the
        // input, which is the policy's totality contract.
        ApplyResult::Unchanged | ApplyResult::RejectedBelow { .. } => value,
    }
}
