/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::material_defaults::MaterialDefaults;
use sc_print_primitives::{NonNegativeFiniteSec, ScaleCompensation, ScaleCompensationPercent};

const FULL_TOML: &str = r#"
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }
bottom_layer_count = 6
wait_before_cure_sec = 0.5
wait_after_cure_sec = 1.0
wait_after_lift_sec = 0.0
"#;

#[test]
fn deserialise_parses_every_field() {
    let parsed: MaterialDefaults = toml_edit::de::from_str(FULL_TOML).unwrap();
    let scale = parsed.scale_compensation_pct.unwrap();
    assert_eq!(scale.x.value(), 0.0);
    assert_eq!(scale.y.value(), 0.0);
    assert_eq!(scale.z.value(), 0.0);
    assert_eq!(parsed.bottom_layer_count, Some(6));
    assert_eq!(parsed.wait_before_cure_sec.unwrap().value(), 0.5);
    assert_eq!(parsed.wait_after_cure_sec.unwrap().value(), 1.0);
    assert_eq!(parsed.wait_after_lift_sec.unwrap().value(), 0.0);
}

#[test]
fn deserialise_empty_section_leaves_every_field_none() {
    let parsed: MaterialDefaults = toml_edit::de::from_str("").unwrap();
    assert!(parsed.scale_compensation_pct.is_none());
    assert!(parsed.bottom_layer_count.is_none());
    assert!(parsed.wait_before_cure_sec.is_none());
    assert!(parsed.wait_after_cure_sec.is_none());
    assert!(parsed.wait_after_lift_sec.is_none());
}

#[test]
fn deserialise_rejects_a_negative_wait() {
    let toml = "wait_after_lift_sec = -0.5\n";
    assert!(toml_edit::de::from_str::<MaterialDefaults>(toml).is_err());
}

#[test]
fn deserialise_with_only_scale_compensation_leaves_count_none() {
    let toml = "scale_compensation_pct = { x = 1.0, y = -2.0, z = 0.5 }\n";
    let parsed: MaterialDefaults = toml_edit::de::from_str(toml).unwrap();
    let scale = parsed.scale_compensation_pct.unwrap();
    assert_eq!(scale.x.value(), 1.0);
    assert_eq!(scale.y.value(), -2.0);
    assert_eq!(scale.z.value(), 0.5);
    assert!(parsed.bottom_layer_count.is_none());
}

#[test]
fn deserialise_with_only_bottom_layer_count_leaves_scale_none() {
    let toml = "bottom_layer_count = 8\n";
    let parsed: MaterialDefaults = toml_edit::de::from_str(toml).unwrap();
    assert!(parsed.scale_compensation_pct.is_none());
    assert_eq!(parsed.bottom_layer_count, Some(8));
}

#[test]
fn deserialise_accepts_zero_bottom_layer_count() {
    let toml = "bottom_layer_count = 0\n";
    let parsed: MaterialDefaults = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.bottom_layer_count, Some(0));
}

#[test]
fn deserialise_rejects_scale_compensation_axis_out_of_range() {
    let toml = "scale_compensation_pct = { x = 0.0, y = 50.0, z = 0.0 }\n";
    assert!(toml_edit::de::from_str::<MaterialDefaults>(toml).is_err());
}

#[test]
fn deserialise_rejects_scale_compensation_non_finite_axis() {
    let toml = "scale_compensation_pct = { x = 0.0, y = nan, z = 0.0 }\n";
    assert!(toml_edit::de::from_str::<MaterialDefaults>(toml).is_err());
}

#[test]
fn deserialise_rejects_negative_bottom_layer_count() {
    let toml = "bottom_layer_count = -1\n";
    assert!(toml_edit::de::from_str::<MaterialDefaults>(toml).is_err());
}

#[test]
fn roundtrip_with_every_field_preserves_value() {
    let original: MaterialDefaults = toml_edit::de::from_str(FULL_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialDefaults = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn roundtrip_empty_section_preserves_all_none() {
    let original: MaterialDefaults = toml_edit::de::from_str("").unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: MaterialDefaults = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
    assert!(roundtripped.scale_compensation_pct.is_none());
    assert!(roundtripped.bottom_layer_count.is_none());
    assert!(roundtripped.wait_before_cure_sec.is_none());
}

#[test]
fn serialise_omits_each_unset_field() {
    let parsed: MaterialDefaults = toml_edit::de::from_str("").unwrap();
    let serialised = toml_edit::ser::to_string(&parsed).unwrap();
    for field in [
        "scale_compensation_pct",
        "bottom_layer_count",
        "wait_before_cure_sec",
        "wait_after_cure_sec",
        "wait_after_lift_sec",
    ] {
        assert!(
            !serialised.contains(field),
            "should omit {field}: {serialised}"
        );
    }
}

#[test]
fn programmatic_construction_round_trips() {
    let defaults = MaterialDefaults {
        scale_compensation_pct: Some(ScaleCompensation {
            x: ScaleCompensationPercent::new(0.5).unwrap(),
            y: ScaleCompensationPercent::new(-0.5).unwrap(),
            z: ScaleCompensationPercent::new(0.0).unwrap(),
        }),
        bottom_layer_count: Some(6),
        wait_before_cure_sec: Some(NonNegativeFiniteSec::new(0.5).unwrap()),
        wait_after_cure_sec: None,
        wait_after_lift_sec: Some(NonNegativeFiniteSec::ZERO),
    };
    let serialised = toml_edit::ser::to_string(&defaults).unwrap();
    let roundtripped: MaterialDefaults = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(defaults, roundtripped);
}
