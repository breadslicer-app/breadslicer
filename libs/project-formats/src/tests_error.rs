/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::error::Error;

use sc_3mf::error::MalformedPartNameError;

use crate::error::{
    ReadProjectError, ReadThreeMfProjectError, WriteProjectError, WriteThreeMfProjectError,
};

fn toml_parse_error() -> toml_edit::de::Error {
    #[derive(Debug, serde::Deserialize)]
    struct Probe {
        #[expect(dead_code, reason = "only the failed parse matters")]
        version: i64,
    }
    toml_edit::de::from_str::<Probe>("=").expect_err("the document is not TOML")
}

mod write_error_tests {
    use super::*;

    #[test]
    fn io_display_and_source() {
        let err = WriteThreeMfProjectError::from(std::io::Error::other("boom"));
        assert!(matches!(err, WriteThreeMfProjectError::Io(_)));
        assert_eq!(err.to_string(), "3MF project writer I/O error: boom");
        assert!(err.source().is_some());
    }

    #[test]
    fn payload_display_names_the_project_part() {
        let ser_err = toml_edit::ser::to_string(&vec![1, 2])
            .expect_err("a TOML document must be a table, not an array");
        let err = WriteThreeMfProjectError::from(ser_err);
        assert!(matches!(err, WriteThreeMfProjectError::Payload(_)));
        assert!(
            err.to_string()
                .starts_with("3MF project writer: failed to serialize the project part:"),
        );
    }

    #[test]
    fn malformed_part_name_display_and_source() {
        let err = WriteThreeMfProjectError::MalformedPartName {
            name: "/Metadata//p.toml".to_string(),
            source: MalformedPartNameError::EmptySegment,
        };
        assert_eq!(
            err.to_string(),
            "3MF project writer: \"/Metadata//p.toml\" is not a valid part name: part name \
             contains an empty segment",
        );
        assert_eq!(
            err.source().map(ToString::to_string),
            Some("part name contains an empty segment".to_string()),
        );
    }

    #[test]
    fn dispatch_wrapper_passes_display_and_source_through() {
        let err =
            WriteProjectError::ThreeMf(WriteThreeMfProjectError::Io(std::io::Error::other("boom")));
        assert_eq!(err.to_string(), "3MF project writer I/O error: boom");
        assert!(err.source().is_some());
    }
}

mod read_error_tests {
    use super::*;

    #[test]
    #[expect(invalid_from_utf8, reason = "the test needs a real Utf8Error value")]
    fn not_utf8_display_and_source() {
        let utf8_err = std::str::from_utf8(&[0xff]).expect_err("0xff is not UTF-8");
        let err = ReadThreeMfProjectError::PayloadNotUtf8(utf8_err);
        assert!(
            err.to_string()
                .starts_with("3MF project reader: the project part is not UTF-8:"),
        );
        assert!(err.source().is_some());
    }

    #[test]
    fn payload_parse_display_and_source() {
        let err = ReadThreeMfProjectError::PayloadParse(toml_parse_error());
        assert!(
            err.to_string()
                .starts_with("3MF project reader: failed to parse the project part:"),
        );
        assert!(err.source().is_some());
    }

    #[test]
    fn dispatch_wrapper_passes_display_and_source_through() {
        let err =
            ReadProjectError::ThreeMf(ReadThreeMfProjectError::PayloadParse(toml_parse_error()));
        assert!(err.to_string().starts_with("3MF project reader:"));
        assert!(err.source().is_some());
    }
}
