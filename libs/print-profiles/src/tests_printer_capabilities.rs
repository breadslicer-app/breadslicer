/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::printer_capabilities::PrinterCapabilities;
use nonempty_collections::NEVec;
use sc_print_primitives::{
    AaLevel, FormatKind, NetworkProtocol, NonEmptyString, OutputFormatBinding,
};

const FULL_TOML: &str = r#"
output_formats = [
    { format = "sl1", version = "2" },
    { format = "ctb", version = "4" },
]
anti_aliasing = "x4"
two_stage_lift = true
transition_layers = 5
has_camera = false
network_protocol = "elegoo_centauri"
"#;

const MINIMAL_TOML: &str = r#"
output_formats = [{ format = "png_per_layer" }]
anti_aliasing = "off"
two_stage_lift = false
has_camera = false
"#;

#[test]
fn deserialise_parses_all_fields() {
    let parsed: PrinterCapabilities = toml_edit::de::from_str(FULL_TOML).unwrap();
    assert_eq!(parsed.output_formats.len().get(), 2);
    assert_eq!(parsed.output_formats.first().format, FormatKind::Sl1);
    assert_eq!(
        parsed.output_formats.first().version,
        Some(NonEmptyString::new("2").unwrap())
    );
    assert_eq!(parsed.anti_aliasing, AaLevel::X4);
    assert!(parsed.two_stage_lift);
    assert_eq!(parsed.transition_layers, Some(5));
    assert!(!parsed.has_camera);
    assert_eq!(
        parsed.network_protocol,
        Some(NetworkProtocol::ElegooCentauri)
    );
}

#[test]
fn deserialise_minimal_required_fields_leaves_optionals_as_none() {
    let parsed: PrinterCapabilities = toml_edit::de::from_str(MINIMAL_TOML).unwrap();
    assert_eq!(parsed.output_formats.len().get(), 1);
    assert_eq!(
        parsed.output_formats.first().format,
        FormatKind::PngPerLayer
    );
    assert_eq!(parsed.output_formats.first().version, None);
    assert_eq!(parsed.anti_aliasing, AaLevel::Off);
    assert!(!parsed.two_stage_lift);
    assert_eq!(parsed.transition_layers, None);
    assert!(!parsed.has_camera);
    assert_eq!(parsed.network_protocol, None);
}

#[test]
fn deserialise_rejects_empty_output_formats_array() {
    let toml = r#"
output_formats = []
anti_aliasing = "off"
two_stage_lift = false
has_camera = false
"#;
    assert!(toml_edit::de::from_str::<PrinterCapabilities>(toml).is_err());
}

#[test]
fn deserialise_accepts_unknown_format_kind_via_escape_hatch() {
    let toml = r#"
output_formats = [{ format = "future_format", version = "1" }]
anti_aliasing = "off"
two_stage_lift = false
has_camera = false
"#;
    let parsed: PrinterCapabilities = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(
        parsed.output_formats.first().format,
        FormatKind::Unknown("future_format".to_owned())
    );
}

#[test]
fn deserialise_accepts_unknown_network_protocol_via_escape_hatch() {
    let toml = r#"
output_formats = [{ format = "sl1" }]
anti_aliasing = "off"
two_stage_lift = false
has_camera = false
network_protocol = "future_protocol"
"#;
    let parsed: PrinterCapabilities = toml_edit::de::from_str(toml).unwrap();
    assert_eq!(
        parsed.network_protocol,
        Some(NetworkProtocol::Unknown("future_protocol".to_owned()))
    );
}

#[test]
fn deserialise_rejects_invalid_anti_aliasing_value() {
    let toml = r#"
output_formats = [{ format = "sl1" }]
anti_aliasing = "x16"
two_stage_lift = false
has_camera = false
"#;
    assert!(toml_edit::de::from_str::<PrinterCapabilities>(toml).is_err());
}

#[test]
fn deserialise_rejects_missing_required_field() {
    let toml = r#"
output_formats = [{ format = "sl1" }]
anti_aliasing = "off"
two_stage_lift = false
"#;
    assert!(toml_edit::de::from_str::<PrinterCapabilities>(toml).is_err());
}

#[test]
fn roundtrip_with_all_fields_preserves_value() {
    let original: PrinterCapabilities = toml_edit::de::from_str(FULL_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterCapabilities = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
}

#[test]
fn roundtrip_minimal_preserves_value() {
    let original: PrinterCapabilities = toml_edit::de::from_str(MINIMAL_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&original).unwrap();
    let roundtripped: PrinterCapabilities = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(original, roundtripped);
    assert_eq!(roundtripped.transition_layers, None);
    assert_eq!(roundtripped.network_protocol, None);
}

#[test]
fn serialise_omits_optionals_when_none() {
    let parsed: PrinterCapabilities = toml_edit::de::from_str(MINIMAL_TOML).unwrap();
    let serialised = toml_edit::ser::to_string(&parsed).unwrap();
    assert!(
        !serialised.contains("transition_layers"),
        "should omit transition_layers: {serialised}"
    );
    assert!(
        !serialised.contains("network_protocol"),
        "should omit network_protocol: {serialised}"
    );
}

#[test]
fn programmatic_construction_with_nevec_builds_a_valid_value() {
    let bindings: NEVec<OutputFormatBinding> = NEVec::new(OutputFormatBinding {
        format: FormatKind::Sl1,
        version: Some(NonEmptyString::new("2").unwrap()),
    });
    let caps = PrinterCapabilities {
        output_formats: bindings,
        anti_aliasing: AaLevel::X2,
        two_stage_lift: false,
        transition_layers: None,
        has_camera: true,
        network_protocol: Some(NetworkProtocol::Moonraker),
    };
    let serialised = toml_edit::ser::to_string(&caps).unwrap();
    let roundtripped: PrinterCapabilities = toml_edit::de::from_str(&serialised).unwrap();
    assert_eq!(caps, roundtripped);
}
