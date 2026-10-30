/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io::{Cursor, Write};

use sc_3mf::error::PackageReadError;
use sc_3mf::names::{MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_formats::error::LoadThreeMfError;
use sc_mesh_formats::messages::LoadThreeMfWarning;
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

/// Returns a package holding just the tetrahedron model part and its start-part
/// relationship.
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
            part(PROJECT_PART_NAME.as_str()),
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

    #[test]
    fn profile_directory_parts_without_an_id_and_toml_are_reported_and_skipped() {
        let names = [
            "/Metadata/profiles/printers/.toml",
            "/Metadata/profiles/printers/test-printer.TOML",
            "/Metadata/profiles/printers/test-printer.json",
        ];
        let mut package = model_only_package();
        add_project_part(&mut package, PAYLOAD_V1_FULL.as_bytes());
        for name in names {
            add_profile_part(&mut package, name, b"junk");
        }

        let loaded = read(&package).expect("the project loads");
        let payload = loaded.payload.expect("the payload is present");
        assert_eq!(payload.embedded, vec![]);
        assert_eq!(
            loaded.warnings,
            names.map(|name| ReadProjectWarning::UnrecognizedProfilePart {
                part_name: name.to_string(),
            }),
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

/// Packages shaped as Bambu Studio and Orca write them: the object in its own
/// model part, referenced from the start part with the production extension's
/// `p:path`, and settings and pictures that the content-type map does not
/// cover.
mod bambu_tests {
    use super::*;

    const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
 <Default Extension="png" ContentType="image/png"/>
</Types>"#;

    const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

    const MODEL_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/Objects/object_1.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

    const START_PART: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:BambuStudio="http://schemas.bambulab.com/package/2021" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" requiredextensions="p">
 <metadata name="Application">BambuStudio-02.07.01.62</metadata>
 <resources>
  <object id="2" p:UUID="00000001-61cb-4c03-9d28-80fed5dfa1dc" type="model">
   <components>
    <component p:path="/3D/Objects/object_1.model" objectid="1" p:UUID="00010000-b206-40ff-9872-83e8017abed1" transform="1 0 0 0 1 0 0 0 1 0 0 0"/>
   </components>
  </object>
 </resources>
 <build p:UUID="2c7c17d8-22b5-4d84-8835-1976022ea369">
  <item objectid="2" p:UUID="00000002-b1ec-4553-aec9-835e5b724bb4" transform="1 0 0 0 1 0 0 0 1 128 128 0" printable="1"/>
 </build>
</model>"#;

    /// Returns the archive with `content_types`, the start part, the object
    /// part, and the parts with no content type.
    fn bambu_archive(content_types: &str) -> Vec<u8> {
        let items: [(&str, &[u8]); 9] = [
            ("[Content_Types].xml", content_types.as_bytes()),
            ("Metadata/plate_1.png", b"png"),
            ("3D/3dmodel.model", START_PART.as_bytes()),
            ("3D/_rels/3dmodel.model.rels", MODEL_RELS.as_bytes()),
            ("3D/Objects/object_1.model", TETRAHEDRON_MODEL.as_bytes()),
            ("Metadata/project_settings.config", b"{}"),
            ("Metadata/model_settings.config", b"<config/>"),
            ("Auxiliaries/Model Pictures/photo.webp", b"webp"),
            ("_rels/.rels", ROOT_RELS.as_bytes()),
        ];
        let mut cursor = Cursor::new(Vec::new());
        let mut archive = zip::write::ZipWriter::new(&mut cursor);
        for (name, bytes) in items {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .expect("the entry starts");
            archive.write_all(bytes).expect("the entry writes");
        }
        archive.finish().expect("the archive finishes");
        cursor.into_inner()
    }

    #[test]
    fn a_bambu_project_opens_and_names_the_parts_it_skips() {
        let mut bytes = Cursor::new(bambu_archive(CONTENT_TYPES));
        let loaded = read_threemf_project(&mut bytes).expect("the project opens");
        assert_eq!(loaded.scene.mesh_resources_len(), 1);
        assert!(loaded.payload.is_none());
        assert_eq!(
            loaded.warnings,
            vec![
                ReadProjectWarning::ThreeMf(LoadThreeMfWarning::UntypedPartsSkipped {
                    parts: vec![
                        "/Metadata/project_settings.config".to_string(),
                        "/Metadata/model_settings.config".to_string(),
                        "/Auxiliaries/Model Pictures/photo.webp".to_string(),
                    ],
                }),
                ReadProjectWarning::ForeignProjectData {
                    dialect: ForeignDialect::BambuStudioOrca,
                },
            ],
        );
    }

    #[test]
    fn an_object_part_with_no_content_type_is_refused_by_name() {
        // Only the start part has a type, by override.
        let content_types = CONTENT_TYPES.replace(
            r#"<Default Extension="model" "#,
            r#"<Override PartName="/3D/3dmodel.model" "#,
        );
        let mut bytes = Cursor::new(bambu_archive(&content_types));
        let err = read_threemf_project(&mut bytes).expect_err("the project is refused");
        assert!(
            matches!(
                &err,
                ReadThreeMfProjectError::Scene(LoadThreeMfError::Package(
                    PackageReadError::PartWithoutContentType { part }
                )) if part == "/3D/Objects/object_1.model"
            ),
            "{err:?}",
        );
    }
}
