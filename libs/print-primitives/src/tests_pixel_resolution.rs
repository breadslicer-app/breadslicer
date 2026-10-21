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

#[cfg(feature = "serde")]
#[test]
fn pixel_resolution_deserialises_from_toml() {
    let toml = "x = 1920\ny = 1080\n";
    let res: PixelResolution = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(res.x(), 1920);
    assert_eq!(res.y(), 1080);
}

#[cfg(feature = "serde")]
#[test]
fn pixel_resolution_deserialise_rejects_zero_x() {
    let toml = "x = 0\ny = 1080\n";
    assert!(toml_edit::de::from_str::<PixelResolution>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn pixel_resolution_deserialise_rejects_zero_y() {
    let toml = "x = 1920\ny = 0\n";
    assert!(toml_edit::de::from_str::<PixelResolution>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn pixel_resolution_roundtrip_through_toml_preserves_value() {
    let original = PixelResolution::new(1920, 1080).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PixelResolution = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}
