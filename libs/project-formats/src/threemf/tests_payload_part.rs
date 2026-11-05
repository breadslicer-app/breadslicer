/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    FormatKind, MaterialId, NonEmptyString, OutputFormatBinding, PositiveFiniteMm, PrinterId,
};

use crate::payload::ProjectPayload;
use crate::threemf::payload_part::{PayloadParseOutcome, parse_payload, serialize_payload};

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).expect("test length is positive and finite")
}

fn goo_v3_0() -> OutputFormatBinding {
    OutputFormatBinding {
        format: FormatKind::Goo,
        version: Some(NonEmptyString::new("v3.0").expect("test version is non-empty")),
    }
}

const PAYLOAD_V1_FULL: &str = include_str!("fixtures/payload-v1-full.toml");
const PAYLOAD_V1_UNKNOWN_KEYS: &str = include_str!("fixtures/payload-v1-unknown-keys.toml");
const PAYLOAD_V1_PRINT_FORMAT: &str = include_str!("fixtures/payload-v1-print-format.toml");
const PAYLOAD_V2: &str = include_str!("fixtures/payload-v2.toml");

fn printer_id(raw: &str) -> PrinterId {
    PrinterId::new(raw).expect("test id is a valid slug")
}

fn material_id(raw: &str) -> MaterialId {
    MaterialId::new(raw).expect("test id is a valid slug")
}

mod serialize_tests {
    use super::*;

    #[test]
    fn full_references_round_trip() {
        let payload = ProjectPayload {
            printer: Some(printer_id("test-printer")),
            resin: Some(material_id("test-resin")),
            first_layer_height: None,
            output_format: None,
            embedded: Vec::new(),
            review_decisions: Vec::new(),
        };
        let text = serialize_payload(&payload, None).expect("references serialize");
        assert_eq!(
            parse_payload(&text).expect("serialized payload parses"),
            PayloadParseOutcome::V1 {
                printer: Some(printer_id("test-printer")),
                resin: Some(material_id("test-resin")),
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn first_layer_height_round_trips_through_the_print_section() {
        let payload = ProjectPayload {
            first_layer_height: Some(mm(0.05)),
            ..ProjectPayload::default()
        };
        let text = serialize_payload(&payload, None).expect("first-layer height serializes");
        assert!(text.contains("first_layer_height_mm"), "got:\n{text}");
        assert_eq!(
            parse_payload(&text).expect("serialized payload parses"),
            PayloadParseOutcome::V1 {
                printer: None,
                resin: None,
                first_layer_height: Some(mm(0.05)),
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn the_output_format_round_trips_through_the_print_section() {
        let payload = ProjectPayload {
            output_format: Some(goo_v3_0()),
            ..ProjectPayload::default()
        };
        let text = serialize_payload(&payload, None).expect("the output format serializes");
        assert!(
            text.contains(r#"output_format = { format = "goo", version = "v3.0" }"#),
            "got:\n{text}",
        );
        assert_eq!(
            parse_payload(&text).expect("serialized payload parses"),
            PayloadParseOutcome::V1 {
                printer: None,
                resin: None,
                first_layer_height: None,
                output_format: Some(goo_v3_0()),
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn an_absent_output_format_is_omitted_from_the_document() {
        let text =
            serialize_payload(&ProjectPayload::default(), None).expect("empty payload serializes");
        assert!(!text.contains("output_format"), "got:\n{text}");
    }

    #[test]
    fn absent_first_layer_height_is_omitted_from_the_document() {
        let text =
            serialize_payload(&ProjectPayload::default(), None).expect("empty payload serializes");
        assert!(!text.contains("first_layer_height"), "got:\n{text}");
    }

    #[test]
    fn absent_references_are_omitted_from_the_document() {
        let text =
            serialize_payload(&ProjectPayload::default(), None).expect("empty payload serializes");
        assert!(text.contains("version = 1"));
        assert!(!text.contains("printer"));
        assert!(!text.contains("resin"));
        assert_eq!(
            parse_payload(&text).expect("serialized payload parses"),
            PayloadParseOutcome::V1 {
                printer: None,
                resin: None,
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn embedded_documents_do_not_leak_into_the_project_part() {
        let payload = ProjectPayload {
            printer: Some(printer_id("test-printer")),
            resin: None,
            first_layer_height: None,
            output_format: None,
            embedded: vec![crate::payload::EmbeddedProfile::Printer {
                id: printer_id("test-printer"),
                bytes: b"marker-not-expected-in-part = true".to_vec(),
            }],
            review_decisions: Vec::new(),
        };
        let text = serialize_payload(&payload, None).expect("references serialize");
        assert!(!text.contains("marker-not-expected-in-part"));
    }

    #[test]
    fn a_slice_fingerprint_round_trips_through_serialization() {
        let text = serialize_payload(&ProjectPayload::default(), Some("cafef00d"))
            .expect("the token serializes");
        assert!(text.contains("cafef00d"), "got:\n{text}");
        assert_eq!(
            parse_payload(&text).expect("serialized payload parses"),
            PayloadParseOutcome::V1 {
                printer: None,
                resin: None,
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: Some("cafef00d".to_string()),
            },
        );
    }

    #[test]
    fn an_absent_slice_fingerprint_is_omitted_from_the_document() {
        let text =
            serialize_payload(&ProjectPayload::default(), None).expect("empty payload serializes");
        assert!(!text.contains("slices"), "got:\n{text}");
        assert!(!text.contains("fingerprint"), "got:\n{text}");
    }
}

mod parse_tests {
    use super::*;

    #[test]
    fn v1_fixture_parses_to_its_references() {
        assert_eq!(
            parse_payload(PAYLOAD_V1_FULL).expect("the v1 fixture parses"),
            PayloadParseOutcome::V1 {
                printer: Some(printer_id("test-printer")),
                resin: Some(material_id("test-resin")),
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn a_document_naming_a_print_format_parses_it_beside_the_other_print_settings() {
        assert_eq!(
            parse_payload(PAYLOAD_V1_PRINT_FORMAT).expect("the fixture parses"),
            PayloadParseOutcome::V1 {
                printer: Some(printer_id("test-printer")),
                resin: None,
                first_layer_height: Some(mm(0.05)),
                output_format: Some(goo_v3_0()),
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn unknown_keys_in_a_v1_document_are_ignored() {
        assert_eq!(
            parse_payload(PAYLOAD_V1_UNKNOWN_KEYS).expect("unknown keys do not fail the parse"),
            PayloadParseOutcome::V1 {
                printer: Some(printer_id("test-printer")),
                resin: Some(material_id("test-resin")),
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn a_version_this_build_does_not_know_is_reported_not_parsed() {
        assert_eq!(
            parse_payload(PAYLOAD_V2).expect("the version probe succeeds"),
            PayloadParseOutcome::UnsupportedVersion { version: 2 },
        );
    }

    #[test]
    fn version_only_documents_parse_with_no_references() {
        assert_eq!(
            parse_payload("version = 1").expect("a version-only document parses"),
            PayloadParseOutcome::V1 {
                printer: None,
                resin: None,
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn negative_versions_are_unsupported_not_errors() {
        assert_eq!(
            parse_payload("version = -3").expect("the version probe succeeds"),
            PayloadParseOutcome::UnsupportedVersion { version: -3 },
        );
    }

    #[test]
    fn a_non_integer_version_is_an_error() {
        assert!(parse_payload("version = 1.5").is_err());
    }

    #[test]
    fn a_missing_version_is_an_error() {
        assert!(parse_payload("[profiles]\nprinter = \"test-printer\"").is_err());
    }

    #[test]
    fn an_invalid_id_slug_is_an_error() {
        assert!(parse_payload("version = 1\n[profiles]\nprinter = \"Bad Id\"").is_err());
    }

    #[test]
    fn non_toml_content_is_an_error() {
        assert!(parse_payload("=").is_err());
    }
}
