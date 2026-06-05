/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::length::{NegativeLengthError, ParseLengthError, PositiveFiniteMm};

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
fn negative_length_error_display_includes_value() {
    let msg = format!("{}", NegativeLengthError { value: -0.5 });
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
