/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::time::{NegativeDuration, NonNegativeFiniteSec, PositiveFiniteSec, SecOutOfRange};

#[test]
fn positive_finite_sec_new_accepts_positive_finite_value() {
    let v = PositiveFiniteSec::new(2.5).unwrap();
    assert_eq!(v.value(), 2.5);
}

#[test]
fn positive_finite_sec_new_rejects_zero() {
    let err = PositiveFiniteSec::new(0.0).unwrap_err();
    assert_eq!(err, SecOutOfRange { value: 0.0 });
}

#[test]
fn positive_finite_sec_new_rejects_negative() {
    let err = PositiveFiniteSec::new(-1.0).unwrap_err();
    assert_eq!(err, SecOutOfRange { value: -1.0 });
}

#[test]
fn positive_finite_sec_new_rejects_nan() {
    assert!(PositiveFiniteSec::new(f64::NAN).is_err());
}

#[test]
fn positive_finite_sec_new_rejects_positive_infinity() {
    assert!(PositiveFiniteSec::new(f64::INFINITY).is_err());
}

#[test]
fn positive_finite_sec_new_rejects_negative_infinity() {
    assert!(PositiveFiniteSec::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn sec_out_of_range_display_mentions_unit_and_value() {
    let err = SecOutOfRange { value: -0.75 };
    let msg = format!("{err}");
    assert!(msg.contains("-0.75"), "msg: {msg}");
    assert!(msg.contains(" s"), "msg: {msg}");
    assert!(msg.contains("duration"), "msg: {msg}");
}

#[test]
fn positive_finite_sec_try_from_f64_round_trips_to_f64() {
    let v: PositiveFiniteSec = 30.0.try_into().unwrap();
    let back: f64 = v.into();
    assert_eq!(back, 30.0);
}

#[test]
fn positive_finite_sec_display_formats_inner_value() {
    let v = PositiveFiniteSec::new(2.5).unwrap();
    let msg = format!("{v}");
    assert!(msg.contains("2.5"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_sec_deserialises_positive_f64() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: PositiveFiniteSec,
    }
    let toml = "value = 2.5\n";
    let parsed: Wrap = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.value.value(), 2.5);
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_sec_deserialise_rejects_zero() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: PositiveFiniteSec,
    }
    let toml = "value = 0.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_sec_deserialise_rejects_negative() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: PositiveFiniteSec,
    }
    let toml = "value = -1.0\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn positive_finite_sec_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: PositiveFiniteSec,
    }
    let original = Wrap {
        value: PositiveFiniteSec::new(30.0).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn non_negative_finite_sec_new_accepts_zero() {
    let v = NonNegativeFiniteSec::new(0.0).unwrap();
    assert_eq!(v.value(), 0.0);
}

#[test]
fn non_negative_finite_sec_new_accepts_positive_finite_value() {
    let v = NonNegativeFiniteSec::new(2.5).unwrap();
    assert_eq!(v.value(), 2.5);
}

#[test]
fn non_negative_finite_sec_new_rejects_negative() {
    let err = NonNegativeFiniteSec::new(-1.0).unwrap_err();
    assert_eq!(err, NegativeDuration { value: -1.0 });
}

#[test]
fn non_negative_finite_sec_new_rejects_nan() {
    assert!(NonNegativeFiniteSec::new(f64::NAN).is_err());
}

#[test]
fn non_negative_finite_sec_new_rejects_positive_infinity() {
    assert!(NonNegativeFiniteSec::new(f64::INFINITY).is_err());
}

#[test]
fn non_negative_finite_sec_new_rejects_negative_infinity() {
    assert!(NonNegativeFiniteSec::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn non_negative_finite_sec_zero_constant_matches_the_constructor() {
    assert_eq!(
        NonNegativeFiniteSec::ZERO,
        NonNegativeFiniteSec::new(0.0).unwrap()
    );
}

#[test]
fn negative_duration_display_mentions_unit_and_value() {
    let err = NegativeDuration { value: -0.75 };
    let msg = format!("{err}");
    assert!(msg.contains("-0.75"), "msg: {msg}");
    assert!(msg.contains(" s"), "msg: {msg}");
    assert!(msg.contains("non-negative"), "msg: {msg}");
}

#[test]
fn non_negative_finite_sec_widens_from_positive_finite_sec() {
    let positive = PositiveFiniteSec::new(2.5).unwrap();
    assert_eq!(NonNegativeFiniteSec::from(positive).value(), 2.5);
}

#[test]
fn non_negative_finite_sec_try_from_f64_round_trips_to_f64() {
    let v: NonNegativeFiniteSec = 0.0.try_into().unwrap();
    let back: f64 = v.into();
    assert_eq!(back, 0.0);
}

#[test]
fn non_negative_finite_sec_display_formats_inner_value() {
    let v = NonNegativeFiniteSec::new(2.5).unwrap();
    let msg = format!("{v}");
    assert!(msg.contains("2.5"), "msg: {msg}");
}

#[cfg(feature = "serde")]
#[test]
fn non_negative_finite_sec_deserialises_zero() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: NonNegativeFiniteSec,
    }
    let parsed: Wrap = toml_edit::de::from_str("value = 0.0\n").unwrap();
    assert_eq!(parsed.value.value(), 0.0);
}

#[cfg(feature = "serde")]
#[test]
fn non_negative_finite_sec_deserialise_rejects_negative() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: NonNegativeFiniteSec,
    }
    assert!(toml_edit::de::from_str::<Wrap>("value = -1.0\n").is_err());
}

#[cfg(feature = "serde")]
#[test]
fn non_negative_finite_sec_roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: NonNegativeFiniteSec,
    }
    let original = Wrap {
        value: NonNegativeFiniteSec::new(1.5).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}
