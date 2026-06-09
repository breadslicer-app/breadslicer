/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::resolve_error::ResolveError;
use super::resolved_field::ResolvedField;

#[test]
fn rejected_below_minimum_display_names_field_value_and_minimum() {
    let err = ResolveError::RejectedBelowMinimum {
        field: ResolvedField::LiftSpeedMmMin,
        value: 30.0,
        minimum: 60.0,
    };
    assert_eq!(
        format!("{err}"),
        "field 'lift_speed_mm_min' value 30 is below the printer minimum 60",
    );
}

#[test]
fn missing_required_field_display_names_field() {
    let err = ResolveError::MissingRequiredField {
        field: ResolvedField::LayerHeightMm,
    };
    assert_eq!(
        format!("{err}"),
        "required field 'layer_height_mm' has no value after all resolution layers",
    );
}

#[test]
fn equality_is_structural() {
    let a = ResolveError::MissingRequiredField {
        field: ResolvedField::NormalExposureSec,
    };
    let b = ResolveError::MissingRequiredField {
        field: ResolvedField::NormalExposureSec,
    };
    let c = ResolveError::MissingRequiredField {
        field: ResolvedField::BottomExposureSec,
    };
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn error_trait_source_is_none_for_leaf_variants() {
    use std::error::Error;
    let err = ResolveError::MissingRequiredField {
        field: ResolvedField::LayerHeightMm,
    };
    assert!(err.source().is_none());
}
