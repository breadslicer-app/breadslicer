/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    LengthRange, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteMwPerCm2,
    SpeedRangeMmPerMin,
};

use super::policy::{
    AUTOSNAP_WARNING_THRESHOLD, ApplyResult, ConstraintPolicy, policies_for, relative_delta_exceeds,
};
use super::printer_constraints::PrinterConstraints;
use super::resolve_warning::SnapReason;
use super::resolved_field::ResolvedField;

#[test]
fn autosnap_warning_threshold_is_five_percent() {
    assert_eq!(AUTOSNAP_WARNING_THRESHOLD, 0.05);
}

#[test]
fn snap_to_multiple_quantises_to_nearest_step() {
    let policy = ConstraintPolicy::SnapToMultiple { step: 0.025 };
    match policy.apply(0.063) {
        ApplyResult::Snapped { snapped, reason } => {
            assert!(
                (snapped - 0.075).abs() < 1e-12,
                "expected ~0.075, got {snapped}",
            );
            assert_eq!(reason, SnapReason::QuantisedToMultiple { step: 0.025 });
        }
        other => panic!("expected Snapped, got {other:?}"),
    }
}

#[test]
fn snap_to_multiple_leaves_grid_aligned_values_unchanged() {
    let policy = ConstraintPolicy::SnapToMultiple { step: 0.025 };
    assert_eq!(policy.apply(0.05), ApplyResult::Unchanged);
}

#[test]
fn snap_to_multiple_below_half_step_snaps_up_to_one_step() {
    // 0 multiples is not a printable height; the first multiple is
    // the nearest printable one.
    let policy = ConstraintPolicy::SnapToMultiple { step: 0.05 };
    match policy.apply(0.02) {
        ApplyResult::Snapped { snapped, reason } => {
            assert_eq!(snapped, 0.05);
            assert_eq!(reason, SnapReason::QuantisedToMultiple { step: 0.05 });
        }
        other => panic!("expected Snapped, got {other:?}"),
    }
}

#[test]
fn snap_to_multiple_at_exactly_half_a_step_rounds_up() {
    // f64::round ties away from zero, so half a step lands on the
    // first multiple, not on 0.
    let policy = ConstraintPolicy::SnapToMultiple { step: 0.05 };
    match policy.apply(0.025) {
        ApplyResult::Snapped { snapped, .. } => assert_eq!(snapped, 0.05),
        other => panic!("expected Snapped, got {other:?}"),
    }
}

#[test]
fn snap_to_multiple_with_a_subnormal_step_leaves_the_value_unchanged() {
    // value / step overflows to infinity; the grid is finer than f64
    // resolution there, so the value already sits on it.
    let policy = ConstraintPolicy::SnapToMultiple { step: 5e-324 };
    assert_eq!(policy.apply(0.05), ApplyResult::Unchanged);
}

#[test]
fn snap_to_multiple_never_emits_a_non_finite_value() {
    // Rounding up can overflow the product near the f64 range limit;
    // the quantiser must pass the value through instead.
    let policy = ConstraintPolicy::SnapToMultiple { step: 9e307 };
    assert_eq!(policy.apply(f64::MAX), ApplyResult::Unchanged);
}

#[test]
fn snap_into_range_clamps_below_to_min() {
    let policy = ConstraintPolicy::SnapIntoRange {
        min: 0.02,
        max: 0.10,
    };
    match policy.apply(0.01) {
        ApplyResult::Snapped { snapped, reason } => {
            assert_eq!(snapped, 0.02);
            assert_eq!(
                reason,
                SnapReason::ClampedIntoRange {
                    min: 0.02,
                    max: 0.10,
                },
            );
        }
        other => panic!("expected Snapped, got {other:?}"),
    }
}

#[test]
fn snap_into_range_clamps_above_to_max() {
    let policy = ConstraintPolicy::SnapIntoRange {
        min: 0.02,
        max: 0.10,
    };
    match policy.apply(0.20) {
        ApplyResult::Snapped { snapped, .. } => assert_eq!(snapped, 0.10),
        other => panic!("expected Snapped, got {other:?}"),
    }
}

#[test]
fn snap_into_range_leaves_in_range_values_unchanged() {
    let policy = ConstraintPolicy::SnapIntoRange {
        min: 0.02,
        max: 0.10,
    };
    assert_eq!(policy.apply(0.05), ApplyResult::Unchanged);
}

#[test]
fn snap_at_maximum_clamps_above_to_max() {
    let policy = ConstraintPolicy::SnapAtMaximum { max: 5.0 };
    match policy.apply(8.0) {
        ApplyResult::Snapped { snapped, reason } => {
            assert_eq!(snapped, 5.0);
            assert_eq!(reason, SnapReason::ClampedAtMaximum { max: 5.0 });
        }
        other => panic!("expected Snapped, got {other:?}"),
    }
}

#[test]
fn snap_at_maximum_leaves_below_unchanged() {
    let policy = ConstraintPolicy::SnapAtMaximum { max: 5.0 };
    assert_eq!(policy.apply(3.0), ApplyResult::Unchanged);
}

#[test]
fn reject_below_min_snap_at_maximum_rejects_below() {
    let policy = ConstraintPolicy::RejectBelowMinSnapAtMaximum {
        min: 60.0,
        max: 300.0,
    };
    assert_eq!(
        policy.apply(30.0),
        ApplyResult::RejectedBelow { minimum: 60.0 },
    );
}

#[test]
fn reject_below_min_snap_at_maximum_snaps_above() {
    let policy = ConstraintPolicy::RejectBelowMinSnapAtMaximum {
        min: 60.0,
        max: 300.0,
    };
    match policy.apply(400.0) {
        ApplyResult::Snapped { snapped, reason } => {
            assert_eq!(snapped, 300.0);
            assert_eq!(reason, SnapReason::ClampedAtMaximum { max: 300.0 });
        }
        other => panic!("expected Snapped, got {other:?}"),
    }
}

#[test]
fn reject_below_min_snap_at_maximum_leaves_in_range_unchanged() {
    let policy = ConstraintPolicy::RejectBelowMinSnapAtMaximum {
        min: 60.0,
        max: 300.0,
    };
    assert_eq!(policy.apply(120.0), ApplyResult::Unchanged);
}

#[test]
fn relative_delta_exceeds_compares_against_threshold() {
    assert!(relative_delta_exceeds(0.05, 0.06, 0.05));
    assert!(!relative_delta_exceeds(0.05, 0.0525, 0.05));
    assert!(relative_delta_exceeds(0.05, 0.04, 0.05));
}

fn empty_constraints() -> PrinterConstraints {
    PrinterConstraints {
        z_step_mm: None,
        layer_height_range_mm: None,
        lift_speed_range_mm_min: None,
        retract_speed_range_mm_min: None,
        lift_distance_max_mm: None,
        light_intensity_mw_per_cm2: None,
    }
}

#[test]
fn policies_for_returns_empty_when_no_constraints_declared() {
    let constraints = empty_constraints();
    for field in [
        ResolvedField::LayerHeightMm,
        ResolvedField::LiftDistanceMm,
        ResolvedField::LiftSpeedMmMin,
        ResolvedField::RetractSpeedMmMin,
        ResolvedField::NormalExposureSec,
        ResolvedField::BottomExposureSec,
        ResolvedField::BottomLayerCount,
        ResolvedField::ScaleCompensationPct,
    ] {
        assert!(
            policies_for(field, &constraints).is_empty(),
            "expected no policies for {field}",
        );
    }
}

#[test]
fn policies_for_layer_height_returns_range_then_quantise() {
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.025).unwrap()),
        layer_height_range_mm: Some(
            LengthRange::new(
                PositiveFiniteMm::new(0.02).unwrap(),
                PositiveFiniteMm::new(0.10).unwrap(),
            )
            .unwrap(),
        ),
        ..empty_constraints()
    };
    let policies = policies_for(ResolvedField::LayerHeightMm, &constraints);
    assert_eq!(policies.len(), 2);
    assert_eq!(
        policies[0],
        ConstraintPolicy::SnapIntoRange {
            min: 0.02,
            max: 0.10,
        },
    );
    assert_eq!(
        policies[1],
        ConstraintPolicy::SnapToMultiple { step: 0.025 }
    );
}

#[test]
fn policies_for_lift_speed_returns_reject_below_min_snap_at_max() {
    let constraints = PrinterConstraints {
        lift_speed_range_mm_min: Some(
            SpeedRangeMmPerMin::new(
                PositiveFiniteMmPerMin::new(60.0).unwrap(),
                PositiveFiniteMmPerMin::new(300.0).unwrap(),
            )
            .unwrap(),
        ),
        ..empty_constraints()
    };
    let policies = policies_for(ResolvedField::LiftSpeedMmMin, &constraints);
    assert_eq!(policies.len(), 1);
    assert_eq!(
        policies[0],
        ConstraintPolicy::RejectBelowMinSnapAtMaximum {
            min: 60.0,
            max: 300.0,
        },
    );
}

#[test]
fn policies_for_lift_distance_returns_snap_at_max() {
    let constraints = PrinterConstraints {
        lift_distance_max_mm: Some(PositiveFiniteMm::new(8.0).unwrap()),
        ..empty_constraints()
    };
    let policies = policies_for(ResolvedField::LiftDistanceMm, &constraints);
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0], ConstraintPolicy::SnapAtMaximum { max: 8.0 });
}

#[test]
fn policies_for_fields_without_constraint_kinds_stays_empty() {
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.025).unwrap()),
        light_intensity_mw_per_cm2: Some(PositiveFiniteMwPerCm2::new(5.0).unwrap()),
        ..empty_constraints()
    };
    for field in [
        ResolvedField::NormalExposureSec,
        ResolvedField::BottomExposureSec,
        ResolvedField::BottomLayerCount,
        ResolvedField::ScaleCompensationPct,
    ] {
        assert!(policies_for(field, &constraints).is_empty());
    }
}
