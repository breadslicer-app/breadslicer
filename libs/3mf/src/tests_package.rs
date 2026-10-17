/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Unit tests for the OPC package layer.

use std::io::{Cursor, Write};

use quick_xml::errors::{Error as XmlError, IllFormedError};

use crate::error::{MalformedPartNameError, PackageReadError, PackageWriteError};
use crate::names::{
    DEFAULT_MODEL_PART_NAME, MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE, THUMBNAIL_REL_TYPE,
};
use crate::package::static_part_name::StaticPartName;
use crate::package::{DecompressionProgress, Package, PackageReadLimits, PartName, Relationship};

fn part(name: &str) -> PartName {
    PartName::new(name).expect("valid part name")
}

fn start_part_relationship(target: &str) -> Relationship {
    Relationship {
        id: "rel-1".to_string(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: target.to_string(),
    }
}

/// Returns a package with 1 model part, 1 extra part, a root StartPart
/// relationship, and 1 part-level relationship.
fn sample_package() -> Package {
    let mut package = Package::new();
    package
        .add_part(
            part(DEFAULT_MODEL_PART_NAME.as_str()),
            MODEL_PART_CONTENT_TYPE,
            b"<model/>".to_vec(),
        )
        .expect("unique name");
    package
        .add_part(
            part("/2D/slices1.model"),
            MODEL_PART_CONTENT_TYPE,
            b"<model2/>".to_vec(),
        )
        .expect("unique name");
    package
        .add_part(part("/Metadata/thumbnail.png"), "image/png", vec![1, 2, 3])
        .expect("unique name");
    package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));
    package.add_root_relationship(Relationship {
        id: "rel-2".to_string(),
        rel_type: THUMBNAIL_REL_TYPE.to_string(),
        target: "/Metadata/thumbnail.png".to_string(),
    });
    package.add_part_relationship(
        &part(DEFAULT_MODEL_PART_NAME.as_str()),
        Relationship {
            id: "rel-100".to_string(),
            rel_type: START_PART_REL_TYPE.to_string(),
            target: "/2D/slices1.model".to_string(),
        },
    );
    package
}

fn write_to_bytes(package: &Package) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    package.write_to(&mut cursor).expect("package writes");
    cursor.into_inner()
}

const MINIMAL_CONTENT_TYPES: &str = include_str!("../tests/fixtures/opc/minimal_content_types.xml");
const MINIMAL_RELS: &str = include_str!("../tests/fixtures/opc/minimal_rels.xml");

/// Returns a hand-built archive holding `content_types`, `rels` as the root
/// relationships part, and the model part `<model/>` styled by
/// `model_options`.
fn opc_package_bytes(
    content_types: &str,
    rels: &str,
    model_options: zip::write::SimpleFileOptions,
) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    let mut archive = zip::write::ZipWriter::new(&mut cursor);
    let plain = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", plain).unwrap();
    archive.write_all(content_types.as_bytes()).unwrap();
    archive.start_file("_rels/.rels", plain).unwrap();
    archive.write_all(rels.as_bytes()).unwrap();
    archive
        .start_file("3D/3dmodel.model", model_options)
        .unwrap();
    archive.write_all(b"<model/>").unwrap();
    archive.finish().unwrap();
    cursor.into_inner()
}

/// Returns a hand-built archive holding the minimal fixture entries (content
/// types, root rels, `<model/>`), so tests know the exact decompressed size of
/// every entry. `model_options` styles the model entry (e.g. forcing ZIP64
/// structures via `large_file`).
fn minimal_package_bytes(model_options: zip::write::SimpleFileOptions) -> Vec<u8> {
    opc_package_bytes(MINIMAL_CONTENT_TYPES, MINIMAL_RELS, model_options)
}

/// Returns the decompressed size of every entry `minimal_package_bytes` writes.
fn minimal_package_decompressed_len() -> u64 {
    (MINIMAL_CONTENT_TYPES.len() + MINIMAL_RELS.len() + b"<model/>".len()) as u64
}

/// Returns a ZIP archive holding 1 deflated entry named `big.bin`.
fn single_entry_archive(payload: Vec<u8>) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    let mut archive = zip::write::ZipWriter::new(&mut cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    archive.start_file("big.bin", options).unwrap();
    archive.write_all(&payload).unwrap();
    archive.finish().unwrap();
    cursor.into_inner()
}

/// Forges the entry's 4-byte uncompressed-size field, in both the local file
/// header and the central directory, to claim `claimed` bytes. Single-entry
/// archives only.
fn forge_claimed_size(bytes: &mut [u8], claimed: u32) {
    assert_eq!(&bytes[0..4], b"PK\x03\x04", "local header at start");
    bytes[22..26].copy_from_slice(&claimed.to_le_bytes());
    // The last central-directory signature is the real one: the entry's payload
    // can only appear before the directory.
    let central = bytes
        .windows(4)
        .rposition(|w| w == b"PK\x01\x02")
        .expect("central directory header");
    bytes[central + 24..central + 28].copy_from_slice(&claimed.to_le_bytes());
}

#[cfg(test)]
mod part_name_tests {
    use super::*;

    #[test]
    fn accepts_conventional_names() {
        for name in [
            "/3D/3dmodel.model",
            "/[Content_Types].xml",
            "/2D/8cb173a3.model",
            "/Metadata/thumbnail.png",
            "/single",
        ] {
            assert!(PartName::new(name).is_ok(), "should accept {name}");
        }
    }

    #[test]
    fn rejects_malformed_names() {
        assert_eq!(PartName::new(""), Err(MalformedPartNameError::Empty));
        assert_eq!(
            PartName::new("3D/model"),
            Err(MalformedPartNameError::NotAbsolute),
        );
        assert_eq!(
            PartName::new("/"),
            Err(MalformedPartNameError::EmptySegment)
        );
        assert_eq!(
            PartName::new("/3D//model"),
            Err(MalformedPartNameError::EmptySegment),
        );
        assert_eq!(
            PartName::new("/3D/model."),
            Err(MalformedPartNameError::TrailingDot),
        );
    }
}

#[cfg(test)]
mod static_part_name_tests {
    use super::*;

    #[test]
    fn accepts_conventional_names_and_gives_them_back() {
        for name in [
            "/3D/3dmodel.model",
            "/[Content_Types].xml",
            "/2D/8cb173a3.model",
            "/Metadata/thumbnail.png",
            "/single",
            "/3D/modèle.model",
        ] {
            assert_eq!(
                StaticPartName::new(name).map(StaticPartName::as_str),
                Ok(name),
            );
        }
    }

    #[test]
    fn rejects_malformed_names() {
        for (name, error) in [
            ("", MalformedPartNameError::Empty),
            ("3D/model", MalformedPartNameError::NotAbsolute),
            ("3D/model.", MalformedPartNameError::NotAbsolute),
            ("/", MalformedPartNameError::EmptySegment),
            ("//", MalformedPartNameError::EmptySegment),
            ("//.", MalformedPartNameError::EmptySegment),
            ("/3D//model", MalformedPartNameError::EmptySegment),
            ("/3D/", MalformedPartNameError::EmptySegment),
            ("/3D/model.", MalformedPartNameError::TrailingDot),
            ("/3D/modèle.", MalformedPartNameError::TrailingDot),
        ] {
            assert_eq!(StaticPartName::new(name), Err(error), "{name:?}");
        }
    }

    #[test]
    fn converts_into_the_part_name_that_new_gives() {
        let name = StaticPartName::new("/3D/3dmodel.model").expect("valid part name");
        assert_eq!(PartName::from(name), part("/3D/3dmodel.model"));
    }
}

#[cfg(test)]
mod part_name_file_stem_tests {
    use super::*;

    #[test]
    fn gives_the_stem_of_a_file_in_the_directory_with_the_extension() {
        let name = part("/Metadata/profiles/printers/a-b.toml");
        assert_eq!(
            name.file_stem_in("/Metadata/profiles/printers/", "toml"),
            Some("a-b"),
        );
    }

    #[test]
    fn gives_the_stem_of_a_root_level_part() {
        assert_eq!(part("/x.bin").file_stem_in("/", "bin"), Some("x"));
    }

    #[test]
    fn keeps_the_dots_inside_the_stem() {
        let name = part("/Metadata/a/x.y.toml");
        assert_eq!(name.file_stem_in("/Metadata/a/", "toml"), Some("x.y"));
    }

    #[test]
    fn refuses_a_part_outside_the_directory() {
        for (name, directory) in [
            ("/Metadata/a/b/c.toml", "/Metadata/a/"),
            ("/Metadata/b/c.toml", "/Metadata/a/"),
            ("/Metadata/a/c.toml", "/Metadata/a"),
        ] {
            assert_eq!(
                part(name).file_stem_in(directory, "toml"),
                None,
                "{name} in {directory}",
            );
        }
    }

    #[test]
    fn refuses_another_extension() {
        for name in [
            "/Metadata/a/c.json",
            "/Metadata/a/c.TOML",
            "/Metadata/a/ctoml",
        ] {
            assert_eq!(
                part(name).file_stem_in("/Metadata/a/", "toml"),
                None,
                "{name}"
            );
        }
    }

    #[test]
    fn refuses_a_file_name_without_a_stem() {
        for name in ["/Metadata/a/.toml", "/Metadata/a/toml"] {
            assert_eq!(
                part(name).file_stem_in("/Metadata/a/", "toml"),
                None,
                "{name}"
            );
        }
    }
}

#[cfg(test)]
mod round_trip_tests {
    use super::*;

    #[test]
    fn package_round_trips_parts_content_types_and_relationships() {
        let original = sample_package();
        let bytes = write_to_bytes(&original);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");

        let names: Vec<&str> = restored.part_names().map(PartName::as_str).collect();
        assert!(
            names.contains(&DEFAULT_MODEL_PART_NAME.as_str()),
            "names: {names:?}"
        );
        assert!(names.contains(&"/2D/slices1.model"));
        assert!(names.contains(&"/Metadata/thumbnail.png"));
        assert_eq!(names.len(), 3, "rels parts must not surface as parts");

        assert_eq!(
            restored.part_content_type(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(MODEL_PART_CONTENT_TYPE),
        );
        assert_eq!(
            restored.part_content_type(&part("/Metadata/thumbnail.png")),
            Some("image/png"),
        );
        assert_eq!(
            restored.part_data(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(b"<model/>".as_slice()),
        );

        assert_eq!(restored.root_relationships().len(), 2);
        let part_rels = restored.part_relationships(&part(DEFAULT_MODEL_PART_NAME.as_str()));
        assert_eq!(part_rels.len(), 1);
        assert_eq!(part_rels[0].target, "/2D/slices1.model");

        let start = restored.start_part().expect("start part resolves");
        assert_eq!(start.as_str(), DEFAULT_MODEL_PART_NAME.as_str());
    }

    #[test]
    fn conflicting_extension_content_types_round_trip_via_overrides() {
        let mut package = Package::new();
        package
            .add_part(part("/a/data.bin"), "application/x-alpha", vec![1])
            .unwrap();
        package
            .add_part(part("/b/data.bin"), "application/x-beta", vec![2])
            .unwrap();
        package.add_root_relationship(start_part_relationship("/a/data.bin"));
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            restored.part_content_type(&part("/a/data.bin")),
            Some("application/x-alpha"),
        );
        assert_eq!(
            restored.part_content_type(&part("/b/data.bin")),
            Some("application/x-beta"),
        );
    }

    #[test]
    fn relative_start_part_target_resolves_from_the_root() {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![],
            )
            .unwrap();
        package.add_root_relationship(start_part_relationship("3D/3dmodel.model"));
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            restored.start_part().expect("resolves").as_str(),
            DEFAULT_MODEL_PART_NAME.as_str(),
        );
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn duplicate_part_names_are_rejected_at_add_time() {
        let mut package = Package::new();
        package
            .add_part(part("/a.bin"), "application/octet-stream", vec![])
            .unwrap();
        let err = package
            .add_part(part("/a.bin"), "application/octet-stream", vec![])
            .unwrap_err();
        assert!(matches!(err, PackageWriteError::DuplicatePartName { name } if name == "/a.bin"),);
    }

    #[test]
    fn missing_content_types_part_is_rejected() {
        let mut cursor = Cursor::new(Vec::new());
        let mut archive = zip::write::ZipWriter::new(&mut cursor);
        archive
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"<model/>").unwrap();
        archive.finish().unwrap();
        let err = Package::read_from(Cursor::new(cursor.into_inner())).unwrap_err();
        assert!(matches!(err, PackageReadError::MissingContentTypes));
    }

    #[test]
    fn missing_start_part_relationship_is_reported_on_lookup() {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![],
            )
            .unwrap();
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        let err = restored.start_part().unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::MissingStartPartRelationship
        ));
    }

    #[test]
    fn start_part_pointing_at_a_missing_part_is_reported() {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![],
            )
            .unwrap();
        package.add_root_relationship(start_part_relationship("/3D/absent.model"));
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        let err = restored.start_part().unwrap_err();
        assert!(
            matches!(err, PackageReadError::StartPartNotFound { target } if target == "/3D/absent.model"),
        );
    }

    #[test]
    fn part_without_content_type_is_rejected() {
        // Hand-build an archive whose content-types map does not cover the
        // stored part.
        let mut cursor = Cursor::new(Vec::new());
        let mut archive = zip::write::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("[Content_Types].xml", options).unwrap();
        archive
            .write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/></Types>"#,
            )
            .unwrap();
        archive.start_file("3D/3dmodel.model", options).unwrap();
        archive.write_all(b"<model/>").unwrap();
        archive.finish().unwrap();
        let err = Package::read_from(Cursor::new(cursor.into_inner())).unwrap_err();
        assert!(
            matches!(err, PackageReadError::PartWithoutContentType { part } if part == "/3D/3dmodel.model"),
        );
    }

    #[test]
    fn content_types_without_their_end_tag_are_rejected() {
        let content_types = MINIMAL_CONTENT_TYPES.replace("</Types>", "");
        let bytes = opc_package_bytes(&content_types, MINIMAL_RELS, Default::default());
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::ContentTypesXml(XmlError::IllFormed(IllFormedError::MissingEndTag(name))) if name == "Types"),
            "{err:?}",
        );
    }

    #[test]
    fn the_innermost_open_element_is_reported() {
        let content_types = MINIMAL_CONTENT_TYPES.replace("</Types>", "<Extra>");
        let bytes = opc_package_bytes(&content_types, MINIMAL_RELS, Default::default());
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::ContentTypesXml(XmlError::IllFormed(IllFormedError::MissingEndTag(name))) if name == "Extra"),
            "{err:?}",
        );
    }

    #[test]
    fn relationships_without_their_end_tag_are_rejected() {
        let rels = MINIMAL_RELS.replace("</Relationships>", "");
        let bytes = opc_package_bytes(MINIMAL_CONTENT_TYPES, &rels, Default::default());
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::RelationshipsXml { part, source: XmlError::IllFormed(IllFormedError::MissingEndTag(name)) } if part == "/_rels/.rels" && name == "Relationships"),
            "{err:?}",
        );
    }

    #[test]
    fn content_types_cut_inside_their_root_are_rejected() {
        let start = MINIMAL_CONTENT_TYPES.find("<Types").unwrap() + 1;
        let end = MINIMAL_CONTENT_TYPES.find("</Types>").unwrap() + "</Types>".len();
        for len in start..end {
            let content_types = &MINIMAL_CONTENT_TYPES[..len];
            let bytes = opc_package_bytes(content_types, MINIMAL_RELS, Default::default());
            let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(err, PackageReadError::ContentTypesXml(_)),
                "{len} bytes: {err:?}",
            );
        }
    }

    #[test]
    fn relationships_cut_inside_their_root_are_rejected() {
        let start = MINIMAL_RELS.find("<Relationships").unwrap() + 1;
        let end = MINIMAL_RELS.find("</Relationships>").unwrap() + "</Relationships>".len();
        for len in start..end {
            let rels = &MINIMAL_RELS[..len];
            let bytes = opc_package_bytes(MINIMAL_CONTENT_TYPES, rels, Default::default());
            let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(&err, PackageReadError::RelationshipsXml { part, .. } if part == "/_rels/.rels"),
                "{len} bytes: {err:?}",
            );
        }
    }
}

#[cfg(test)]
mod read_limits_tests {
    use super::*;

    /// Returns a serialized package whose only user part is the model part,
    /// carrying `data`. The content-types and rels entries it also contains
    /// stay well under a kilobyte each.
    fn package_bytes_with_model_part(data: Vec<u8>) -> Vec<u8> {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                data,
            )
            .expect("unique name");
        package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));
        write_to_bytes(&package)
    }

    #[test]
    fn default_limits_are_2_gib_per_part_and_4_gib_total() {
        let limits = PackageReadLimits::default();
        assert_eq!(limits.max_part_bytes, 2 * (1u64 << 30));
        assert_eq!(limits.max_total_bytes, 4 * (1u64 << 30));
    }

    #[test]
    fn part_exactly_at_the_part_cap_is_read_intact() {
        let bytes = package_bytes_with_model_part(vec![b'a'; 1024]);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1 << 20,
        };
        let package = Package::read_from_with_limits(Cursor::new(bytes), limits)
            .expect("a part at the cap must pass");
        let data = package
            .part_data(&part(DEFAULT_MODEL_PART_NAME.as_str()))
            .expect("part present");
        assert_eq!(data.len(), 1024);
        assert!(data.iter().all(|&b| b == b'a'));
    }

    #[test]
    fn part_one_byte_over_the_part_cap_is_refused() {
        let bytes = package_bytes_with_model_part(vec![b'a'; 1025]);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1 << 20,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPart { part, cap_bytes: 1024 }
                if part == DEFAULT_MODEL_PART_NAME.as_str()
        ));
    }

    #[test]
    fn highly_compressible_oversized_part_is_refused() {
        // 1 MiB of zeros deflates to about a kilobyte: the archive stays tiny
        // on disk while its decompressed size dwarfs the cap, so an on-disk
        // size gate alone would not catch it.
        let bytes = package_bytes_with_model_part(vec![0u8; 1 << 20]);
        assert!(bytes.len() < 64 * 1024, "archive must stay small on disk");
        let limits = PackageReadLimits {
            max_part_bytes: 64 * 1024,
            max_total_bytes: 1 << 30,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPart { part, .. } if part == DEFAULT_MODEL_PART_NAME.as_str()
        ));
    }

    #[test]
    fn parts_summing_past_the_total_cap_are_refused() {
        let mut package = Package::new();
        for name in ["/a.bin", "/b.bin", "/c.bin"] {
            package
                .add_part(part(name), "application/octet-stream", vec![b'x'; 4096])
                .expect("unique name");
        }
        package.add_root_relationship(start_part_relationship("/a.bin"));
        let bytes = write_to_bytes(&package);
        // Each part fits the per-part cap; their sum (plus the small
        // content-types and rels entries) crosses the total cap.
        let limits = PackageReadLimits {
            max_part_bytes: 8192,
            max_total_bytes: 10_000,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPackage { cap_bytes: 10_000 }
        ));
    }

    #[test]
    fn forged_header_claiming_huge_size_is_refused() {
        // A tiny entry whose forged header claims 1 GiB: the claimed-size check
        // must refuse it before allocating.
        let mut bytes = single_entry_archive(b"0123456789".to_vec());
        forge_claimed_size(&mut bytes, 1 << 30);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1 << 20,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPart { part, cap_bytes: 1024 } if part == "/big.bin"
        ));
    }

    #[test]
    fn forged_header_claiming_fewer_bytes_than_real_is_still_capped() {
        // The header claims 10 bytes, so the claimed-size check passes; the
        // bytes actually inflated must still be counted and refused through the
        // take bound.
        let mut bytes = single_entry_archive(vec![0u8; 100 * 1024]);
        forge_claimed_size(&mut bytes, 10);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1 << 20,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPart { part, cap_bytes: 1024 } if part == "/big.bin"
        ));
    }

    #[test]
    fn forged_header_cannot_sneak_past_the_total_cap_either() {
        // Same lying-low header, but here the total ceiling is the tighter one,
        // so the overrun must surface as a package-level refusal.
        let mut bytes = single_entry_archive(vec![0u8; 100 * 1024]);
        forge_claimed_size(&mut bytes, 10);
        let limits = PackageReadLimits {
            max_part_bytes: 2048,
            max_total_bytes: 1024,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPackage { cap_bytes: 1024 }
        ));
    }
}

#[cfg(test)]
mod zip64_tests {
    use super::*;

    /// Returns the byte offset right after a local file header's name, where
    /// the entry's extra field begins.
    fn local_extra_field_offset(bytes: &[u8], name: &[u8]) -> usize {
        let name_pos = (30..=bytes.len() - name.len())
            .find(|&p| &bytes[p - 30..p - 26] == b"PK\x03\x04" && &bytes[p..p + name.len()] == name)
            .expect("local file header for the entry");
        name_pos + name.len()
    }

    #[test]
    fn zip64_entries_are_read_intact() {
        // The writer's large_file option forces ZIP64 structures onto the entry
        // even though its real size is small, which pins ZIP64 reading without
        // a multi-gigabyte fixture.
        let zip64 = zip::write::SimpleFileOptions::default().large_file(true);
        let bytes = minimal_package_bytes(zip64);

        // The model entry's local header must carry the ZIP64
        // extended-information extra field (id 0x0001); without it this test
        // would no longer pin ZIP64 reading.
        let extra = local_extra_field_offset(&bytes, b"3D/3dmodel.model");
        assert_eq!(
            &bytes[extra..extra + 2],
            [0x01u8, 0x00],
            "zip64 extra field id"
        );

        let package = Package::read_from(Cursor::new(bytes)).expect("zip64 package reads");
        assert_eq!(
            package.part_data(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(b"<model/>".as_slice()),
        );
        assert_eq!(
            package.part_content_type(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(MODEL_PART_CONTENT_TYPE),
        );
        let start = package.start_part().expect("start part resolves");
        assert_eq!(start.as_str(), DEFAULT_MODEL_PART_NAME.as_str());
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    fn read_collecting_ticks(
        bytes: Vec<u8>,
    ) -> (
        Result<Package, PackageReadError>,
        Vec<DecompressionProgress>,
    ) {
        let mut ticks = Vec::new();
        let result = Package::read_from_with_progress(Cursor::new(bytes), |p| ticks.push(p));
        (result, ticks)
    }

    /// Asserts the ticks of an archive whose announced sizes are accurate: at
    /// least 1 tick, a fixed total on every tick, strictly growing counts
    /// (ticks only fire when bytes were inflated), and a final count equal to
    /// the total, which it returns.
    fn assert_monotone_ending_at_total(ticks: &[DecompressionProgress]) -> u64 {
        assert!(!ticks.is_empty(), "reading a package must tick");
        let total = ticks[0].total_bytes;
        for pair in ticks.windows(2) {
            assert_eq!(pair[1].total_bytes, total, "total must stay fixed");
            assert!(
                pair[1].decompressed_bytes > pair[0].decompressed_bytes,
                "ticks must carry strictly growing counts"
            );
        }
        let last = ticks.last().expect("non-empty").decompressed_bytes;
        assert_eq!(last, total, "accurate announced sizes end at the total");
        total
    }

    #[test]
    fn known_entry_sizes_produce_an_exact_total() {
        let bytes = minimal_package_bytes(zip::write::SimpleFileOptions::default());
        let (result, ticks) = read_collecting_ticks(bytes);
        result.expect("package reads");
        let total = assert_monotone_ending_at_total(&ticks);
        assert_eq!(total, minimal_package_decompressed_len());
    }

    #[test]
    fn zip64_archive_ticks_end_at_its_exact_total() {
        let zip64 = zip::write::SimpleFileOptions::default().large_file(true);
        let bytes = minimal_package_bytes(zip64);
        let (result, ticks) = read_collecting_ticks(bytes);
        result.expect("zip64 package reads");
        let total = assert_monotone_ending_at_total(&ticks);
        assert_eq!(total, minimal_package_decompressed_len());
    }

    #[test]
    fn round_tripped_package_ticks_cover_parts_and_generated_entries() {
        let bytes = write_to_bytes(&sample_package());
        let (result, ticks) = read_collecting_ticks(bytes);
        result.expect("package reads");
        let total = assert_monotone_ending_at_total(&ticks);
        // The 3 payloads of `sample_package` plus the generated content-types
        // and relationship entries: the exact XML sizes are the writer's
        // business, but the total must cover at least the payload bytes.
        let payload_bytes = (b"<model/>".len() + b"<model2/>".len() + 3) as u64;
        assert!(total > payload_bytes, "total covers the non-part entries");
    }

    #[test]
    fn oversized_claim_is_refused_before_any_tick() {
        // 3 GiB claimed against the default 2 GiB per-part ceiling: the
        // claimed-size check refuses the entry before a single byte is
        // inflated, so no tick may fire.
        let mut bytes = single_entry_archive(b"0123456789".to_vec());
        forge_claimed_size(&mut bytes, 3 << 30);
        let (result, ticks) = read_collecting_ticks(bytes);
        assert!(matches!(
            result.unwrap_err(),
            PackageReadError::OversizedPart { part, .. } if part == "/big.bin"
        ));
        assert!(ticks.is_empty(), "a refused claim must not tick");
    }

    #[test]
    fn lying_low_claim_ticks_past_the_announced_total() {
        // The header claims 10 bytes but inflation yields 100 KiB; under the
        // default ceilings the entry itself is accepted, so the counts step
        // past the claimed total exactly as the `DecompressionProgress` docs
        // warn. The read still fails later (no content-types entry), which must
        // not retract the ticks already delivered.
        let mut bytes = single_entry_archive(vec![0u8; 100 * 1024]);
        forge_claimed_size(&mut bytes, 10);
        let (result, ticks) = read_collecting_ticks(bytes);
        assert!(matches!(
            result.unwrap_err(),
            PackageReadError::MissingContentTypes
        ));
        let last = ticks.last().expect("extraction must have ticked");
        assert_eq!(last.total_bytes, 10, "the forged claim is the total");
        assert_eq!(
            last.decompressed_bytes,
            100 * 1024,
            "counts report real inflated bytes, not the claim"
        );
    }
}

mod write_progress_tests {
    use super::*;

    fn write_collecting_ticks(package: &Package) -> (Vec<u8>, Vec<(u64, u64)>) {
        let mut cursor = Cursor::new(Vec::new());
        let mut ticks = Vec::new();
        package
            .write_to_with_progress(&mut cursor, |done, total| ticks.push((done, total)))
            .expect("package writes");
        (cursor.into_inner(), ticks)
    }

    /// Returns a package whose single part is larger than the compression
    /// chunk, so writing it yields several intermediate ticks.
    fn large_part_package(part_bytes: usize) -> Package {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![b'x'; part_bytes],
            )
            .expect("unique name");
        package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));
        package
    }

    #[test]
    fn ticks_are_monotone_and_end_at_the_part_total() {
        let part_bytes = 700 * 1024;
        let (_, ticks) = write_collecting_ticks(&large_part_package(part_bytes));

        assert!(ticks.len() >= 3, "a multi-chunk part ticks several times");
        let total = ticks[0].1;
        assert_eq!(total, part_bytes as u64, "total is the sum of part bytes");
        for pair in ticks.windows(2) {
            assert_eq!(pair[1].1, total, "total stays fixed");
            assert!(pair[1].0 > pair[0].0, "counts grow strictly");
        }
        assert_eq!(
            ticks.last().unwrap().0,
            total,
            "the last tick reaches total"
        );
    }

    #[test]
    fn the_total_sums_every_part_and_ignores_generated_entries() {
        // 3 parts of 8, 9, and 3 bytes; the content-type and relationship
        // entries the writer generates are not counted.
        let (_, ticks) = write_collecting_ticks(&sample_package());
        let total = ticks[0].1;
        assert_eq!(total, (b"<model/>".len() + b"<model2/>".len() + 3) as u64);
        assert_eq!(ticks.last().unwrap().0, total);
    }

    #[test]
    fn progress_does_not_change_the_written_bytes() {
        let package = sample_package();
        let plain = write_to_bytes(&package);
        let (ticked, _) = write_collecting_ticks(&package);
        assert_eq!(plain, ticked, "reporting progress must not alter output");
    }

    #[test]
    fn an_empty_package_reports_a_zero_total() {
        let package = Package::new();
        let (_, ticks) = write_collecting_ticks(&package);
        // No content parts: nothing to tick against.
        assert!(ticks.is_empty(), "a package with no parts does not tick");
    }

    #[test]
    fn a_part_compressed_across_threads_still_ticks_to_total() {
        // 5 MiB is past the threshold that routes a part to the parallel
        // deflater, so this exercises the progress reporting of that path
        // rather than the single-threaded one.
        let part_bytes = 5 * 1024 * 1024;
        let (_, ticks) = write_collecting_ticks(&large_part_package(part_bytes));

        assert!(ticks.len() >= 3, "a large part ticks several times");
        let total = ticks[0].1;
        assert_eq!(total, part_bytes as u64, "total is the sum of part bytes");
        for pair in ticks.windows(2) {
            assert_eq!(pair[1].1, total, "total stays fixed");
            assert!(pair[1].0 > pair[0].0, "counts grow strictly");
        }
        assert_eq!(
            ticks.last().unwrap().0,
            total,
            "the last tick reaches total"
        );
    }
}

mod parallel_compression_tests {
    use super::*;

    /// Returns bytes with local structure, so the compressor finds cross-chunk
    /// back-references (exercising the dictionary priming) instead of the
    /// degenerate all-same case.
    fn varied_bytes(len: usize) -> Vec<u8> {
        let mut data = Vec::with_capacity(len + 64);
        let mut state = 0x2545_f491u32;
        while data.len() < len {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            data.extend_from_slice(
                format!(
                    "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
                    state % 1000,
                    (state >> 7) % 1000,
                    (state >> 14) % 1000,
                )
                .as_bytes(),
            );
        }
        data.truncate(len);
        data
    }

    #[test]
    fn a_large_part_round_trips_through_the_parallel_path() {
        // 5 MiB is past PARALLEL_PART_THRESHOLD, so the big part is deflated
        // across threads and spliced in, while the small part stays on the
        // single-threaded path. Both must read back exact.
        let big = varied_bytes(5 * 1024 * 1024);
        let small = b"<model/>".to_vec();

        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                big.clone(),
            )
            .expect("unique name");
        package
            .add_part(
                part("/2D/slices1.model"),
                MODEL_PART_CONTENT_TYPE,
                small.clone(),
            )
            .expect("unique name");
        package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));

        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");

        assert_eq!(
            restored.part_data(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(big.as_slice()),
        );
        assert_eq!(
            restored.part_data(&part("/2D/slices1.model")),
            Some(small.as_slice()),
        );
        assert_eq!(
            restored.part_content_type(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(MODEL_PART_CONTENT_TYPE),
        );
    }

    #[test]
    fn a_maximally_compressible_large_part_round_trips() {
        // A degenerate, all-identical part still round-trips: dictionary
        // priming across chunk boundaries must not corrupt the stream.
        let big = vec![b'x'; 5 * 1024 * 1024];
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                big.clone(),
            )
            .expect("unique name");
        package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));

        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            restored.part_data(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(big.as_slice()),
        );
    }
}
