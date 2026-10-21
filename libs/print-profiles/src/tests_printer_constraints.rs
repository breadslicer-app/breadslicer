/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::printer_constraints::PrinterConstraints;
use sc_print_primitives::{
    LengthRange, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteMwPerCm2,
    SpeedRangeMmPerMin,
};

const FULL_TOML: &str = r#"
z_step_mm = 0.01
layer_height_range_mm = { min = 0.01, max = 0.15 }
lift_speed_range_mm_min = { min = 30.0, max = 300.0 }
retract_speed_range_mm_min = { min = 30.0, max = 300.0 }
lift_distance_max_mm = 12.0
light_intensity_mw_per_cm2 = 4.2
"#;

#[test]
fn deserialise_parses_all_fields() {
    let parsed: PrinterConstraints = toml_edit::de::from_str(FULL_TOML).unwrap();
    assert_eq!(parsed.z_step_mm.map(|v| v.value()), Some(0.01));
    let lh = parsed.layer_height_range_mm.unwrap();
    assert_eq!(lh.min().value(), 0.01);
    assert_eq!(lh.max().value(), 0.15);
    let ls = parsed.lift_speed_range_mm_min.unwrap();
    assert_eq!(ls.min().value(), 30.0);
    assert_eq!(ls.max().value(), 300.0);
    let rs = parsed.retract_speed_range_mm_min.unwrap();
    assert_eq!(rs.min().value(), 30.0);
    assert_eq!(rs.max().value(), 300.0);
    assert_eq!(parsed.lift_distance_max_mm.map(|v| v.value()), Some(12.0));
    assert_eq!(
        parsed.light_intensity_mw_per_cm2.map(|v| v.value()),
        Some(4.2)
    );
}

#[test]
fn deserialise_empty_section_leaves_all_fields_none() {
    let toml = "";
    let parsed: PrinterConstraints = toml_edit::de::from_str(toml).unwrap();
    assert!(parsed.z_step_mm.is_none());
    assert!(parsed.layer_height_range_mm.is_none());
    assert!(parsed.lift_speed_range_mm_min.is_none());
    assert!(parsed.retract_speed_range_mm_min.is_none());
    assert!(parsed.lift_distance_max_mm.is_none());
    assert!(parsed.light_intensity_mw_per_cm2.is_none());
}

#[test]
fn deserialise_partial_section_keeps_unset_fields_none() {
    let toml = r#"
z_step_mm = 0.01
lift_distance_max_mm = 8.0
"#;
    let parsed: PrinterConstraints = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.z_step_mm.map(|v| v.value()), Some(0.01));
    assert_eq!(parsed.lift_distance_max_mm.map(|v| v.value()), Some(8.0));
    assert!(parsed.layer_height_range_mm.is_none());
    assert!(parsed.lift_speed_range_mm_min.is_none());
    assert!(parsed.retract_speed_range_mm_min.is_none());
    assert!(parsed.light_intensity_mw_per_cm2.is_none());
}

#[test]
fn deserialise_rejects_non_positive_z_step_mm() {
    let toml = "z_step_mm = -0.01\n";
    assert!(toml_edit::de::from_str::<PrinterConstraints>(toml).is_err());
}

#[test]
fn deserialise_rejects_zero_lift_distance_max_mm() {
    let toml = "lift_distance_max_mm = 0.0\n";
    assert!(toml_edit::de::from_str::<PrinterConstraints>(toml).is_err());
}

#[test]
fn deserialise_rejects_inverted_layer_height_range_mm() {
    let toml = "layer_height_range_mm = { min = 0.15, max = 0.01 }\n";
    assert!(toml_edit::de::from_str::<PrinterConstraints>(toml).is_err());
}

#[test]
fn deserialise_rejects_equal_bounds_lift_speed_range() {
    let toml = "lift_speed_range_mm_min = { min = 60.0, max = 60.0 }\n";
    assert!(toml_edit::de::from_str::<PrinterConstraints>(toml).is_err());
}

#[test]
fn deserialise_rejects_non_positive_retract_speed_min() {
    let toml = "retract_speed_range_mm_min = { min = 0.0, max = 100.0 }\n";
    assert!(toml_edit::de::from_str::<PrinterConstraints>(toml).is_err());
}

#[test]
fn deserialise_rejects_non_finite_light_intensity() {
    let toml = "light_intensity_mw_per_cm2 = nan\n";
    assert!(toml_edit::de::from_str::<PrinterConstraints>(toml).is_err());
}

#[test]
fn roundtrip_with_all_fields_preserves_value() {
    let original: PrinterConstraints = toml_edit::de::from_str(FULL_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterConstraints = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn roundtrip_with_empty_section_preserves_all_none() {
    let original: PrinterConstraints = toml_edit::de::from_str("").unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterConstraints = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
    assert!(roundtripped.z_step_mm.is_none());
    assert!(roundtripped.layer_height_range_mm.is_none());
}

#[test]
fn serialise_omits_every_unset_field() {
    let parsed: PrinterConstraints = toml_edit::de::from_str("").unwrap();
    let serialised = toml_edit::ser::to_string(&parsed).unwrap();
    for field in [
        "z_step_mm",
        "layer_height_range_mm",
        "lift_speed_range_mm_min",
        "retract_speed_range_mm_min",
        "lift_distance_max_mm",
        "light_intensity_mw_per_cm2",
    ] {
        assert!(
            !serialised.contains(field),
            "should omit {field}: {serialised}"
        );
    }
}

#[test]
fn programmatic_construction_round_trips() {
    let constraints = PrinterConstraints {
        z_step_mm: Some(PositiveFiniteMm::new(0.01).unwrap()),
        layer_height_range_mm: Some(
            LengthRange::new(
                PositiveFiniteMm::new(0.02).unwrap(),
                PositiveFiniteMm::new(0.10).unwrap(),
            )
            .unwrap(),
        ),
        lift_speed_range_mm_min: Some(
            SpeedRangeMmPerMin::new(
                PositiveFiniteMmPerMin::new(60.0).unwrap(),
                PositiveFiniteMmPerMin::new(180.0).unwrap(),
            )
            .unwrap(),
        ),
        retract_speed_range_mm_min: None,
        lift_distance_max_mm: Some(PositiveFiniteMm::new(8.0).unwrap()),
        light_intensity_mw_per_cm2: Some(PositiveFiniteMwPerCm2::new(4.2).unwrap()),
    };
    let serialised = toml_edit::ser::to_string(&constraints).unwrap();
    let roundtripped: PrinterConstraints = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(constraints, roundtripped);
}
