/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::light_pwm::{LightPwm, LightPwmOutOfRange};

#[test]
fn new_accepts_the_lower_bound() {
    assert_eq!(LightPwm::new(0.0).unwrap().percent(), 0.0);
}

#[test]
fn new_accepts_the_upper_bound() {
    assert_eq!(LightPwm::new(100.0).unwrap().percent(), 100.0);
}

#[test]
fn new_accepts_an_interior_value() {
    assert_eq!(LightPwm::new(62.5).unwrap().percent(), 62.5);
}

#[test]
fn new_rejects_above_the_upper_bound() {
    let err = LightPwm::new(100.1).unwrap_err();
    assert_eq!(
        err,
        LightPwmOutOfRange {
            value: 100.1,
            min: 0.0,
            max: 100.0,
        },
    );
}

#[test]
fn new_rejects_below_the_lower_bound() {
    assert!(LightPwm::new(-0.1).is_err());
}

#[test]
fn new_rejects_nan() {
    assert!(LightPwm::new(f64::NAN).is_err());
}

#[test]
fn new_rejects_infinities() {
    assert!(LightPwm::new(f64::INFINITY).is_err());
    assert!(LightPwm::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn full_is_one_hundred_percent() {
    assert_eq!(LightPwm::FULL.percent(), 100.0);
    assert_eq!(LightPwm::FULL, LightPwm::new(100.0).unwrap());
}

#[test]
fn default_is_full_power() {
    assert_eq!(LightPwm::default(), LightPwm::FULL);
}

#[test]
fn duty_is_the_percentage_as_a_fraction() {
    assert_eq!(LightPwm::FULL.duty(), 1.0);
    assert_eq!(LightPwm::new(0.0).unwrap().duty(), 0.0);
    assert_eq!(LightPwm::new(50.0).unwrap().duty(), 0.5);
}

/// An exposure derived from a target energy density divides by the
/// duty, so half power must ask for twice the time.
#[test]
fn halving_the_duty_doubles_a_derived_exposure() {
    let full = 4.0 / LightPwm::FULL.duty();
    let half = 4.0 / LightPwm::new(50.0).unwrap().duty();
    assert_eq!(half, 2.0 * full);
}

#[test]
fn out_of_range_display_mentions_the_value_and_the_bounds() {
    let err = LightPwm::new(120.0).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("120"), "msg: {msg}");
    assert!(msg.contains("100"), "msg: {msg}");
    assert!(msg.contains('0'), "msg: {msg}");
}

#[test]
fn display_renders_a_percentage() {
    assert_eq!(format!("{}", LightPwm::new(62.5).unwrap()), "62.5%");
}

#[test]
fn try_from_f64_round_trips_to_f64() {
    let v: LightPwm = 25.0.try_into().unwrap();
    let back: f64 = v.into();
    assert_eq!(back, 25.0);
}

#[cfg(feature = "serde")]
#[test]
fn deserialises_an_in_range_percentage() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: LightPwm,
    }
    let parsed: Wrap = toml_edit::de::from_str("value = 80.0\n").unwrap();
    assert_eq!(parsed.value.percent(), 80.0);
}

#[cfg(feature = "serde")]
#[test]
fn deserialise_rejects_an_out_of_range_percentage() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: LightPwm,
    }
    assert!(toml_edit::de::from_str::<Wrap>("value = 101.0\n").is_err());
}

#[cfg(feature = "serde")]
#[test]
fn roundtrip_preserves_value() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: LightPwm,
    }
    let original = Wrap {
        value: LightPwm::new(37.5).unwrap(),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}
