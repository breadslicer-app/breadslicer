/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::override_record::OverrideRecord;
use super::resolved_field::ResolvedField;
use super::value_source::ValueSource;

#[test]
fn record_with_prior_value_carries_full_history() {
    let record = OverrideRecord {
        field: ResolvedField::LayerHeightMm,
        source: ValueSource::Override,
        prior_value: Some(0.06),
        final_value: 0.05,
    };
    assert_eq!(record.field, ResolvedField::LayerHeightMm);
    assert_eq!(record.source, ValueSource::Override);
    assert_eq!(record.prior_value, Some(0.06));
    assert_eq!(record.final_value, 0.05);
}

#[test]
fn record_without_prior_value_marks_first_source() {
    let record = OverrideRecord {
        field: ResolvedField::NormalExposureSec,
        source: ValueSource::Calibrated,
        prior_value: None,
        final_value: 2.5,
    };
    assert_eq!(record.prior_value, None);
    assert_eq!(record.final_value, 2.5);
}

#[test]
fn display_includes_prior_value_when_present() {
    let record = OverrideRecord {
        field: ResolvedField::LayerHeightMm,
        source: ValueSource::Override,
        prior_value: Some(0.06),
        final_value: 0.05,
    };
    assert_eq!(
        format!("{record}"),
        "layer_height_mm: 0.05 (override, was 0.06)",
    );
}

#[test]
fn display_omits_prior_value_when_absent() {
    let record = OverrideRecord {
        field: ResolvedField::NormalExposureSec,
        source: ValueSource::Calibrated,
        prior_value: None,
        final_value: 2.5,
    };
    assert_eq!(format!("{record}"), "normal_exposure_sec: 2.5 (calibrated)");
}

#[test]
fn equality_is_structural() {
    let a = OverrideRecord {
        field: ResolvedField::LiftSpeedMmMin,
        source: ValueSource::Default,
        prior_value: None,
        final_value: 120.0,
    };
    let b = OverrideRecord {
        field: ResolvedField::LiftSpeedMmMin,
        source: ValueSource::Default,
        prior_value: None,
        final_value: 120.0,
    };
    let c = OverrideRecord {
        field: ResolvedField::LiftSpeedMmMin,
        source: ValueSource::Default,
        prior_value: None,
        final_value: 100.0,
    };
    assert_eq!(a, b);
    assert_ne!(a, c);
}
