/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::speed::{
    PositiveFiniteMmPerMin, SpeedMmPerMinOutOfRange, SpeedRangeMmPerMin,
    SpeedRangeMmPerMinBoundsInverted,
};

#[test]
fn positive_finite_mm_per_min_new_accepts_positive_finite_value() {
    let v = PositiveFiniteMmPerMin::new(60.0).unwrap();
    assert_eq!(v.value(), 60.0);
}

#[test]
fn positive_finite_mm_per_min_new_rejects_zero() {
    let err = PositiveFiniteMmPerMin::new(0.0).unwrap_err();
    assert_eq!(err, SpeedMmPerMinOutOfRange { value: 0.0 });
}

#[test]
fn positive_finite_mm_per_min_new_rejects_negative() {
    let err = PositiveFiniteMmPerMin::new(-1.0).unwrap_err();
    assert_eq!(err, SpeedMmPerMinOutOfRange { value: -1.0 });
}

#[test]
fn positive_finite_mm_per_min_new_rejects_nan() {
    assert!(PositiveFiniteMmPerMin::new(f64::NAN).is_err());
}

#[test]
fn positive_finite_mm_per_min_new_rejects_positive_infinity() {
    assert!(PositiveFiniteMmPerMin::new(f64::INFINITY).is_err());
}

#[test]
fn positive_finite_mm_per_min_new_rejects_negative_infinity() {
    assert!(PositiveFiniteMmPerMin::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn speed_mm_per_min_out_of_range_display_mentions_unit_and_value() {
    let err = SpeedMmPerMinOutOfRange { value: -2.5 };
    let msg = format!("{err}");
    assert!(msg.contains("-2.5"), "msg: {msg}");
    assert!(msg.contains("mm/min"), "msg: {msg}");
}

#[test]
fn positive_finite_mm_per_min_try_from_f64_round_trips_to_f64() {
    let v: PositiveFiniteMmPerMin = 42.0.try_into().unwrap();
    let back: f64 = v.into();
    assert_eq!(back, 42.0);
}

#[test]
fn speed_range_mm_per_min_new_accepts_strictly_ordered_bounds() {
    let min = PositiveFiniteMmPerMin::new(30.0).unwrap();
    let max = PositiveFiniteMmPerMin::new(300.0).unwrap();
    let r = SpeedRangeMmPerMin::new(min, max).unwrap();
    assert_eq!(r.min().value(), 30.0);
    assert_eq!(r.max().value(), 300.0);
}

#[test]
fn speed_range_mm_per_min_new_rejects_equal_bounds() {
    let bound = PositiveFiniteMmPerMin::new(60.0).unwrap();
    let err = SpeedRangeMmPerMin::new(bound, bound).unwrap_err();
    assert_eq!(
        err,
        SpeedRangeMmPerMinBoundsInverted {
            min: 60.0,
            max: 60.0,
        }
    );
}

#[test]
fn speed_range_mm_per_min_new_rejects_inverted_bounds() {
    let min = PositiveFiniteMmPerMin::new(300.0).unwrap();
    let max = PositiveFiniteMmPerMin::new(30.0).unwrap();
    let err = SpeedRangeMmPerMin::new(min, max).unwrap_err();
    assert_eq!(
        err,
        SpeedRangeMmPerMinBoundsInverted {
            min: 300.0,
            max: 30.0,
        }
    );
}

#[test]
fn speed_range_mm_per_min_bounds_inverted_display_mentions_unit_and_values() {
    let err = SpeedRangeMmPerMinBoundsInverted {
        min: 100.0,
        max: 50.0,
    };
    let msg = format!("{err}");
    assert!(msg.contains("100"), "msg: {msg}");
    assert!(msg.contains("50"), "msg: {msg}");
    assert!(msg.contains("mm/min"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mm_per_min_deserialises_positive_f64() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: PositiveFiniteMmPerMin,
    }
    let toml = "value = 75.0\n";
    let parsed: Wrap = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.value.value(), 75.0);
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mm_per_min_deserialise_rejects_zero() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: PositiveFiniteMmPerMin,
    }
    let toml = "value = 0.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mm_per_min_deserialise_rejects_negative() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: PositiveFiniteMmPerMin,
    }
    let toml = "value = -10.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mm_per_min_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: PositiveFiniteMmPerMin,
    }
    let original = Wrap {
        value: PositiveFiniteMmPerMin::new(120.0).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[cfg(feature = "serde")]
#[test]
fn speed_range_mm_per_min_deserialises_valid_toml_table() {
    let toml = r#"
min = 30.0
max = 300.0
"#;
    let parsed: SpeedRangeMmPerMin = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.min().value(), 30.0);
    assert_eq!(parsed.max().value(), 300.0);
}

#[cfg(feature = "serde")]
#[test]
fn speed_range_mm_per_min_deserialise_rejects_inverted_bounds() {
    let toml = r#"
min = 300.0
max = 30.0
"#;
    assert!(toml_edit::de::from_str::<SpeedRangeMmPerMin>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn speed_range_mm_per_min_deserialise_rejects_equal_bounds() {
    let toml = r#"
min = 60.0
max = 60.0
"#;
    assert!(toml_edit::de::from_str::<SpeedRangeMmPerMin>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn speed_range_mm_per_min_deserialise_rejects_non_positive_min() {
    let toml = r#"
min = 0.0
max = 100.0
"#;
    assert!(toml_edit::de::from_str::<SpeedRangeMmPerMin>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn speed_range_mm_per_min_deserialise_rejects_non_finite_max() {
    let toml = r#"
min = 30.0
max = nan
"#;
    assert!(toml_edit::de::from_str::<SpeedRangeMmPerMin>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn speed_range_mm_per_min_serialise_emits_min_and_max_fields() {
    let r = SpeedRangeMmPerMin::new(
        PositiveFiniteMmPerMin::new(30.0).unwrap(),
        PositiveFiniteMmPerMin::new(300.0).unwrap(),
    )
    .unwrap();
    #[derive(serde::Serialize)]
    struct Wrap {
        range: SpeedRangeMmPerMin,
    }
    let toml = toml_edit::ser::to_string(&Wrap { range: r }).unwrap();
    assert!(toml.contains("min"), "toml: {toml}");
    assert!(toml.contains("max"), "toml: {toml}");
    assert!(toml.contains("30"), "toml: {toml}");
    assert!(toml.contains("300"), "toml: {toml}");
}

#[cfg(feature = "serde")]
#[test]
fn speed_range_mm_per_min_roundtrip_preserves_value() {
    let original = SpeedRangeMmPerMin::new(
        PositiveFiniteMmPerMin::new(60.0).unwrap(),
        PositiveFiniteMmPerMin::new(180.0).unwrap(),
    )
    .unwrap();
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        range: SpeedRangeMmPerMin,
    }
    let wrap = Wrap { range: original };
    let serialised = toml_edit::ser::to_string(&wrap).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(wrap, roundtripped);
}
