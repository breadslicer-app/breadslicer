/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::non_empty_string::{EmptyStringError, NonEmptyString};

#[test]
fn non_empty_string_new_accepts_single_byte() {
    let s = NonEmptyString::new("a").unwrap();
    assert_eq!(s.as_str(), "a");
}

#[test]
fn non_empty_string_new_accepts_multibyte_utf8() {
    let s = NonEmptyString::new("caf\u{00e9}").unwrap();
    assert_eq!(s.as_str(), "caf\u{00e9}");
}

#[test]
fn non_empty_string_new_accepts_whitespace_only() {
    let s = NonEmptyString::new(" ").unwrap();
    assert_eq!(s.as_str(), " ");
}

#[test]
fn non_empty_string_new_rejects_empty_string() {
    let err = NonEmptyString::new("").unwrap_err();
    assert_eq!(err, EmptyStringError);
}

#[test]
fn non_empty_string_try_from_accepts_non_empty_string() {
    let s = NonEmptyString::try_from("Elegoo".to_owned()).unwrap();
    assert_eq!(s.as_str(), "Elegoo");
}

#[test]
fn non_empty_string_try_from_rejects_empty_string() {
    let err = NonEmptyString::try_from(String::new()).unwrap_err();
    assert_eq!(err, EmptyStringError);
}

#[test]
fn non_empty_string_from_str_accepts_non_empty_string() {
    let s: NonEmptyString = "Anycubic".parse().unwrap();
    assert_eq!(s.as_str(), "Anycubic");
}

#[test]
fn non_empty_string_from_str_rejects_empty_string() {
    let err = "".parse::<NonEmptyString>().unwrap_err();
    assert_eq!(err, EmptyStringError);
}

#[test]
fn non_empty_string_display_renders_inner_value() {
    let s = NonEmptyString::new("Elegoo Mars 3 Pro").unwrap();
    assert_eq!(format!("{s}"), "Elegoo Mars 3 Pro");
}

#[test]
fn non_empty_string_into_string_returns_inner_value() {
    let s = NonEmptyString::new("Elegoo").unwrap();
    let raw: String = s.into();
    assert_eq!(raw, "Elegoo");
}

#[test]
fn empty_string_error_display_mentions_empty() {
    let msg = format!("{}", EmptyStringError);
    assert!(msg.contains("empty"), "msg: {msg}");
}
