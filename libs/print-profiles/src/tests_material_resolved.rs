/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    MaterialId, NonEmptyString, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
    ScaleCompensation, ScaleCompensationPercent,
};
use semver::Version;

use super::material_identity::MaterialIdentity;
use super::material_resolved::MaterialResolved;

fn sample_identity() -> MaterialIdentity {
    MaterialIdentity {
        id: MaterialId::new("elegoo-resin-grey").unwrap(),
        brand: NonEmptyString::new("Elegoo").unwrap(),
        family: NonEmptyString::new("Standard Resin").unwrap(),
        name: NonEmptyString::new("Standard Grey").unwrap(),
        profile_version: Version::new(1, 0, 0),
    }
}

fn sample_scale_compensation() -> ScaleCompensation {
    ScaleCompensation {
        x: ScaleCompensationPercent::new(0.5).unwrap(),
        y: ScaleCompensationPercent::new(0.5).unwrap(),
        z: ScaleCompensationPercent::new(0.0).unwrap(),
    }
}

fn sample_resolved() -> MaterialResolved {
    MaterialResolved {
        identity: sample_identity(),
        layer_height_mm: PositiveFiniteMm::new(0.05).unwrap(),
        normal_exposure_sec: PositiveFiniteSec::new(2.5).unwrap(),
        bottom_exposure_sec: PositiveFiniteSec::new(30.0).unwrap(),
        bottom_layer_count: 6,
        lift_distance_mm: PositiveFiniteMm::new(5.0).unwrap(),
        lift_speed_mm_min: PositiveFiniteMmPerMin::new(120.0).unwrap(),
        retract_speed_mm_min: PositiveFiniteMmPerMin::new(150.0).unwrap(),
        scale_compensation_pct: sample_scale_compensation(),
    }
}

#[test]
fn constructed_value_carries_every_field() {
    let resolved = sample_resolved();
    assert_eq!(resolved.identity.id.as_str(), "elegoo-resin-grey");
    assert_eq!(resolved.layer_height_mm.value(), 0.05);
    assert_eq!(resolved.normal_exposure_sec.value(), 2.5);
    assert_eq!(resolved.bottom_exposure_sec.value(), 30.0);
    assert_eq!(resolved.bottom_layer_count, 6);
    assert_eq!(resolved.lift_distance_mm.value(), 5.0);
    assert_eq!(resolved.lift_speed_mm_min.value(), 120.0);
    assert_eq!(resolved.retract_speed_mm_min.value(), 150.0);
    assert_eq!(resolved.scale_compensation_pct.x.value(), 0.5);
}

#[test]
fn equality_is_structural() {
    assert_eq!(sample_resolved(), sample_resolved());

    let mut altered = sample_resolved();
    altered.bottom_layer_count = 7;
    assert_ne!(sample_resolved(), altered);
}
