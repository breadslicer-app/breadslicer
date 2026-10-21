/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::time::{
    NegativeDuration, NonNegativeFiniteSec, PositiveFiniteSec, SecOutOfRange, TimeMultiplier,
    TimeMultiplierOutOfRange, TimeOffsetOutOfRange, TimeOffsetSec,
};

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
fn interpolate_towards_at_zero_returns_the_starting_duration() {
    let from = PositiveFiniteSec::new(30.0).unwrap();
    let to = PositiveFiniteSec::new(2.5).unwrap();
    assert_eq!(from.interpolate_towards(to, 0.0).value(), 30.0);
}

#[test]
fn interpolate_towards_at_one_reaches_the_target_duration() {
    let from = PositiveFiniteSec::new(30.0).unwrap();
    let to = PositiveFiniteSec::new(2.5).unwrap();
    assert_eq!(from.interpolate_towards(to, 1.0).value(), 2.5);
}

#[test]
fn interpolate_towards_steps_proportionally() {
    let from = PositiveFiniteSec::new(30.0).unwrap();
    let to = PositiveFiniteSec::new(2.5).unwrap();
    // 30 - 27.5 / 6 * 2, the second step of a five-layer ramp.
    let stepped = from.interpolate_towards(to, 2.0 / 6.0).value();
    assert!(
        (stepped - 30.0 + 27.5 / 6.0 * 2.0).abs() < 1e-12,
        "stepped: {stepped}",
    );
}

#[test]
fn interpolate_towards_clamps_fractions_outside_the_unit_interval() {
    let from = PositiveFiniteSec::new(30.0).unwrap();
    let to = PositiveFiniteSec::new(2.5).unwrap();
    assert_eq!(from.interpolate_towards(to, 4.0).value(), 2.5);
    assert_eq!(from.interpolate_towards(to, -4.0).value(), 30.0);
    assert_eq!(from.interpolate_towards(to, f64::INFINITY).value(), 2.5);
}

#[test]
fn interpolate_towards_treats_a_nan_fraction_as_no_step() {
    let from = PositiveFiniteSec::new(30.0).unwrap();
    let to = PositiveFiniteSec::new(2.5).unwrap();
    assert_eq!(from.interpolate_towards(to, f64::NAN).value(), 30.0);
}

#[test]
fn interpolate_towards_stays_in_range_at_the_extremes() {
    let tiny = PositiveFiniteSec::new(f64::MIN_POSITIVE).unwrap();
    let huge = PositiveFiniteSec::new(f64::MAX).unwrap();
    for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
        for (from, to) in [(tiny, huge), (huge, tiny)] {
            let value = from.interpolate_towards(to, fraction).value();
            assert!(
                PositiveFiniteSec::new(value).is_ok(),
                "fraction {fraction} produced {value}",
            );
        }
    }
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

#[test]
fn time_multiplier_new_accepts_positive_finite_value() {
    assert_eq!(TimeMultiplier::new(1.322).unwrap().value(), 1.322);
}

#[test]
fn time_multiplier_new_rejects_zero() {
    let err = TimeMultiplier::new(0.0).unwrap_err();
    assert_eq!(err, TimeMultiplierOutOfRange { value: 0.0 });
}

#[test]
fn time_multiplier_new_rejects_negative() {
    assert!(TimeMultiplier::new(-1.0).is_err());
}

#[test]
fn time_multiplier_new_rejects_nan() {
    assert!(TimeMultiplier::new(f64::NAN).is_err());
}

#[test]
fn time_multiplier_new_rejects_positive_infinity() {
    assert!(TimeMultiplier::new(f64::INFINITY).is_err());
}

#[test]
fn time_multiplier_identity_matches_one() {
    assert_eq!(TimeMultiplier::IDENTITY, TimeMultiplier::new(1.0).unwrap());
}

#[test]
fn time_multiplier_out_of_range_display_mentions_the_value() {
    let msg = TimeMultiplierOutOfRange { value: -2.5 }.to_string();
    assert!(msg.contains("-2.5"), "msg: {msg}");
    assert!(msg.contains("multiplier"), "msg: {msg}");
}

#[test]
fn time_multiplier_try_from_f64_round_trips_to_f64() {
    let m: TimeMultiplier = 1.5.try_into().unwrap();
    let back: f64 = m.into();
    assert_eq!(back, 1.5);
}

#[test]
fn time_multiplier_display_formats_inner_value() {
    assert!(
        TimeMultiplier::new(1.25)
            .unwrap()
            .to_string()
            .contains("1.25"),
    );
}

#[test]
fn time_offset_sec_accepts_a_negative_value() {
    assert_eq!(TimeOffsetSec::new(-30.0).unwrap().value(), -30.0);
}

#[test]
fn time_offset_sec_accepts_zero_and_positive_values() {
    assert_eq!(TimeOffsetSec::new(0.0).unwrap().value(), 0.0);
    assert_eq!(TimeOffsetSec::new(45.5).unwrap().value(), 45.5);
}

#[test]
fn time_offset_sec_rejects_nan() {
    let err = TimeOffsetSec::new(f64::NAN).unwrap_err();
    assert!(err.value.is_nan());
}

#[test]
fn time_offset_sec_rejects_infinities() {
    assert!(TimeOffsetSec::new(f64::INFINITY).is_err());
    assert!(TimeOffsetSec::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn time_offset_sec_zero_matches_the_constructed_zero() {
    assert_eq!(TimeOffsetSec::ZERO, TimeOffsetSec::new(0.0).unwrap());
}

#[test]
fn time_offset_out_of_range_display_mentions_unit_and_value() {
    let msg = TimeOffsetOutOfRange {
        value: f64::INFINITY,
    }
    .to_string();
    assert!(msg.contains("inf"), "msg: {msg}");
    assert!(msg.contains(" s"), "msg: {msg}");
}

#[test]
fn time_offset_sec_try_from_f64_round_trips_to_f64() {
    let o: TimeOffsetSec = (-12.5).try_into().unwrap();
    let back: f64 = o.into();
    assert_eq!(back, -12.5);
}

#[test]
fn time_offset_sec_display_formats_inner_value() {
    assert!(
        TimeOffsetSec::new(-7.5)
            .unwrap()
            .to_string()
            .contains("-7.5")
    );
}

#[cfg(feature = "serde")]
#[test]
fn time_multiplier_deserialise_rejects_zero() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: TimeMultiplier,
    }
    assert!(toml_edit::de::from_str::<Wrap>("value = 0.0\n").is_err());
}

#[cfg(feature = "serde")]
#[test]
fn time_offset_sec_deserialises_a_negative_value() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: TimeOffsetSec,
    }
    let parsed: Wrap = toml_edit::de::from_str("value = -30.0\n").unwrap();
    assert_eq!(parsed.value.value(), -30.0);
}

#[cfg(feature = "serde")]
#[test]
fn the_correction_pair_roundtrips_through_toml() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        multiplier: TimeMultiplier,
        offset: TimeOffsetSec,
    }
    let original = Wrap {
        multiplier: TimeMultiplier::new(1.322).unwrap(),
        offset: TimeOffsetSec::new(-15.0).unwrap(),
    };
    let rendered = toml_edit::ser::to_string(&original).unwrap();
    let parsed: Wrap = toml_edit::de::from_str(&rendered).unwrap();
    assert_eq!(parsed, original);
}
