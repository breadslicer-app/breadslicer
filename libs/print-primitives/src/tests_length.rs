/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::length::{
    LengthOutOfRange, LengthRange, LengthRangeBoundsInverted, ParseLengthError,
    PositiveFiniteMicrons, PositiveFiniteMm,
};

#[test]
fn positive_finite_mm_new_accepts_positive_finite() {
    let v = PositiveFiniteMm::new(0.1).unwrap();
    assert_eq!(v.value(), 0.1);
}

#[test]
fn positive_finite_mm_new_rejects_zero() {
    let err = PositiveFiniteMm::new(0.0).unwrap_err();
    assert_eq!(err.value, 0.0);
}

#[test]
fn positive_finite_mm_new_rejects_negative() {
    let err = PositiveFiniteMm::new(-0.5).unwrap_err();
    assert_eq!(err.value, -0.5);
}

#[test]
fn positive_finite_mm_new_rejects_nan() {
    let err = PositiveFiniteMm::new(f64::NAN).unwrap_err();
    assert!(err.value.is_nan());
}

#[test]
fn positive_finite_mm_new_rejects_infinity() {
    assert!(PositiveFiniteMm::new(f64::INFINITY).is_err());
    assert!(PositiveFiniteMm::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn length_out_of_range_display_includes_value() {
    let msg = format!("{}", LengthOutOfRange { value: -0.5 });
    assert!(msg.contains("-0.5"), "msg: {msg}");
    assert!(msg.contains("positive"), "msg: {msg}");
}

#[test]
fn positive_finite_mm_from_str_parses_valid_input() {
    let v: PositiveFiniteMm = "0.25".parse().unwrap();
    assert_eq!(v.value(), 0.25);
}

#[test]
fn positive_finite_mm_from_str_rejects_non_numeric() {
    let err = "abc".parse::<PositiveFiniteMm>().unwrap_err();
    assert!(matches!(err, ParseLengthError::NotANumber(_)));
    let msg = format!("{err}");
    assert!(msg.contains("invalid"), "msg: {msg}");
}

#[test]
fn positive_finite_mm_from_str_rejects_out_of_range() {
    let err = "-1.0".parse::<PositiveFiniteMm>().unwrap_err();
    assert!(matches!(err, ParseLengthError::OutOfRange(_)));
    let msg = format!("{err}");
    assert!(msg.contains("-1"), "msg: {msg}");
}

#[test]
fn positive_finite_mm_try_from_f64_accepts_positive_finite() {
    let v = PositiveFiniteMm::try_from(0.05).unwrap();
    assert_eq!(v.value(), 0.05);
}

#[test]
fn positive_finite_mm_try_from_f64_rejects_zero() {
    let err = PositiveFiniteMm::try_from(0.0).unwrap_err();
    assert_eq!(err.value, 0.0);
}

#[test]
fn positive_finite_mm_into_f64_returns_inner_value() {
    let v = PositiveFiniteMm::new(0.05).unwrap();
    let raw: f64 = v.into();
    assert_eq!(raw, 0.05);
}

#[test]
fn positive_finite_microns_new_accepts_positive_finite() {
    let v = PositiveFiniteMicrons::new(18.0).unwrap();
    assert_eq!(v.value(), 18.0);
}

#[test]
fn positive_finite_microns_new_rejects_zero() {
    let err = PositiveFiniteMicrons::new(0.0).unwrap_err();
    assert_eq!(err.value, 0.0);
}

#[test]
fn positive_finite_microns_new_rejects_negative() {
    let err = PositiveFiniteMicrons::new(-1.0).unwrap_err();
    assert_eq!(err.value, -1.0);
}

#[test]
fn positive_finite_microns_new_rejects_nan() {
    let err = PositiveFiniteMicrons::new(f64::NAN).unwrap_err();
    assert!(err.value.is_nan());
}

#[test]
fn positive_finite_microns_new_rejects_infinity() {
    assert!(PositiveFiniteMicrons::new(f64::INFINITY).is_err());
    assert!(PositiveFiniteMicrons::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn positive_finite_microns_from_str_parses_valid_input() {
    let v: PositiveFiniteMicrons = "18.5".parse().unwrap();
    assert_eq!(v.value(), 18.5);
}

#[test]
fn positive_finite_microns_from_str_rejects_non_numeric() {
    let err = "abc".parse::<PositiveFiniteMicrons>().unwrap_err();
    assert!(matches!(err, ParseLengthError::NotANumber(_)));
}

#[test]
fn positive_finite_microns_from_str_rejects_out_of_range() {
    let err = "-1.0".parse::<PositiveFiniteMicrons>().unwrap_err();
    assert!(matches!(err, ParseLengthError::OutOfRange(_)));
}

#[test]
fn positive_finite_microns_try_from_f64_accepts_positive_finite() {
    let v = PositiveFiniteMicrons::try_from(18.0).unwrap();
    assert_eq!(v.value(), 18.0);
}

#[test]
fn positive_finite_microns_try_from_f64_rejects_zero() {
    let err = PositiveFiniteMicrons::try_from(0.0).unwrap_err();
    assert_eq!(err.value, 0.0);
}

#[test]
fn positive_finite_microns_into_f64_returns_inner_value() {
    let v = PositiveFiniteMicrons::new(18.0).unwrap();
    let raw: f64 = v.into();
    assert_eq!(raw, 18.0);
}

#[test]
fn length_out_of_range_display_is_unit_agnostic() {
    let msg = format!("{}", LengthOutOfRange { value: -0.5 });
    assert!(
        !msg.contains("millimetre"),
        "Display should be unit-agnostic: {msg}"
    );
    assert!(msg.contains("length"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mm_roundtrip_through_toml() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrapper {
        value: PositiveFiniteMm,
    }
    let original = Wrapper {
        value: PositiveFiniteMm::new(0.05).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrapper = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mm_deserialise_rejects_zero() {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[allow(dead_code)]
        value: PositiveFiniteMm,
    }
    let toml = "value = 0.0\n";
    assert!(toml_edit::de::from_str::<Wrapper>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mm_deserialise_rejects_negative() {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[allow(dead_code)]
        value: PositiveFiniteMm,
    }
    let toml = "value = -1.0\n";
    assert!(toml_edit::de::from_str::<Wrapper>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_microns_roundtrip_through_toml() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrapper {
        value: PositiveFiniteMicrons,
    }
    let original = Wrapper {
        value: PositiveFiniteMicrons::new(18.0).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrapper = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_microns_deserialise_rejects_zero() {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[allow(dead_code)]
        value: PositiveFiniteMicrons,
    }
    let toml = "value = 0.0\n";
    assert!(toml_edit::de::from_str::<Wrapper>(toml).is_err());
}

#[test]
fn length_range_new_accepts_strictly_ordered_bounds() {
    let min = PositiveFiniteMm::new(0.01).unwrap();
    let max = PositiveFiniteMm::new(0.15).unwrap();
    let r = LengthRange::new(min, max).unwrap();
    assert_eq!(r.min().value(), 0.01);
    assert_eq!(r.max().value(), 0.15);
}

#[test]
fn length_range_new_rejects_equal_bounds() {
    let bound = PositiveFiniteMm::new(0.05).unwrap();
    let err = LengthRange::new(bound, bound).unwrap_err();
    assert_eq!(
        err,
        LengthRangeBoundsInverted {
            min: 0.05,
            max: 0.05,
        }
    );
}

#[test]
fn length_range_new_rejects_inverted_bounds() {
    let min = PositiveFiniteMm::new(0.15).unwrap();
    let max = PositiveFiniteMm::new(0.01).unwrap();
    let err = LengthRange::new(min, max).unwrap_err();
    assert_eq!(
        err,
        LengthRangeBoundsInverted {
            min: 0.15,
            max: 0.01,
        }
    );
}

#[test]
fn length_range_bounds_inverted_display_mentions_unit_and_values() {
    let err = LengthRangeBoundsInverted { min: 0.2, max: 0.1 };
    let msg = format!("{err}");
    assert!(msg.contains("0.2"), "msg: {msg}");
    assert!(msg.contains("0.1"), "msg: {msg}");
    assert!(msg.contains("mm"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn length_range_deserialises_valid_toml_table() {
    let toml = r#"
min = 0.01
max = 0.15
"#;
    let parsed: LengthRange = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.min().value(), 0.01);
    assert_eq!(parsed.max().value(), 0.15);
}

#[cfg(feature = "serde")]
#[test]
fn length_range_deserialise_rejects_inverted_bounds() {
    let toml = r#"
min = 0.15
max = 0.01
"#;
    assert!(toml_edit::de::from_str::<LengthRange>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn length_range_deserialise_rejects_equal_bounds() {
    let toml = r#"
min = 0.05
max = 0.05
"#;
    assert!(toml_edit::de::from_str::<LengthRange>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn length_range_deserialise_rejects_non_positive_min() {
    let toml = r#"
min = 0.0
max = 0.1
"#;
    assert!(toml_edit::de::from_str::<LengthRange>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn length_range_deserialise_rejects_non_finite_max() {
    let toml = r#"
min = 0.01
max = nan
"#;
    assert!(toml_edit::de::from_str::<LengthRange>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn length_range_serialise_emits_min_and_max_fields() {
    let r = LengthRange::new(
        PositiveFiniteMm::new(0.01).unwrap(),
        PositiveFiniteMm::new(0.15).unwrap(),
    )
    .unwrap();
    #[derive(serde::Serialize)]
    struct Wrap {
        range: LengthRange,
    }
    let toml = toml_edit::ser::to_string(&Wrap { range: r }).unwrap();
    assert!(toml.contains("min"), "toml: {toml}");
    assert!(toml.contains("max"), "toml: {toml}");
    assert!(toml.contains("0.01"), "toml: {toml}");
    assert!(toml.contains("0.15"), "toml: {toml}");
}

#[cfg(feature = "serde")]
#[test]
fn length_range_roundtrip_preserves_value() {
    let original = LengthRange::new(
        PositiveFiniteMm::new(0.02).unwrap(),
        PositiveFiniteMm::new(0.20).unwrap(),
    )
    .unwrap();
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        range: LengthRange,
    }
    let wrap = Wrap { range: original };
    let serialised = toml_edit::ser::to_string(&wrap).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(wrap, roundtripped);
}
