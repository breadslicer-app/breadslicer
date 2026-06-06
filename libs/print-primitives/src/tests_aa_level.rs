/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::str::FromStr;

use super::aa_level::{AaLevel, UnknownAaLevel};

#[test]
fn aa_level_from_str_parses_canonical_names() {
    assert_eq!(AaLevel::from_str("off"), Ok(AaLevel::Off));
    assert_eq!(AaLevel::from_str("x2"), Ok(AaLevel::X2));
    assert_eq!(AaLevel::from_str("x4"), Ok(AaLevel::X4));
    assert_eq!(AaLevel::from_str("x8"), Ok(AaLevel::X8));
}

#[test]
fn aa_level_from_str_rejects_numeric_shorthand() {
    for s in ["0", "2", "4", "8"] {
        let err = AaLevel::from_str(s).unwrap_err();
        assert_eq!(err, UnknownAaLevel { got: s.to_owned() });
    }
}

#[test]
fn aa_level_from_str_rejects_uppercase() {
    for s in ["Off", "X2", "X4", "X8", "OFF"] {
        let err = AaLevel::from_str(s).unwrap_err();
        assert_eq!(err, UnknownAaLevel { got: s.to_owned() });
    }
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

#[cfg(feature = "serde")]
#[test]
fn aa_level_deserialises_canonical_string_for_each_variant() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        value: AaLevel,
    }
    let cases = [
        ("off", AaLevel::Off),
        ("x2", AaLevel::X2),
        ("x4", AaLevel::X4),
        ("x8", AaLevel::X8),
    ];
    for (s, expected) in cases {
        let toml = format!("value = \"{s}\"\n");
        let parsed: Wrap = toml_edit::de::from_str(&toml).unwrap();
        assert_eq!(parsed.value, expected);
    }
}

#[cfg(feature = "serde")]
#[test]
fn aa_level_deserialise_rejects_numeric_shorthand() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: AaLevel,
    }
    let toml = "value = \"4\"\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn aa_level_deserialise_rejects_uppercase() {
    #[derive(serde::Deserialize)]
    struct Wrap {
        #[allow(dead_code)]
        value: AaLevel,
    }
    let toml = "value = \"Off\"\n";
    assert!(toml_edit::de::from_str::<Wrap>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn aa_level_serialises_each_variant_as_canonical_lowercase() {
    #[derive(serde::Serialize)]
    struct Wrap {
        value: AaLevel,
    }
    let cases = [
        (AaLevel::Off, "off"),
        (AaLevel::X2, "x2"),
        (AaLevel::X4, "x4"),
        (AaLevel::X8, "x8"),
    ];
    for (variant, expected) in cases {
        let toml = toml_edit::ser::to_string(&Wrap { value: variant }).unwrap();
        assert!(toml.contains(expected), "toml: {toml}");
    }
}

#[cfg(feature = "serde")]
#[test]
fn aa_level_roundtrip_through_toml_preserves_each_variant() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Wrap {
        value: AaLevel,
    }
    for variant in [AaLevel::Off, AaLevel::X2, AaLevel::X4, AaLevel::X8] {
        let original = Wrap { value: variant };
        let serialised = toml_edit::ser::to_string(&original).unwrap();
        let roundtripped: Wrap = toml_edit::de::from_str(&serialised).unwrap();
        assert_eq!(original, roundtripped);
    }
}
