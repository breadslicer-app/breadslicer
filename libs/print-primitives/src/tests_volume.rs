/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::volume::{VolumeMm3, VolumeOutOfRange};

#[test]
fn volume_mm3_new_accepts_zero() {
    let v = VolumeMm3::new(0.0).unwrap();
    assert_eq!(v.value(), 0.0);
}

#[test]
fn volume_mm3_new_accepts_positive_finite_value() {
    let v = VolumeMm3::new(11701.231442).unwrap();
    assert_eq!(v.value(), 11701.231442);
}

#[test]
fn volume_mm3_new_rejects_negative() {
    let err = VolumeMm3::new(-1.0).unwrap_err();
    assert_eq!(err, VolumeOutOfRange { value: -1.0 });
}

#[test]
fn volume_mm3_new_rejects_nan() {
    assert!(VolumeMm3::new(f64::NAN).is_err());
}

#[test]
fn volume_mm3_new_rejects_positive_infinity() {
    assert!(VolumeMm3::new(f64::INFINITY).is_err());
}

#[test]
fn volume_mm3_new_rejects_negative_infinity() {
    assert!(VolumeMm3::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn volume_mm3_zero_matches_the_constructed_zero() {
    assert_eq!(VolumeMm3::ZERO, VolumeMm3::new(0.0).unwrap());
}

#[test]
fn volume_out_of_range_display_mentions_unit_and_value() {
    let err = VolumeOutOfRange { value: -1.25 };
    let msg = format!("{err}");
    assert!(msg.contains("-1.25"), "msg: {msg}");
    assert!(msg.contains("mm^3"), "msg: {msg}");
    assert!(msg.contains("volume"), "msg: {msg}");
}

#[test]
fn volume_mm3_try_from_f64_round_trips_to_f64() {
    let v: VolumeMm3 = 12.5.try_into().unwrap();
    let back: f64 = v.into();
    assert_eq!(back, 12.5);
}

#[test]
fn volume_mm3_try_from_f64_rejects_negative() {
    let outcome: Result<VolumeMm3, _> = (-0.5).try_into();
    assert!(outcome.is_err());
}

#[test]
fn volume_mm3_display_formats_inner_value() {
    let v = VolumeMm3::new(15.5).unwrap();
    let msg = format!("{v}");
    assert!(msg.contains("15.5"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn volume_mm3_deserialises_zero() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: VolumeMm3,
    }
    let toml = "value = 0.0\n";
    let parsed: Wrap = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.value.value(), 0.0);
}

#[cfg(feature = "serde")]
#[test]
fn volume_mm3_deserialise_rejects_negative() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: VolumeMm3,
    }
    let toml = "value = -3.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn volume_mm3_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: VolumeMm3,
    }
    let original = Wrap {
        value: VolumeMm3::new(18.0).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}
