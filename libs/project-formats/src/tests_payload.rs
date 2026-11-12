/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{MaterialId, PrinterId};

use crate::messages::ProfileRefWarning;
use crate::payload::{EmbeddedProfile, ProjectPayload};

fn printer_id(raw: &str) -> PrinterId {
    PrinterId::new(raw).expect("test id is a valid slug")
}

fn material_id(raw: &str) -> MaterialId {
    MaterialId::new(raw).expect("test id is a valid slug")
}

fn payload_with_refs() -> ProjectPayload {
    ProjectPayload {
        printer: Some(printer_id("test-printer")),
        resin: Some(material_id("test-resin")),
        first_layer_height: None,
        output_format: None,
        embedded: Vec::new(),
        review_decisions: Vec::new(),
        plates: 0,
        plate_names: Vec::new(),
    }
}

mod embeds_tests {
    use super::*;

    #[test]
    fn embedded_printer_is_found_by_id() {
        let payload = ProjectPayload {
            embedded: vec![EmbeddedProfile::Printer {
                id: printer_id("test-printer"),
                bytes: b"a = 1".to_vec(),
            }],
            ..ProjectPayload::default()
        };
        assert!(payload.embeds_printer(&printer_id("test-printer")));
        assert!(!payload.embeds_printer(&printer_id("other-printer")));
    }

    #[test]
    fn embedded_material_is_found_by_id() {
        let payload = ProjectPayload {
            embedded: vec![EmbeddedProfile::Material {
                id: material_id("test-resin"),
                bytes: b"a = 1".to_vec(),
            }],
            ..ProjectPayload::default()
        };
        assert!(payload.embeds_material(&material_id("test-resin")));
        assert!(!payload.embeds_material(&material_id("other-resin")));
    }

    #[test]
    fn material_extension_does_not_count_as_material() {
        let payload = ProjectPayload {
            embedded: vec![EmbeddedProfile::MaterialExtension {
                id: material_id("test-resin"),
                bytes: b"a = 1".to_vec(),
            }],
            ..ProjectPayload::default()
        };
        assert!(!payload.embeds_material(&material_id("test-resin")));
    }

    #[test]
    fn printer_and_material_ids_do_not_cross_match() {
        let payload = ProjectPayload {
            embedded: vec![EmbeddedProfile::Printer {
                id: printer_id("shared-slug"),
                bytes: b"a = 1".to_vec(),
            }],
            ..ProjectPayload::default()
        };
        assert!(!payload.embeds_material(&material_id("shared-slug")));
    }
}

mod profile_ref_warnings_tests {
    use super::*;

    #[test]
    fn no_references_produce_no_warnings() {
        let payload = ProjectPayload::default();
        assert!(
            payload
                .profile_ref_warnings(|_| false, |_| false)
                .is_empty()
        );
    }

    #[test]
    fn installed_references_produce_no_warnings() {
        let payload = payload_with_refs();
        assert!(payload.profile_ref_warnings(|_| true, |_| true).is_empty());
    }

    #[test]
    fn missing_references_without_embedded_copies_use_the_missing_tier() {
        let payload = payload_with_refs();
        let warnings = payload.profile_ref_warnings(|_| false, |_| false);
        assert_eq!(
            warnings,
            vec![
                ProfileRefWarning::PrinterMissing {
                    id: printer_id("test-printer"),
                },
                ProfileRefWarning::ResinMissing {
                    id: material_id("test-resin"),
                },
            ],
        );
    }

    #[test]
    fn missing_references_with_embedded_copies_use_the_softer_tier() {
        let mut payload = payload_with_refs();
        payload.embedded = vec![
            EmbeddedProfile::Printer {
                id: printer_id("test-printer"),
                bytes: b"a = 1".to_vec(),
            },
            EmbeddedProfile::Material {
                id: material_id("test-resin"),
                bytes: b"b = 2".to_vec(),
            },
        ];
        let warnings = payload.profile_ref_warnings(|_| false, |_| false);
        assert_eq!(
            warnings,
            vec![
                ProfileRefWarning::PrinterEmbeddedOnly {
                    id: printer_id("test-printer"),
                },
                ProfileRefWarning::ResinEmbeddedOnly {
                    id: material_id("test-resin"),
                },
            ],
        );
    }

    #[test]
    fn an_embedded_extension_alone_does_not_soften_the_resin_warning() {
        let mut payload = payload_with_refs();
        payload.printer = None;
        payload.embedded = vec![EmbeddedProfile::MaterialExtension {
            id: material_id("test-resin"),
            bytes: b"a = 1".to_vec(),
        }];
        let warnings = payload.profile_ref_warnings(|_| false, |_| false);
        assert_eq!(
            warnings,
            vec![ProfileRefWarning::ResinMissing {
                id: material_id("test-resin"),
            }],
        );
    }

    #[test]
    fn tiers_are_decided_per_reference() {
        let mut payload = payload_with_refs();
        payload.embedded = vec![EmbeddedProfile::Printer {
            id: printer_id("test-printer"),
            bytes: b"a = 1".to_vec(),
        }];
        let warnings = payload.profile_ref_warnings(|_| false, |_| false);
        assert_eq!(
            warnings,
            vec![
                ProfileRefWarning::PrinterEmbeddedOnly {
                    id: printer_id("test-printer"),
                },
                ProfileRefWarning::ResinMissing {
                    id: material_id("test-resin"),
                },
            ],
        );
    }

    #[test]
    fn the_lookups_receive_the_referenced_ids() {
        let payload = payload_with_refs();
        let warnings = payload.profile_ref_warnings(
            |id| id == &printer_id("test-printer"),
            |id| id == &material_id("test-resin"),
        );
        assert!(warnings.is_empty());
    }
}
