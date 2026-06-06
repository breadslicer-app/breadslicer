/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::format::{FormatKind, OutputFormatBinding, OutputFormatBindings};
use crate::non_empty_string::NonEmptyString;

#[test]
fn format_kind_canonical_roundtrips_for_known_variants() {
    let cases = [
        ("sl1", FormatKind::Sl1),
        ("ctb", FormatKind::Ctb),
        ("goo", FormatKind::Goo),
        ("pwmx", FormatKind::Pwmx),
        ("png_per_layer", FormatKind::PngPerLayer),
        ("svg_per_layer", FormatKind::SvgPerLayer),
        ("svg_combined", FormatKind::SvgCombined),
    ];
    for (s, expected) in cases {
        let parsed = FormatKind::from_canonical(s);
        assert_eq!(parsed, expected);
        assert_eq!(parsed.as_canonical(), s);
    }
}

#[test]
fn format_kind_from_canonical_falls_through_to_unknown() {
    let parsed = FormatKind::from_canonical("future_format");
    assert_eq!(parsed, FormatKind::Unknown("future_format".to_owned()));
}

#[test]
fn format_kind_unknown_preserves_inner_string_in_as_canonical() {
    let kind = FormatKind::Unknown("future_format".to_owned());
    assert_eq!(kind.as_canonical(), "future_format");
}

#[test]
fn output_format_bindings_alias_holds_a_non_empty_list() {
    let bindings: OutputFormatBindings = OutputFormatBindings::new(OutputFormatBinding {
        format: FormatKind::Sl1,
        version: None,
    });
    assert_eq!(bindings.len().get(), 1);
    assert_eq!(bindings.first().format, FormatKind::Sl1);
}

#[cfg(feature = "serde")]
#[test]
fn output_format_binding_deserialises_ctb_with_integer_string_version() {
    let toml = "format = \"ctb\"\nversion = \"4\"\n";
    let parsed: OutputFormatBinding = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.format, FormatKind::Ctb);
    assert_eq!(parsed.version, Some(NonEmptyString::new("4").unwrap()));
}

#[cfg(feature = "serde")]
#[test]
fn output_format_binding_deserialises_goo_with_fixed_string_version() {
    let toml = "format = \"goo\"\nversion = \"V3.0\"\n";
    let parsed: OutputFormatBinding = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.format, FormatKind::Goo);
    assert_eq!(parsed.version, Some(NonEmptyString::new("V3.0").unwrap()));
}

#[cfg(feature = "serde")]
#[test]
fn output_format_binding_deserialises_unknown_kind_via_escape_hatch() {
    let toml = "format = \"future_format\"\nversion = \"1\"\n";
    let parsed: OutputFormatBinding = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(
        parsed.format,
        FormatKind::Unknown("future_format".to_owned())
    );
}

#[cfg(feature = "serde")]
#[test]
fn output_format_binding_deserialises_without_version_when_absent() {
    let toml = "format = \"png_per_layer\"\n";
    let parsed: OutputFormatBinding = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(parsed.format, FormatKind::PngPerLayer);
    assert_eq!(parsed.version, None);
}

#[cfg(feature = "serde")]
#[test]
fn output_format_binding_deserialise_rejects_empty_version_string() {
    let toml = "format = \"ctb\"\nversion = \"\"\n";
    assert!(toml_edit::de::from_str::<OutputFormatBinding>(toml).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn output_format_binding_roundtrip_with_version_preserves_value() {
    let original = OutputFormatBinding {
        format: FormatKind::Goo,
        version: Some(NonEmptyString::new("V3.0").unwrap()),
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: OutputFormatBinding = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[cfg(feature = "serde")]
#[test]
fn output_format_binding_roundtrip_without_version_preserves_value() {
    let original = OutputFormatBinding {
        format: FormatKind::PngPerLayer,
        version: None,
    };
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: OutputFormatBinding = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
    assert_eq!(roundtripped.version, None);
}

#[cfg(feature = "serde")]
#[test]
fn output_format_binding_serialise_omits_version_when_none() {
    let binding = OutputFormatBinding {
        format: FormatKind::PngPerLayer,
        version: None,
    };
    let serialised = toml_edit::ser::to_string(&binding).unwrap();
    assert!(!serialised.contains("version"), "serialised: {serialised}");
}
