/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::length::PositiveFiniteMicrons;
use super::pixel_size::{PixelAxis, PixelSizeMicrons};

#[test]
fn pixel_size_microns_new_accepts_positive_finite_axes() {
    let size = PixelSizeMicrons::new(18.0, 18.0).unwrap();
    assert_eq!(size.x.value(), 18.0);
    assert_eq!(size.y.value(), 18.0);
}

#[test]
fn pixel_size_microns_new_allows_non_square_pixels() {
    let size = PixelSizeMicrons::new(34.0, 51.0).unwrap();
    assert_eq!(size.x.value(), 34.0);
    assert_eq!(size.y.value(), 51.0);
}

#[test]
fn pixel_size_microns_new_rejects_zero_x() {
    let err = PixelSizeMicrons::new(0.0, 18.0).unwrap_err();
    assert_eq!(err.axis, PixelAxis::X);
    assert_eq!(err.source.value, 0.0);
}

#[test]
fn pixel_size_microns_new_rejects_negative_x() {
    let err = PixelSizeMicrons::new(-1.0, 18.0).unwrap_err();
    assert_eq!(err.axis, PixelAxis::X);
    assert_eq!(err.source.value, -1.0);
}

#[test]
fn pixel_size_microns_new_rejects_nan_x() {
    let err = PixelSizeMicrons::new(f64::NAN, 18.0).unwrap_err();
    assert_eq!(err.axis, PixelAxis::X);
    assert!(err.source.value.is_nan());
}

#[test]
fn pixel_size_microns_new_rejects_infinite_x() {
    let err = PixelSizeMicrons::new(f64::INFINITY, 18.0).unwrap_err();
    assert_eq!(err.axis, PixelAxis::X);
}

#[test]
fn pixel_size_microns_new_reports_x_when_both_invalid() {
    let err = PixelSizeMicrons::new(-1.0, -2.0).unwrap_err();
    assert_eq!(err.axis, PixelAxis::X);
    assert_eq!(err.source.value, -1.0);
}

#[test]
fn pixel_size_microns_new_rejects_zero_y() {
    let err = PixelSizeMicrons::new(18.0, 0.0).unwrap_err();
    assert_eq!(err.axis, PixelAxis::Y);
    assert_eq!(err.source.value, 0.0);
}

#[test]
fn pixel_size_microns_new_rejects_negative_y() {
    let err = PixelSizeMicrons::new(18.0, -1.0).unwrap_err();
    assert_eq!(err.axis, PixelAxis::Y);
    assert_eq!(err.source.value, -1.0);
}

#[test]
fn pixel_size_microns_new_rejects_nan_y() {
    let err = PixelSizeMicrons::new(18.0, f64::NAN).unwrap_err();
    assert_eq!(err.axis, PixelAxis::Y);
    assert!(err.source.value.is_nan());
}

#[test]
fn pixel_size_microns_field_access_works_after_new() {
    let size = PixelSizeMicrons::new(34.0, 51.0).unwrap();
    let x: PositiveFiniteMicrons = size.x;
    let y: PositiveFiniteMicrons = size.y;
    assert_eq!(x.value(), 34.0);
    assert_eq!(y.value(), 51.0);
}

#[test]
fn pixel_axis_display_renders_x_lowercase() {
    assert_eq!(format!("{}", PixelAxis::X), "x");
}

#[test]
fn pixel_axis_display_renders_y_lowercase() {
    assert_eq!(format!("{}", PixelAxis::Y), "y");
}

#[test]
fn pixel_axis_out_of_range_display_includes_axis_and_value() {
    let err = PixelSizeMicrons::new(-3.5, 18.0).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("x"), "msg: {msg}");
    assert!(msg.contains("-3.5"), "msg: {msg}");
    assert!(msg.contains("microns"), "msg: {msg}");
}

#[test]
fn pixel_axis_out_of_range_source_chains_to_length_out_of_range() {
    use std::error::Error;

    let err = PixelSizeMicrons::new(-3.5, 18.0).unwrap_err();
    let source = err.source().expect("expected a chained source");
    let chained_msg = format!("{source}");
    assert!(chained_msg.contains("-3.5"), "chained msg: {chained_msg}");
    assert!(
        chained_msg.contains("positive"),
        "chained msg: {chained_msg}"
    );
}

#[cfg(feature = "serde")]
#[test]
fn pixel_size_microns_deserialises_from_toml() {
    let toml = "x = 18.0\ny = 18.0\n";
    let size: PixelSizeMicrons = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(size.x.value(), 18.0);
    assert_eq!(size.y.value(), 18.0);
}

#[cfg(feature = "serde")]
#[test]
fn pixel_size_microns_deserialise_rejects_zero_x() {
    let toml = "x = 0.0\ny = 18.0\n";
    assert!(toml_edit::de::from_str::<PixelSizeMicrons>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn pixel_size_microns_deserialise_rejects_negative_y() {
    let toml = "x = 18.0\ny = -1.0\n";
    assert!(toml_edit::de::from_str::<PixelSizeMicrons>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn pixel_size_microns_roundtrip_through_toml_preserves_value() {
    let original = PixelSizeMicrons::new(34.0, 51.0).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PixelSizeMicrons = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}
