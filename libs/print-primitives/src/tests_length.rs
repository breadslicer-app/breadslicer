/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::length::{LengthOutOfRange, ParseLengthError, PositiveFiniteMicrons, PositiveFiniteMm};

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
