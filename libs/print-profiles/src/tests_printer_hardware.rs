/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::printer_hardware::{EnvelopeAxisMismatch, PrinterHardware};
use sc_print_primitives::{
    BuildVolume, Mirror, PanelRotation, PixelAxis, PixelResolution, PixelSizeMicrons,
};

fn consistent_hardware() -> PrinterHardware {
    PrinterHardware::new(
        BuildVolume::new(100.0, 50.0, 100.0).unwrap(),
        PixelResolution::new(1000, 500).unwrap(),
        PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        Mirror::default(),
    )
    .unwrap()
}

const VALID_TOML: &str = r#"
build_volume = { x_mm = 100.0, y_mm = 50.0, z_mm = 100.0 }
pixel_resolution = { x = 1000, y = 500 }
pixel_size_microns = { x = 100.0, y = 100.0 }
mirror = { x = false, y = false }
"#;

#[test]
fn new_accepts_components_consistent_with_pixel_grid() {
    let hw = consistent_hardware();
    assert_eq!(hw.build_volume().x_mm(), 100.0);
    assert_eq!(hw.build_volume().y_mm(), 50.0);
    assert_eq!(hw.build_volume().z_mm(), 100.0);
    assert_eq!(hw.pixel_resolution().x(), 1000);
    assert_eq!(hw.pixel_resolution().y(), 500);
    assert_eq!(hw.pixel_size_microns().x.value(), 100.0);
    assert_eq!(hw.pixel_size_microns().y.value(), 100.0);
    assert_eq!(hw.mirror(), Mirror::default());
    assert_eq!(hw.panel_rotation(), PanelRotation::None);
}

#[test]
fn new_rejects_x_axis_mismatch() {
    let err = PrinterHardware::new(
        BuildVolume::new(110.0, 50.0, 100.0).unwrap(),
        PixelResolution::new(1000, 500).unwrap(),
        PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        Mirror::default(),
    )
    .unwrap_err();
    assert_eq!(err.axis, PixelAxis::X);
    assert_eq!(err.build_volume_mm, 110.0);
    assert_eq!(err.derived_mm, 100.0);
    assert!(
        (err.relative_diff - 10.0 / 110.0).abs() < 1e-12,
        "relative_diff = {}",
        err.relative_diff
    );
}

#[test]
fn new_rejects_y_axis_mismatch() {
    let err = PrinterHardware::new(
        BuildVolume::new(100.0, 60.0, 100.0).unwrap(),
        PixelResolution::new(1000, 500).unwrap(),
        PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        Mirror::default(),
    )
    .unwrap_err();
    assert_eq!(err.axis, PixelAxis::Y);
    assert_eq!(err.build_volume_mm, 60.0);
    assert_eq!(err.derived_mm, 50.0);
}

#[test]
fn new_short_circuits_on_x_when_both_axes_mismatch() {
    let err = PrinterHardware::new(
        BuildVolume::new(200.0, 70.0, 100.0).unwrap(),
        PixelResolution::new(1000, 500).unwrap(),
        PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        Mirror::default(),
    )
    .unwrap_err();
    assert_eq!(err.axis, PixelAxis::X);
}

#[test]
fn new_accepts_within_tolerance_just_under_05_percent() {
    let hw = PrinterHardware::new(
        BuildVolume::new(100.0, 50.0, 100.0).unwrap(),
        PixelResolution::new(1004, 500).unwrap(),
        PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        Mirror::default(),
    );
    assert!(
        hw.is_ok(),
        "expected within-tolerance to be accepted: {:?}",
        hw.err()
    );
}

#[test]
fn new_rejects_just_over_05_percent_tolerance() {
    let hw = PrinterHardware::new(
        BuildVolume::new(100.0, 50.0, 100.0).unwrap(),
        PixelResolution::new(1006, 500).unwrap(),
        PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        Mirror::default(),
    );
    assert!(
        hw.is_err(),
        "expected over-tolerance to be rejected: {:?}",
        hw.ok()
    );
}

#[test]
fn envelope_axis_mismatch_display_includes_axis_and_values() {
    let err = PrinterHardware::new(
        BuildVolume::new(110.0, 50.0, 100.0).unwrap(),
        PixelResolution::new(1000, 500).unwrap(),
        PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        Mirror::default(),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("x"), "msg: {msg}");
    assert!(msg.contains("110"), "msg: {msg}");
    assert!(msg.contains("100"), "msg: {msg}");
    assert!(msg.contains("tolerance"), "msg: {msg}");
}

#[test]
fn deserialise_parses_valid_toml() {
    let hw: PrinterHardware = toml_edit::de::from_str(VALID_TOML).unwrap();
    assert_eq!(hw.build_volume().x_mm(), 100.0);
    assert_eq!(hw.pixel_resolution().x(), 1000);
    assert_eq!(hw.pixel_size_microns().x.value(), 100.0);
    assert!(!hw.mirror().x);
}

#[test]
fn deserialise_rejects_mismatched_x_axis() {
    let toml = r#"
        build_volume = { x_mm = 200.0, y_mm = 50.0, z_mm = 100.0 }
        pixel_resolution = { x = 1000, y = 500 }
        pixel_size_microns = { x = 100.0, y = 100.0 }
        mirror = { x = false, y = false }
    "#;
    assert!(toml_edit::de::from_str::<PrinterHardware>(toml).is_err());
}

#[test]
fn deserialise_rejects_invalid_inner_primitive() {
    let toml = r#"
        build_volume = { x_mm = 0.0, y_mm = 50.0, z_mm = 100.0 }
        pixel_resolution = { x = 1000, y = 500 }
        pixel_size_microns = { x = 100.0, y = 100.0 }
        mirror = { x = false, y = false }
    "#;
    assert!(toml_edit::de::from_str::<PrinterHardware>(toml).is_err());
}

#[test]
fn deserialise_rejects_missing_mirror() {
    let toml = r#"
        build_volume = { x_mm = 100.0, y_mm = 50.0, z_mm = 100.0 }
        pixel_resolution = { x = 1000, y = 500 }
        pixel_size_microns = { x = 100.0, y = 100.0 }
    "#;
    assert!(toml_edit::de::from_str::<PrinterHardware>(toml).is_err());
}

#[test]
fn roundtrip_through_toml_preserves_value() {
    let original = consistent_hardware();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterHardware = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn deserialise_reads_a_quarter_turned_panel() {
    let toml = format!(
        "panel_rotation = \"ccw90\"
{VALID_TOML}"
    );
    let hw: PrinterHardware = toml_edit::de::from_str(&toml).unwrap();
    assert_eq!(hw.panel_rotation(), PanelRotation::Ccw90);
    // The plate keeps its own axes; only the stored image turns.
    assert_eq!(hw.build_volume().x_mm(), 100.0);
    assert_eq!(hw.pixel_resolution().x(), 1000);
}

#[test]
fn serialise_leaves_out_an_unturned_panel() {
    let serialised = toml_edit::ser::to_string(&consistent_hardware()).unwrap();
    assert!(
        !serialised.contains("panel_rotation"),
        "serialised: {serialised}"
    );
}

#[test]
fn roundtrip_through_toml_preserves_a_quarter_turn() {
    let original = consistent_hardware().with_panel_rotation(PanelRotation::Ccw90);
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterHardware = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn envelope_axis_mismatch_implements_error() {
    fn assert_is_error<E: std::error::Error>(_: &E) {}
    let err = EnvelopeAxisMismatch {
        axis: PixelAxis::X,
        build_volume_mm: 100.0,
        derived_mm: 110.0,
        relative_diff: 0.1,
    };
    assert_is_error(&err);
}
