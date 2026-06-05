/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::primitives::CoordAxis;

use super::build_volume::BuildVolume;

#[test]
fn build_volume_new_accepts_positive_finite() {
    let bv = BuildVolume::new(60.0, 60.0, 100.0).unwrap();
    assert_eq!(bv.x_mm(), 60.0);
    assert_eq!(bv.y_mm(), 60.0);
    assert_eq!(bv.z_mm(), 100.0);
}

#[test]
fn build_volume_new_rejects_zero_x_axis() {
    let err = BuildVolume::new(0.0, 60.0, 100.0).unwrap_err();
    assert_eq!(err.axis, CoordAxis::X);
    assert_eq!(err.source.value, 0.0);
}

#[test]
fn build_volume_new_rejects_negative_y_axis() {
    let err = BuildVolume::new(60.0, -1.0, 100.0).unwrap_err();
    assert_eq!(err.axis, CoordAxis::Y);
    assert_eq!(err.source.value, -1.0);
}

#[test]
fn build_volume_new_rejects_nan_z_axis() {
    let err = BuildVolume::new(60.0, 60.0, f64::NAN).unwrap_err();
    assert_eq!(err.axis, CoordAxis::Z);
    assert!(err.source.value.is_nan());
}

#[test]
fn build_volume_new_rejects_infinite_x_axis() {
    let err = BuildVolume::new(f64::INFINITY, 60.0, 100.0).unwrap_err();
    assert_eq!(err.axis, CoordAxis::X);
    assert!(err.source.value.is_infinite());
}

#[test]
fn build_volume_envelope_centres_xy_on_origin_and_floors_z_at_zero() {
    let bv = BuildVolume::new(192.0, 120.0, 200.0).unwrap();
    assert_eq!(bv.envelope_min_mm(), [-96.0, -60.0, 0.0]);
    assert_eq!(bv.envelope_max_mm(), [96.0, 60.0, 200.0]);
}

#[test]
fn build_volume_envelope_extent_matches_per_axis_dimensions() {
    let bv = BuildVolume::new(60.0, 40.0, 100.0).unwrap();
    let min = bv.envelope_min_mm();
    let max = bv.envelope_max_mm();
    assert!((max[0] - min[0] - bv.x_mm()).abs() < 1e-12);
    assert!((max[1] - min[1] - bv.y_mm()).abs() < 1e-12);
    assert!((max[2] - min[2] - bv.z_mm()).abs() < 1e-12);
}

#[test]
fn axis_out_of_range_display_includes_axis_and_value() {
    let err = BuildVolume::new(0.0, 60.0, 100.0).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("X axis"), "msg: {msg}");
    assert!(msg.contains("got 0"), "msg: {msg}");
}
