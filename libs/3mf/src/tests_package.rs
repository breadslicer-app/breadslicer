/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Unit tests for the OPC package layer.

use std::io::{Cursor, Write};

use crate::error::{MalformedPartNameError, PackageReadError, PackageWriteError};
use crate::names::{
    DEFAULT_MODEL_PART_NAME, MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE, THUMBNAIL_REL_TYPE,
};
use crate::package::{Package, PartName, Relationship};

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

/// A package with one model part, one extra part, a root StartPart
/// relationship, and one part-level relationship.
fn sample_package() -> Package {
    let mut package = Package::new();
    package
        .add_part(
            part(DEFAULT_MODEL_PART_NAME),
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
    package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME));
    package.add_root_relationship(Relationship {
        id: "rel-2".to_string(),
        rel_type: THUMBNAIL_REL_TYPE.to_string(),
        target: "/Metadata/thumbnail.png".to_string(),
    });
    package.add_part_relationship(
        &part(DEFAULT_MODEL_PART_NAME),
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
mod round_trip_tests {
    use super::*;

    #[test]
    fn package_round_trips_parts_content_types_and_relationships() {
        let original = sample_package();
        let bytes = write_to_bytes(&original);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");

        let names: Vec<&str> = restored.part_names().map(PartName::as_str).collect();
        assert!(names.contains(&DEFAULT_MODEL_PART_NAME), "names: {names:?}");
        assert!(names.contains(&"/2D/slices1.model"));
        assert!(names.contains(&"/Metadata/thumbnail.png"));
        assert_eq!(names.len(), 3, "rels parts must not surface as parts");

        assert_eq!(
            restored.part_content_type(&part(DEFAULT_MODEL_PART_NAME)),
            Some(MODEL_PART_CONTENT_TYPE),
        );
        assert_eq!(
            restored.part_content_type(&part("/Metadata/thumbnail.png")),
            Some("image/png"),
        );
        assert_eq!(
            restored.part_data(&part(DEFAULT_MODEL_PART_NAME)),
            Some(b"<model/>".as_slice()),
        );

        assert_eq!(restored.root_relationships().len(), 2);
        let part_rels = restored.part_relationships(&part(DEFAULT_MODEL_PART_NAME));
        assert_eq!(part_rels.len(), 1);
        assert_eq!(part_rels[0].target, "/2D/slices1.model");

        let start = restored.start_part().expect("start part resolves");
        assert_eq!(start.as_str(), DEFAULT_MODEL_PART_NAME);
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
                part(DEFAULT_MODEL_PART_NAME),
                MODEL_PART_CONTENT_TYPE,
                vec![],
            )
            .unwrap();
        package.add_root_relationship(start_part_relationship("3D/3dmodel.model"));
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            restored.start_part().expect("resolves").as_str(),
            DEFAULT_MODEL_PART_NAME,
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
                part(DEFAULT_MODEL_PART_NAME),
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
                part(DEFAULT_MODEL_PART_NAME),
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
        // Hand-build an archive whose content-types map does not cover
        // the stored part.
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
}
