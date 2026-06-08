/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::light_intensity::{MwPerCm2OutOfRange, PositiveFiniteMwPerCm2};

#[test]
fn positive_finite_mw_per_cm2_new_accepts_positive_finite_value() {
    let v = PositiveFiniteMwPerCm2::new(4.2).unwrap();
    assert_eq!(v.value(), 4.2);
}

#[test]
fn positive_finite_mw_per_cm2_new_rejects_zero() {
    let err = PositiveFiniteMwPerCm2::new(0.0).unwrap_err();
    assert_eq!(err, MwPerCm2OutOfRange { value: 0.0 });
}

#[test]
fn positive_finite_mw_per_cm2_new_rejects_negative() {
    let err = PositiveFiniteMwPerCm2::new(-1.5).unwrap_err();
    assert_eq!(err, MwPerCm2OutOfRange { value: -1.5 });
}

#[test]
fn positive_finite_mw_per_cm2_new_rejects_nan() {
    assert!(PositiveFiniteMwPerCm2::new(f64::NAN).is_err());
}

#[test]
fn positive_finite_mw_per_cm2_new_rejects_positive_infinity() {
    assert!(PositiveFiniteMwPerCm2::new(f64::INFINITY).is_err());
}

#[test]
fn positive_finite_mw_per_cm2_new_rejects_negative_infinity() {
    assert!(PositiveFiniteMwPerCm2::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn mw_per_cm2_out_of_range_display_mentions_unit_and_value() {
    let err = MwPerCm2OutOfRange { value: -0.5 };
    let msg = format!("{err}");
    assert!(msg.contains("-0.5"), "msg: {msg}");
    assert!(msg.contains("mW/cm^2"), "msg: {msg}");
    assert!(msg.contains("light intensity"), "msg: {msg}");
}

#[test]
fn positive_finite_mw_per_cm2_try_from_f64_round_trips_to_f64() {
    let v: PositiveFiniteMwPerCm2 = 5.5.try_into().unwrap();
    let back: f64 = v.into();
    assert_eq!(back, 5.5);
}

#[test]
fn positive_finite_mw_per_cm2_display_formats_inner_value() {
    let v = PositiveFiniteMwPerCm2::new(4.2).unwrap();
    let msg = format!("{v}");
    assert!(msg.contains("4.2"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mw_per_cm2_deserialises_positive_f64() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: PositiveFiniteMwPerCm2,
    }
    let toml = "value = 4.2\n";
    let parsed: Wrap = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.value.value(), 4.2);
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mw_per_cm2_deserialise_rejects_zero() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: PositiveFiniteMwPerCm2,
    }
    let toml = "value = 0.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mw_per_cm2_deserialise_rejects_negative() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: PositiveFiniteMwPerCm2,
    }
    let toml = "value = -1.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mw_per_cm2_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: PositiveFiniteMwPerCm2,
    }
    let original = Wrap {
        value: PositiveFiniteMwPerCm2::new(4.5).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}
