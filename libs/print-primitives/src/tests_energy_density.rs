/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::energy_density::{MjPerCm2OutOfRange, PositiveFiniteMjPerCm2};

#[test]
fn positive_finite_mj_per_cm2_new_accepts_positive_finite_value() {
    let v = PositiveFiniteMjPerCm2::new(15.5).unwrap();
    assert_eq!(v.value(), 15.5);
}

#[test]
fn positive_finite_mj_per_cm2_new_rejects_zero() {
    let err = PositiveFiniteMjPerCm2::new(0.0).unwrap_err();
    assert_eq!(err, MjPerCm2OutOfRange { value: 0.0 });
}

#[test]
fn positive_finite_mj_per_cm2_new_rejects_negative() {
    let err = PositiveFiniteMjPerCm2::new(-2.0).unwrap_err();
    assert_eq!(err, MjPerCm2OutOfRange { value: -2.0 });
}

#[test]
fn positive_finite_mj_per_cm2_new_rejects_nan() {
    assert!(PositiveFiniteMjPerCm2::new(f64::NAN).is_err());
}

#[test]
fn positive_finite_mj_per_cm2_new_rejects_positive_infinity() {
    assert!(PositiveFiniteMjPerCm2::new(f64::INFINITY).is_err());
}

#[test]
fn positive_finite_mj_per_cm2_new_rejects_negative_infinity() {
    assert!(PositiveFiniteMjPerCm2::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn mj_per_cm2_out_of_range_display_mentions_unit_and_value() {
    let err = MjPerCm2OutOfRange { value: -1.25 };
    let msg = format!("{err}");
    assert!(msg.contains("-1.25"), "msg: {msg}");
    assert!(msg.contains("mJ/cm^2"), "msg: {msg}");
    assert!(msg.contains("energy density"), "msg: {msg}");
}

#[test]
fn positive_finite_mj_per_cm2_try_from_f64_round_trips_to_f64() {
    let v: PositiveFiniteMjPerCm2 = 12.5.try_into().unwrap();
    let back: f64 = v.into();
    assert_eq!(back, 12.5);
}

#[test]
fn positive_finite_mj_per_cm2_display_formats_inner_value() {
    let v = PositiveFiniteMjPerCm2::new(15.5).unwrap();
    let msg = format!("{v}");
    assert!(msg.contains("15.5"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mj_per_cm2_deserialises_positive_f64() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: PositiveFiniteMjPerCm2,
    }
    let toml = "value = 15.5\n";
    let parsed: Wrap = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.value.value(), 15.5);
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mj_per_cm2_deserialise_rejects_zero() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: PositiveFiniteMjPerCm2,
    }
    let toml = "value = 0.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mj_per_cm2_deserialise_rejects_negative() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: PositiveFiniteMjPerCm2,
    }
    let toml = "value = -3.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_mj_per_cm2_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: PositiveFiniteMjPerCm2,
    }
    let original = Wrap {
        value: PositiveFiniteMjPerCm2::new(18.0).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}
