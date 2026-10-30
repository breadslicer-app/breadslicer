/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io::{Cursor, Write};

use sc_3mf::package::{Package, PartName};

use crate::messages::ForeignDialect;
use crate::threemf::foreign::{
    BAMBU_ORCA_MARKER_PART, CURA_FOLDER_PREFIX, PRUSA_MARKER_PART, detect_foreign_dialects,
};

fn package_with_parts(names: &[&str]) -> Package {
    let mut package = Package::new();
    for name in names {
        package
            .add_part(
                PartName::new(name).expect("test part name is valid"),
                "application/octet-stream",
                b"data".to_vec(),
            )
            .expect("the test part is added");
    }
    package
}

mod detection_tests {
    use super::*;

    #[test]
    fn an_empty_package_has_no_dialects() {
        assert!(detect_foreign_dialects(&package_with_parts(&[])).is_empty());
    }

    #[test]
    fn unrelated_parts_are_not_dialect_markers() {
        let package = package_with_parts(&["/3D/3dmodel.model", "/Metadata/thumbnail.png"]);
        assert!(detect_foreign_dialects(&package).is_empty());
    }

    #[test]
    fn each_marker_maps_to_its_dialect() {
        assert_eq!(
            detect_foreign_dialects(&package_with_parts(&[PRUSA_MARKER_PART])),
            vec![ForeignDialect::PrusaSlicer],
        );
        assert_eq!(
            detect_foreign_dialects(&package_with_parts(&[BAMBU_ORCA_MARKER_PART])),
            vec![ForeignDialect::BambuStudioOrca],
        );
        let cura_part = format!("{CURA_FOLDER_PREFIX}machine.inst.cfg");
        assert_eq!(
            detect_foreign_dialects(&package_with_parts(&[&cura_part])),
            vec![ForeignDialect::Cura],
        );
    }

    #[test]
    fn several_parts_of_one_dialect_report_it_once() {
        let package = package_with_parts(&[
            "/Cura/machine.inst.cfg",
            "/Cura/extruder.inst.cfg",
            "/Cura/quality.inst.cfg",
        ]);
        assert_eq!(
            detect_foreign_dialects(&package),
            vec![ForeignDialect::Cura],
        );
    }

    #[test]
    fn dialects_are_reported_in_part_order() {
        let package = package_with_parts(&[
            BAMBU_ORCA_MARKER_PART,
            "/Cura/machine.inst.cfg",
            PRUSA_MARKER_PART,
        ]);
        assert_eq!(
            detect_foreign_dialects(&package),
            vec![
                ForeignDialect::BambuStudioOrca,
                ForeignDialect::Cura,
                ForeignDialect::PrusaSlicer,
            ],
        );
    }

    #[test]
    fn a_marker_part_with_no_content_type_counts() {
        // Bambu Studio and Orca do not declare the type of their marker part.
        let mut cursor = Cursor::new(Vec::new());
        let mut archive = zip::write::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("[Content_Types].xml", options).unwrap();
        archive
            .write_all(
                br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
            )
            .unwrap();
        archive
            .start_file(&BAMBU_ORCA_MARKER_PART[1..], options)
            .unwrap();
        archive.write_all(b"{}").unwrap();
        archive.finish().unwrap();
        let package =
            Package::read_from(Cursor::new(cursor.into_inner())).expect("the package reads");
        assert_eq!(
            detect_foreign_dialects(&package),
            vec![ForeignDialect::BambuStudioOrca],
        );
    }
}
