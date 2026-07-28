/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io::Cursor;

use sc_3mf::names::{MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_print_primitives::{MaterialId, PrinterId};

use crate::error::ReadThreeMfProjectError;
use crate::messages::{ForeignDialect, ReadProjectWarning};
use crate::payload::EmbeddedProfile;
use crate::read::LoadedProject;
use crate::threemf::reader::read_threemf_project;
use crate::threemf::{PROFILE_PART_CONTENT_TYPE, PROJECT_PART_CONTENT_TYPE, PROJECT_PART_NAME};

const TETRAHEDRON_MODEL: &str = include_str!("fixtures/tetrahedron.model");
const PAYLOAD_V1_FULL: &str = include_str!("fixtures/payload-v1-full.toml");
const PAYLOAD_V2: &str = include_str!("fixtures/payload-v2.toml");

fn printer_id(raw: &str) -> PrinterId {
    PrinterId::new(raw).expect("test id is a valid slug")
}

fn material_id(raw: &str) -> MaterialId {
    MaterialId::new(raw).expect("test id is a valid slug")
}

fn part(name: &str) -> PartName {
    PartName::new(name).expect("test part name is valid")
}

/// A package holding just the tetrahedron model part and its
/// start-part relationship.
fn model_only_package() -> Package {
    let mut package = Package::new();
    package
        .add_part(
            part("/3D/3dmodel.model"),
            MODEL_PART_CONTENT_TYPE,
            TETRAHEDRON_MODEL.as_bytes().to_vec(),
        )
        .expect("the model part is added");
    package.add_root_relationship(Relationship {
        id: "rel-1".to_string(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: "/3D/3dmodel.model".to_string(),
    });
    package
}

fn add_project_part(package: &mut Package, bytes: &[u8]) {
    package
        .add_part(
            part(PROJECT_PART_NAME),
            PROJECT_PART_CONTENT_TYPE,
            bytes.to_vec(),
        )
        .expect("the project part is added");
}

fn add_profile_part(package: &mut Package, name: &str, bytes: &[u8]) {
    package
        .add_part(part(name), PROFILE_PART_CONTENT_TYPE, bytes.to_vec())
        .expect("the profile part is added");
}

fn read(package: &Package) -> Result<LoadedProject, ReadThreeMfProjectError> {
    let mut buffer = Cursor::new(Vec::new());
    package
        .write_to(&mut buffer)
        .expect("the test package serializes");
    buffer.set_position(0);
    read_threemf_project(&mut buffer)
}

mod plain_model_tests {
    use super::*;

    #[test]
    fn a_package_without_the_project_part_is_a_plain_model() {
        let loaded = read(&model_only_package()).expect("the plain model loads");
        assert!(loaded.payload.is_none());
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.scene.roots().len(), 1);
    }
}

mod payload_tests {
    use super::*;

    #[test]
    fn references_and_embedded_documents_are_extracted() {
        let mut package = model_only_package();
        add_project_part(&mut package, PAYLOAD_V1_FULL.as_bytes());
        add_profile_part(
            &mut package,
            "/Metadata/profiles/printers/test-printer.toml",
            b"kind = \"printer\"",
        );
        add_profile_part(
            &mut package,
            "/Metadata/profiles/materials/test-resin.toml",
            b"kind = \"material\"",
        );
        add_profile_part(
            &mut package,
            "/Metadata/profiles/materials/extensions/test-resin.toml",
            b"kind = \"extension\"",
        );

        let loaded = read(&package).expect("the project loads");
        assert!(loaded.warnings.is_empty());
        let payload = loaded.payload.expect("the payload is present");
        assert_eq!(payload.printer, Some(printer_id("test-printer")));
        assert_eq!(payload.resin, Some(material_id("test-resin")));
        assert_eq!(
            payload.embedded,
            vec![
                EmbeddedProfile::Printer {
                    id: printer_id("test-printer"),
                    bytes: b"kind = \"printer\"".to_vec(),
                },
                EmbeddedProfile::Material {
                    id: material_id("test-resin"),
                    bytes: b"kind = \"material\"".to_vec(),
                },
                EmbeddedProfile::MaterialExtension {
                    id: material_id("test-resin"),
                    bytes: b"kind = \"extension\"".to_vec(),
                },
            ],
        );
    }

    #[test]
    fn an_unsupported_payload_version_keeps_the_scene_and_warns() {
        let mut package = model_only_package();
        add_project_part(&mut package, PAYLOAD_V2.as_bytes());
        add_profile_part(
            &mut package,
            "/Metadata/profiles/printers/test-printer.toml",
            b"kind = \"printer\"",
        );

        let loaded = read(&package).expect("the scene still loads");
        assert!(loaded.payload.is_none());
        assert_eq!(
            loaded.warnings,
            vec![ReadProjectWarning::ProjectVersionUnsupported { version: 2 }],
        );
        assert_eq!(loaded.scene.roots().len(), 1);
    }

    #[test]
    fn a_malformed_project_part_is_an_error() {
        let mut package = model_only_package();
        add_project_part(&mut package, b"=");
        assert!(matches!(
            read(&package).expect_err("malformed TOML is refused"),
            ReadThreeMfProjectError::PayloadParse(_),
        ));
    }

    #[test]
    fn a_non_utf8_project_part_is_an_error() {
        let mut package = model_only_package();
        add_project_part(&mut package, &[0xff, 0xfe, 0x00]);
        assert!(matches!(
            read(&package).expect_err("non-UTF-8 content is refused"),
            ReadThreeMfProjectError::PayloadNotUtf8(_),
        ));
    }

    #[test]
    fn the_slice_fingerprint_is_returned_when_the_document_carries_it() {
        let mut package = model_only_package();
        add_project_part(
            &mut package,
            b"version = 1\n[slices]\nfingerprint = \"cafef00d\"\n",
        );
        let loaded = read(&package).expect("the project loads");
        assert_eq!(loaded.slice_fingerprint, Some("cafef00d".to_string()));
    }

    #[test]
    fn a_document_without_a_fingerprint_yields_none() {
        let mut package = model_only_package();
        add_project_part(&mut package, PAYLOAD_V1_FULL.as_bytes());
        let loaded = read(&package).expect("the project loads");
        assert_eq!(loaded.slice_fingerprint, None);
    }

    #[test]
    fn a_plain_model_has_no_slice_fingerprint() {
        let loaded = read(&model_only_package()).expect("the plain model loads");
        assert_eq!(loaded.slice_fingerprint, None);
    }
}

mod profile_part_classification_tests {
    use super::*;

    #[test]
    fn parts_that_break_the_naming_are_reported_and_skipped() {
        let mut package = model_only_package();
        add_project_part(&mut package, PAYLOAD_V1_FULL.as_bytes());
        add_profile_part(&mut package, "/Metadata/profiles/readme.txt", b"junk");
        add_profile_part(
            &mut package,
            "/Metadata/profiles/printers/nested/deep.toml",
            b"junk",
        );
        add_profile_part(
            &mut package,
            "/Metadata/profiles/printers/BAD.toml",
            b"junk",
        );
        add_profile_part(
            &mut package,
            "/Metadata/profiles/printers/test-printer.toml",
            b"kind = \"printer\"",
        );

        let loaded = read(&package).expect("the project loads");
        let payload = loaded.payload.expect("the payload is present");
        assert_eq!(
            payload.embedded,
            vec![EmbeddedProfile::Printer {
                id: printer_id("test-printer"),
                bytes: b"kind = \"printer\"".to_vec(),
            }],
        );
        assert_eq!(
            loaded.warnings,
            vec![
                ReadProjectWarning::UnrecognizedProfilePart {
                    part_name: "/Metadata/profiles/readme.txt".to_string(),
                },
                ReadProjectWarning::UnrecognizedProfilePart {
                    part_name: "/Metadata/profiles/printers/nested/deep.toml".to_string(),
                },
                ReadProjectWarning::UnrecognizedProfilePart {
                    part_name: "/Metadata/profiles/printers/BAD.toml".to_string(),
                },
            ],
        );
    }

    #[test]
    fn extension_parts_never_classify_as_full_materials() {
        let mut package = model_only_package();
        add_project_part(&mut package, PAYLOAD_V1_FULL.as_bytes());
        add_profile_part(
            &mut package,
            "/Metadata/profiles/materials/extensions/test-resin.toml",
            b"kind = \"extension\"",
        );

        let loaded = read(&package).expect("the project loads");
        let payload = loaded.payload.expect("the payload is present");
        assert_eq!(
            payload.embedded,
            vec![EmbeddedProfile::MaterialExtension {
                id: material_id("test-resin"),
                bytes: b"kind = \"extension\"".to_vec(),
            }],
        );
    }
}

mod foreign_data_tests {
    use super::*;

    #[test]
    fn foreign_markers_are_reported_alongside_a_plain_model() {
        let mut package = model_only_package();
        package
            .add_part(
                part("/Metadata/Slic3r_PE.config"),
                "text/plain",
                b"; prusa config".to_vec(),
            )
            .expect("the marker part is added");

        let loaded = read(&package).expect("the model loads");
        assert!(loaded.payload.is_none());
        assert_eq!(
            loaded.warnings,
            vec![ReadProjectWarning::ForeignProjectData {
                dialect: ForeignDialect::PrusaSlicer,
            }],
        );
    }

    #[test]
    fn foreign_markers_coexist_with_our_payload() {
        let mut package = model_only_package();
        add_project_part(&mut package, PAYLOAD_V1_FULL.as_bytes());
        package
            .add_part(
                part("/Cura/machine.inst.cfg"),
                "text/plain",
                b"[general]".to_vec(),
            )
            .expect("the marker part is added");

        let loaded = read(&package).expect("the project loads");
        assert!(loaded.payload.is_some());
        assert_eq!(
            loaded.warnings,
            vec![ReadProjectWarning::ForeignProjectData {
                dialect: ForeignDialect::Cura,
            }],
        );
    }
}
