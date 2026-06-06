/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::identity::{InvalidProfileId, MaterialId, PrinterId};

#[test]
fn printer_id_new_accepts_single_lowercase_letter() {
    let id = PrinterId::new("a").unwrap();
    assert_eq!(id.as_str(), "a");
}

#[test]
fn printer_id_new_accepts_single_digit() {
    let id = PrinterId::new("3").unwrap();
    assert_eq!(id.as_str(), "3");
}

#[test]
fn printer_id_new_accepts_alphanumerics_and_hyphens() {
    let id = PrinterId::new("elegoo-mars-3-pro").unwrap();
    assert_eq!(id.as_str(), "elegoo-mars-3-pro");
}

#[test]
fn printer_id_new_accepts_trailing_hyphen() {
    let id = PrinterId::new("foo-").unwrap();
    assert_eq!(id.as_str(), "foo-");
}

#[test]
fn printer_id_new_accepts_consecutive_hyphens() {
    let id = PrinterId::new("a--b").unwrap();
    assert_eq!(id.as_str(), "a--b");
}

#[test]
fn printer_id_new_rejects_empty_string() {
    let err = PrinterId::new("").unwrap_err();
    assert_eq!(err, InvalidProfileId::Empty);
}

#[test]
fn printer_id_new_rejects_leading_hyphen() {
    let err = PrinterId::new("-elegoo").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::LeadingHyphen {
            value: "-elegoo".to_owned(),
        }
    );
}

#[test]
fn printer_id_new_rejects_uppercase_letter_at_start() {
    let err = PrinterId::new("Elegoo").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::InvalidCharAt {
            value: "Elegoo".to_owned(),
            index: 0,
        }
    );
}

#[test]
fn printer_id_new_rejects_uppercase_letter_in_middle() {
    let err = PrinterId::new("elegooMars").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::InvalidCharAt {
            value: "elegooMars".to_owned(),
            index: 6,
        }
    );
}

#[test]
fn printer_id_new_rejects_underscore() {
    let err = PrinterId::new("elegoo_mars").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::InvalidCharAt {
            value: "elegoo_mars".to_owned(),
            index: 6,
        }
    );
}

#[test]
fn printer_id_new_rejects_space() {
    let err = PrinterId::new("elegoo mars").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::InvalidCharAt {
            value: "elegoo mars".to_owned(),
            index: 6,
        }
    );
}

#[test]
fn printer_id_new_rejects_dot() {
    let err = PrinterId::new("v1.0").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::InvalidCharAt {
            value: "v1.0".to_owned(),
            index: 2,
        }
    );
}

#[test]
fn printer_id_new_rejects_slash() {
    let err = PrinterId::new("foo/bar").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::InvalidCharAt {
            value: "foo/bar".to_owned(),
            index: 3,
        }
    );
}

#[test]
fn printer_id_new_rejects_non_ascii_at_first_offending_byte() {
    let err = PrinterId::new("caf\u{00e9}").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::InvalidCharAt {
            value: "caf\u{00e9}".to_owned(),
            index: 3,
        }
    );
}

#[test]
fn printer_id_display_renders_inner_slug() {
    let id = PrinterId::new("elegoo-mars-3-pro").unwrap();
    assert_eq!(format!("{id}"), "elegoo-mars-3-pro");
}

#[test]
fn printer_id_try_from_string_accepts_valid_slug() {
    let id = PrinterId::try_from("elegoo-mars-3".to_owned()).unwrap();
    assert_eq!(id.as_str(), "elegoo-mars-3");
}

#[test]
fn printer_id_try_from_string_rejects_invalid_slug() {
    let err = PrinterId::try_from("-bad".to_owned()).unwrap_err();
    assert!(matches!(err, InvalidProfileId::LeadingHyphen { .. }));
}

#[test]
fn printer_id_from_str_accepts_valid_slug() {
    let id: PrinterId = "elegoo-mars-3".parse().unwrap();
    assert_eq!(id.as_str(), "elegoo-mars-3");
}

#[test]
fn printer_id_from_str_rejects_invalid_slug() {
    let err = "Elegoo".parse::<PrinterId>().unwrap_err();
    assert!(matches!(err, InvalidProfileId::InvalidCharAt { .. }));
}

#[test]
fn printer_id_into_string_returns_inner_slug() {
    let id = PrinterId::new("foo").unwrap();
    let s: String = id.into();
    assert_eq!(s, "foo");
}

#[test]
fn material_id_new_accepts_alphanumerics_and_hyphens() {
    let id = MaterialId::new("elegoo-water-washable-grey").unwrap();
    assert_eq!(id.as_str(), "elegoo-water-washable-grey");
}

#[test]
fn material_id_new_rejects_empty_string() {
    let err = MaterialId::new("").unwrap_err();
    assert_eq!(err, InvalidProfileId::Empty);
}

#[test]
fn material_id_new_rejects_invalid_char() {
    let err = MaterialId::new("foo.bar").unwrap_err();
    assert_eq!(
        err,
        InvalidProfileId::InvalidCharAt {
            value: "foo.bar".to_owned(),
            index: 3,
        }
    );
}

#[test]
fn material_id_display_renders_inner_slug() {
    let id = MaterialId::new("anycubic-tough-resin").unwrap();
    assert_eq!(format!("{id}"), "anycubic-tough-resin");
}

#[test]
fn material_id_from_str_accepts_valid_slug() {
    let id: MaterialId = "anycubic-tough-resin".parse().unwrap();
    assert_eq!(id.as_str(), "anycubic-tough-resin");
}

#[test]
fn invalid_profile_id_display_for_empty_mentions_empty() {
    let msg = format!("{}", InvalidProfileId::Empty);
    assert!(msg.contains("empty"), "msg: {msg}");
}

#[test]
fn invalid_profile_id_display_for_leading_hyphen_includes_value_and_hyphen() {
    let msg = format!(
        "{}",
        InvalidProfileId::LeadingHyphen {
            value: "-x".to_owned(),
        }
    );
    assert!(msg.contains("hyphen"), "msg: {msg}");
    assert!(msg.contains("-x"), "msg: {msg}");
}

#[test]
fn invalid_profile_id_display_for_invalid_char_includes_value_and_index() {
    let msg = format!(
        "{}",
        InvalidProfileId::InvalidCharAt {
            value: "Foo".to_owned(),
            index: 0,
        }
    );
    assert!(msg.contains("Foo"), "msg: {msg}");
    assert!(msg.contains("byte 0"), "msg: {msg}");
}
