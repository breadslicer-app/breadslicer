/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, ScaleCompensation,
    ScaleCompensationPercent,
};

use super::scene_overrides::SceneOverrides;

#[test]
fn default_value_has_every_field_unset() {
    let overrides = SceneOverrides::default();
    assert!(overrides.layer_height_mm.is_none());
    assert!(overrides.normal_exposure_sec.is_none());
    assert!(overrides.bottom_exposure_sec.is_none());
    assert!(overrides.bottom_layer_count.is_none());
    assert!(overrides.lift_distance_mm.is_none());
    assert!(overrides.lift_speed_mm_min.is_none());
    assert!(overrides.retract_speed_mm_min.is_none());
    assert!(overrides.scale_compensation_pct.is_none());
}

#[test]
fn struct_update_syntax_allows_partial_overrides() {
    let overrides = SceneOverrides {
        layer_height_mm: Some(PositiveFiniteMm::new(0.05).unwrap()),
        ..Default::default()
    };
    assert_eq!(overrides.layer_height_mm.map(|v| v.value()), Some(0.05));
    assert!(overrides.normal_exposure_sec.is_none());
    assert!(overrides.bottom_layer_count.is_none());
}

#[test]
fn every_field_is_independently_settable() {
    let overrides = SceneOverrides {
        layer_height_mm: Some(PositiveFiniteMm::new(0.04).unwrap()),
        normal_exposure_sec: Some(PositiveFiniteSec::new(2.5).unwrap()),
        bottom_exposure_sec: Some(PositiveFiniteSec::new(30.0).unwrap()),
        bottom_layer_count: Some(8),
        lift_distance_mm: Some(PositiveFiniteMm::new(5.0).unwrap()),
        lift_speed_mm_min: Some(PositiveFiniteMmPerMin::new(120.0).unwrap()),
        retract_speed_mm_min: Some(PositiveFiniteMmPerMin::new(150.0).unwrap()),
        scale_compensation_pct: Some(ScaleCompensation {
            x: ScaleCompensationPercent::new(0.5).unwrap(),
            y: ScaleCompensationPercent::new(0.5).unwrap(),
            z: ScaleCompensationPercent::new(0.0).unwrap(),
        }),
    };
    assert_eq!(overrides.layer_height_mm.unwrap().value(), 0.04);
    assert_eq!(overrides.normal_exposure_sec.unwrap().value(), 2.5);
    assert_eq!(overrides.bottom_exposure_sec.unwrap().value(), 30.0);
    assert_eq!(overrides.bottom_layer_count, Some(8));
    assert_eq!(overrides.lift_distance_mm.unwrap().value(), 5.0);
    assert_eq!(overrides.lift_speed_mm_min.unwrap().value(), 120.0);
    assert_eq!(overrides.retract_speed_mm_min.unwrap().value(), 150.0);
    let scale = overrides.scale_compensation_pct.unwrap();
    assert_eq!(scale.z.value(), 0.0);
}
