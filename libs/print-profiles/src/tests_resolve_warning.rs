/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::resolve_warning::{IntentSource, ResolveWarning, SnapReason};
use super::resolved_field::ResolvedField;

#[test]
fn intent_source_as_str_matches_toml_field_name() {
    assert_eq!(
        IntentSource::TargetEnergyDensity.as_str(),
        "target_energy_density_mj_per_cm2",
    );
    assert_eq!(
        IntentSource::TargetLayerHeight.as_str(),
        "target_layer_height_mm",
    );
}

#[test]
fn intent_source_display_matches_as_str() {
    assert_eq!(
        format!("{}", IntentSource::TargetEnergyDensity),
        "target_energy_density_mj_per_cm2",
    );
}

#[test]
fn snap_reason_display_renders_quantised_step() {
    assert_eq!(
        format!("{}", SnapReason::QuantisedToMultiple { step: 0.025 }),
        "quantised to multiple of 0.025",
    );
}

#[test]
fn snap_reason_display_renders_range() {
    assert_eq!(
        format!(
            "{}",
            SnapReason::ClampedIntoRange {
                min: 0.01,
                max: 0.20,
            },
        ),
        "clamped into [0.01, 0.2]",
    );
}

#[test]
fn snap_reason_display_renders_max() {
    assert_eq!(
        format!("{}", SnapReason::ClampedAtMaximum { max: 5.0 }),
        "clamped at maximum 5",
    );
}

#[test]
fn autosnap_display_includes_field_values_and_reason() {
    let warning = ResolveWarning::Autosnap {
        field: ResolvedField::LayerHeightMm,
        requested: 0.063,
        snapped: 0.05,
        reason: SnapReason::QuantisedToMultiple { step: 0.025 },
    };
    assert_eq!(
        format!("{warning}"),
        "field 'layer_height_mm' snapped from 0.063 to 0.05 (quantised to multiple of 0.025)",
    );
}

#[test]
fn derived_from_intent_display_names_field_and_intent_input() {
    let warning = ResolveWarning::DerivedFromIntent {
        field: ResolvedField::NormalExposureSec,
        source: IntentSource::TargetEnergyDensity,
    };
    assert_eq!(
        format!("{warning}"),
        "field 'normal_exposure_sec' derived from intent target_energy_density_mj_per_cm2",
    );
}

#[test]
fn default_used_display_names_field() {
    let warning = ResolveWarning::DefaultUsed {
        field: ResolvedField::BottomLayerCount,
    };
    assert_eq!(
        format!("{warning}"),
        "field 'bottom_layer_count' fell back to material default",
    );
}

#[test]
fn warning_equality_is_structural() {
    let a = ResolveWarning::DefaultUsed {
        field: ResolvedField::BottomLayerCount,
    };
    let b = ResolveWarning::DefaultUsed {
        field: ResolvedField::BottomLayerCount,
    };
    let c = ResolveWarning::DefaultUsed {
        field: ResolvedField::ScaleCompensationPct,
    };
    assert_eq!(a, b);
    assert_ne!(a, c);
}
