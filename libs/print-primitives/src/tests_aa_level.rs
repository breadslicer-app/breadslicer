/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::str::FromStr;

use super::aa_level::{AaLevel, UnknownAaLevel};

#[test]
fn aa_level_from_str_parses_numeric_shorthand() {
    assert_eq!(AaLevel::from_str("0"), Ok(AaLevel::Off));
    assert_eq!(AaLevel::from_str("2"), Ok(AaLevel::X2));
    assert_eq!(AaLevel::from_str("4"), Ok(AaLevel::X4));
    assert_eq!(AaLevel::from_str("8"), Ok(AaLevel::X8));
}

#[test]
fn aa_level_from_str_parses_canonical_names() {
    assert_eq!(AaLevel::from_str("off"), Ok(AaLevel::Off));
    assert_eq!(AaLevel::from_str("x2"), Ok(AaLevel::X2));
    assert_eq!(AaLevel::from_str("x4"), Ok(AaLevel::X4));
    assert_eq!(AaLevel::from_str("x8"), Ok(AaLevel::X8));
}

#[test]
fn aa_level_from_str_is_case_insensitive() {
    assert_eq!(AaLevel::from_str("Off"), Ok(AaLevel::Off));
    assert_eq!(AaLevel::from_str("X4"), Ok(AaLevel::X4));
}

#[test]
fn aa_level_from_str_rejects_unknown_name() {
    let err = AaLevel::from_str("3").unwrap_err();
    assert_eq!(
        err,
        UnknownAaLevel {
            got: "3".to_owned(),
        }
    );
    let msg = format!("{err}");
    assert!(msg.contains("3"), "msg: {msg}");
    assert!(msg.contains("expected"), "msg: {msg}");
}
