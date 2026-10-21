/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::scale_compensation::{
    ScaleCompensation, ScaleCompensationPercent, ScaleCompensationPercentOutOfBound,
};

#[test]
fn scale_compensation_percent_new_accepts_zero() {
    let v = ScaleCompensationPercent::new(0.0).unwrap();
    assert_eq!(v.value(), 0.0);
}

#[test]
fn scale_compensation_percent_new_accepts_lower_bound() {
    let v = ScaleCompensationPercent::new(-10.0).unwrap();
    assert_eq!(v.value(), -10.0);
}

#[test]
fn scale_compensation_percent_new_accepts_upper_bound() {
    let v = ScaleCompensationPercent::new(10.0).unwrap();
    assert_eq!(v.value(), 10.0);
}

#[test]
fn scale_compensation_percent_new_accepts_value_inside_range() {
    let v = ScaleCompensationPercent::new(3.5).unwrap();
    assert_eq!(v.value(), 3.5);
}

#[test]
fn scale_compensation_percent_new_rejects_just_below_lower_bound() {
    let err = ScaleCompensationPercent::new(-10.001).unwrap_err();
    assert_eq!(err.value, -10.001);
    assert_eq!(err.min, -10.0);
    assert_eq!(err.max, 10.0);
}

#[test]
fn scale_compensation_percent_new_rejects_just_above_upper_bound() {
    let err = ScaleCompensationPercent::new(10.001).unwrap_err();
    assert_eq!(err.value, 10.001);
}

#[test]
fn scale_compensation_percent_new_rejects_far_below() {
    assert!(ScaleCompensationPercent::new(-50.0).is_err());
}

#[test]
fn scale_compensation_percent_new_rejects_nan() {
    assert!(ScaleCompensationPercent::new(f64::NAN).is_err());
}

#[test]
fn scale_compensation_percent_new_rejects_positive_infinity() {
    assert!(ScaleCompensationPercent::new(f64::INFINITY).is_err());
}

#[test]
fn scale_compensation_percent_new_rejects_negative_infinity() {
    assert!(ScaleCompensationPercent::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn scale_compensation_percent_out_of_bound_display_mentions_bounds_and_value() {
    let err = ScaleCompensationPercentOutOfBound {
        value: 12.5,
        min: -10.0,
        max: 10.0,
    };
    let msg = format!("{err}");
    assert!(msg.contains("12.5"), "msg: {msg}");
    assert!(msg.contains("-10"), "msg: {msg}");
    assert!(msg.contains("10"), "msg: {msg}");
    assert!(msg.contains("%"), "msg: {msg}");
}

#[test]
fn scale_compensation_percent_try_from_f64_round_trips_to_f64() {
    let v: ScaleCompensationPercent = (-1.5).try_into().unwrap();
    let back: f64 = v.into();
    assert_eq!(back, -1.5);
}

#[test]
fn scale_compensation_percent_display_formats_inner_value() {
    let v = ScaleCompensationPercent::new(-2.5).unwrap();
    let msg = format!("{v}");
    assert!(msg.contains("-2.5"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn scale_compensation_percent_deserialises_value_inside_range() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: ScaleCompensationPercent,
    }
    let toml = "value = 2.5\n";
    let parsed: Wrap = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.value.value(), 2.5);
}

#[cfg(feature = "serde")]
#[test]
fn scale_compensation_percent_deserialise_rejects_above_upper_bound() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: ScaleCompensationPercent,
    }
    let toml = "value = 11.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn scale_compensation_percent_deserialise_rejects_below_lower_bound() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: ScaleCompensationPercent,
    }
    let toml = "value = -15.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn scale_compensation_percent_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: ScaleCompensationPercent,
    }
    let original = Wrap {
        value: ScaleCompensationPercent::new(-3.0).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[cfg(feature = "serde")]
#[test]
fn scale_compensation_deserialises_full_axis_table() {
    let toml = r#"
x = 0.0
y = 0.5
z = -1.0
"#;
    let parsed: ScaleCompensation = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.x.value(), 0.0);
    assert_eq!(parsed.y.value(), 0.5);
    assert_eq!(parsed.z.value(), -1.0);
}

#[cfg(feature = "serde")]
#[test]
fn scale_compensation_deserialise_rejects_axis_outside_range() {
    let toml = r#"
x = 0.0
y = 50.0
z = 0.0
"#;
    assert!(toml_edit::de::from_str::<ScaleCompensation>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn scale_compensation_roundtrip_preserves_value() {
    let original = ScaleCompensation {
        x: ScaleCompensationPercent::new(0.0).unwrap(),
        y: ScaleCompensationPercent::new(2.5).unwrap(),
        z: ScaleCompensationPercent::new(-1.5).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: ScaleCompensation = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}
