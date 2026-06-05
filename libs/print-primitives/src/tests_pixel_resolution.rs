/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::primitives::CoordAxis;

use super::pixel_resolution::{PixelResolution, ZeroPixelExtentError};

#[test]
fn pixel_resolution_new_accepts_positive_dimensions() {
    let res = PixelResolution::new(1920, 1080).unwrap();
    assert_eq!(res.x(), 1920);
    assert_eq!(res.y(), 1080);
}

#[test]
fn pixel_resolution_new_rejects_zero_x() {
    let err = PixelResolution::new(0, 1080).unwrap_err();
    assert!(matches!(err, ZeroPixelExtentError { axis: CoordAxis::X }));
}

#[test]
fn pixel_resolution_new_rejects_zero_y() {
    let err = PixelResolution::new(1920, 0).unwrap_err();
    assert!(matches!(err, ZeroPixelExtentError { axis: CoordAxis::Y }));
}

#[test]
fn zero_pixel_extent_error_display_includes_axis() {
    let msg = format!("{}", ZeroPixelExtentError { axis: CoordAxis::X });
    assert!(msg.contains("X") || msg.contains("x"), "msg: {msg}");
    assert!(msg.contains("non-zero"), "msg: {msg}");
}
