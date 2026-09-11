/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use sc_print_profiles::{
    CalibrationTarget, EffectiveMaterial, SceneOverrides, load_material_profile_from_path,
    load_printer_profile_from_path, resolve,
};
use sc_profiles_converter::cli::RawCliArgs;
use sc_profiles_converter::format::SourceFormat;
use sc_profiles_converter::request::ConversionRequest;
use sc_profiles_converter::run::run;
use sc_profiles_converter::source_note::SourceNote;

/// The header the vendor's sheet carries, above the note it opens with.
const HEADER: [&str; 11] = [
    "3DP Model",
    "Resin Type",
    "Notes",
    "Color",
    "Bottom Exposure(s)",
    "Normal Exposure(s)",
    "Layer Height(mm)",
    "Z Lifting Distance(mm)",
    "Z Lifting Speed(mm/min)",
    "Z Retract Speed(mm/min)",
    "Rest Time After Retract(s)",
];

/// Rows copied from the published sheet, one per shape the reader has
/// to handle: a split travel, a machine that moves in one go, one cell
/// naming 2 machines, a machine that plans its own motion, a span a
/// spreadsheet program rewrote as a date, the one written-out date, a
/// row qualified by a printing mode, a machine we ship nothing for, and
/// the one pair of rows that contradict each other.
const ROWS: [[&str; 11]; 12] = [
    [
        "ELEGOO Mars 5",
        "Standard Resin",
        "",
        "Grey",
        "27-32",
        "2.5-3",
        "0.05",
        "2+3",
        "75+230",
        "230+75",
        "0.5",
    ],
    [
        "ELEGOO Mars 5",
        "Standard Resin",
        "",
        "Black",
        "30-35",
        "3-3.5",
        "0.05",
        "2+3",
        "75+230",
        "230+75",
        "0.5",
    ],
    [
        "ELEGOO Mars 3/Mars 3 Pro",
        "Standard Resin",
        "",
        "Grey",
        "25-30",
        "2.5-3",
        "0.05",
        "5+0",
        "80+0",
        "210+0",
        "0.5",
    ],
    [
        "ELEGOO Mars 2 Pro/Mars 2",
        "Standard Resin",
        "",
        "Grey",
        "25-35",
        "2.5-3",
        "0.05",
        "5+0",
        "80+0",
        "210+0",
        "0.5",
    ],
    [
        "ELEGOO Mars 2 Pro/Mars2",
        "Standard Resin",
        "",
        "Grey",
        "25-35",
        "2.5-3",
        "0.05",
        "5+0",
        "80+0",
        "210+0",
        "0.5",
    ],
    [
        "ELEGOO Mars 5 Ultra",
        "Standard Resin",
        "",
        "Grey",
        "27-32",
        "2.5-3",
        "0.05",
        "/",
        "/",
        "/",
        "/",
    ],
    [
        "ELEGOO Saturn 3",
        "ABS-Like Resin",
        "",
        "clear",
        "35-40",
        "6\u{6708}8\u{65e5}",
        "0.05",
        "3+4",
        "65+180",
        "180+65",
        "0.5",
    ],
    [
        "ELEGOO Saturn 4",
        "ABS-Like Resin",
        "",
        "Grey",
        "35-40",
        "45816.0",
        "0.05",
        "3+4",
        "65+180",
        "180+65",
        "0.5",
    ],
    [
        "ELEGOO Mars 4 Ultra",
        "Rapid Standard Photopolymer Resin",
        "NOTE: This parameter works for Fast Resin + Fast Speed",
        "Grey",
        "30-40",
        "1.8",
        "0.1",
        "2+1",
        "300+1000",
        "1000+300",
        "0.0",
    ],
    [
        "ELEGOO Saturn 8K",
        "Standard Resin",
        "",
        "Grey",
        "25-30",
        "2.5-3",
        "0.05",
        "5+0",
        "80+0",
        "210+0",
        "0.5",
    ],
    [
        "ELEGOO Saturn 3",
        "Standard Plant-based Photopolymer Resin",
        "",
        "Translucent",
        "25-30",
        "2.5-3",
        "0.05",
        "3+4",
        "65+180",
        "180+65",
        "0.5",
    ],
    [
        "ELEGOO Saturn 3",
        "Standard Plant-based Photopolymer Resin",
        "",
        "Translucent",
        "25-30",
        "3-3.5",
        "0.05",
        "3+4",
        "65+180",
        "180+65",
        "0.5",
    ],
];

/// Write a workbook holding one worksheet of `rows` under the vendor's
/// header, which is the shape this reader is handed, showing each range
/// in `merges` as one cell.
fn workbook(path: &Path, rows: &[[&str; 11]], merges: &[&str]) {
    let mut xml = String::from(r#"<?xml version="1.0"?><worksheet><sheetData>"#);
    let opening = ["The optimal printing environment temperature is 25-30 C."];
    for (index, cells) in std::iter::once(&opening[..])
        .chain(std::iter::once(&HEADER[..]))
        .chain(rows.iter().map(|row| &row[..]))
        .enumerate()
    {
        xml.push_str(&format!(r#"<row r="{}">"#, index + 1));
        for (column, cell) in cells.iter().enumerate() {
            xml.push_str(&format!(
                r#"<c r="{}{}" t="inlineStr"><is><t>{}</t></is></c>"#,
                reference(column),
                index + 1,
                escape(cell),
            ));
        }
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData><mergeCells>");
    for merge in merges {
        xml.push_str(&format!(r#"<mergeCell ref="{merge}"/>"#));
    }
    xml.push_str("</mergeCells></worksheet>");

    let mut archive = zip::ZipWriter::new(File::create(path).unwrap());
    archive
        .start_file::<_, ()>(
            "xl/worksheets/sheet1.xml",
            zip::write::FileOptions::default(),
        )
        .unwrap();
    archive.write_all(xml.as_bytes()).unwrap();
    archive.finish().unwrap();
}

/// The letters naming a column, counting from A.
fn reference(column: usize) -> String {
    let mut letters = String::new();
    let mut index = column + 1;
    while index > 0 {
        let remainder = (index - 1) % 26;
        letters.insert(0, char::from(b'A' + u8::try_from(remainder).unwrap()));
        index = (index - 1) / 26;
    }
    letters
}

/// Text as XML content.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Convert a workbook of `rows` into a fresh library, and say what the
/// run reported.
fn convert(rows: &[[&str; 11]]) -> (PathBuf, tempfile::TempDir, Vec<SourceNote>) {
    convert_merged(rows, &[])
}

/// Convert a workbook of `rows` that shows each range in `merges` as one
/// cell.
fn convert_merged(
    rows: &[[&str; 11]],
    merges: &[&str],
) -> (PathBuf, tempfile::TempDir, Vec<SourceNote>) {
    let dir = tempfile::tempdir().unwrap();
    let sheet = dir.path().join("settings.xlsx");
    workbook(&sheet, rows, merges);
    let out = dir.path().join("library");
    let request = ConversionRequest::from_raw(RawCliArgs {
        format: SourceFormat::ElegooSettingsSheet,
        inputs: vec![sheet],
        out: out.clone(),
        with_images: false,
        printer_library: None,
        material_curation: None,
    })
    .unwrap();

    let report = run(&request).unwrap();

    (out, dir, report.notes)
}

/// The printer profile we ship for `id`.
fn bundled_printer(id: &str) -> sc_print_profiles::PrinterProfile {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("profiles")
        .join("printers")
        .join(format!("{id}.toml"));
    load_printer_profile_from_path(&path).unwrap()
}

#[test]
fn a_workbook_of_settings_becomes_resins_that_load_back_and_resolve() {
    let (out, _dir, _notes) = convert(&ROWS);

    let materials = out.join("materials");
    for (file, printer) in [
        ("elegoo-standard-resin-grey-50um.toml", "elegoo-mars-5"),
        ("elegoo-standard-resin-black-50um.toml", "elegoo-mars-5"),
        ("elegoo-abs-like-resin-grey-50um.toml", "elegoo-saturn-4"),
        (
            "elegoo-rapid-standard-photopolymer-resin-grey-100um.toml",
            "elegoo-mars-4-ultra",
        ),
    ] {
        let path = materials.join(file);
        let profile = load_material_profile_from_path(&path)
            .unwrap_or_else(|err| panic!("{file} does not load: {err}"));
        let material = EffectiveMaterial::from(profile);

        resolve(
            &bundled_printer(printer),
            &material,
            &SceneOverrides::default(),
        )
        .unwrap_or_else(|err| panic!("{file} does not resolve on {printer}: {err}"));
    }
}

#[test]
fn one_row_naming_2_machines_calibrates_both_of_them() {
    let (out, _dir, _notes) = convert(&ROWS);

    let profile = load_material_profile_from_path(
        &out.join("materials")
            .join("elegoo-standard-resin-grey-50um.toml"),
    )
    .unwrap();

    for id in ["elegoo-mars-3", "elegoo-mars-3-pro", "elegoo-mars-2-pro"] {
        let printer = sc_print_primitives::PrinterId::new(id).unwrap();
        assert!(
            profile
                .per_printer
                .contains_key(&CalibrationTarget::from_printer(&printer)),
            "{id} has no block",
        );
    }
}

#[test]
fn the_resin_this_sheet_was_read_for_keeps_its_2_travel_stages() {
    let (out, _dir, _notes) = convert(&ROWS);

    let profile = load_material_profile_from_path(
        &out.join("materials")
            .join("elegoo-rapid-standard-photopolymer-resin-grey-100um.toml"),
    )
    .unwrap();

    let printer = sc_print_primitives::PrinterId::new("elegoo-mars-4-ultra").unwrap();
    let block = profile.per_printer[&CalibrationTarget::from_printer(&printer)];
    let second = block.second_travel.expect("the sheet states 2 stages");
    assert_eq!(block.lift_distance_mm.unwrap().value(), 2.0);
    assert_eq!(block.lift_speed_mm_min.unwrap().value(), 300.0);
    assert_eq!(block.retract_speed_mm_min.unwrap().value(), 300.0);
    assert_eq!(second.lift_distance_mm.value(), 1.0);
    assert_eq!(second.lift_speed_mm_min.value(), 1000.0);
    assert_eq!(second.retract_speed_mm_min.value(), 1000.0);
}

#[test]
fn a_span_a_spreadsheet_rewrote_as_a_date_still_reads_as_an_exposure() {
    let (out, _dir, _notes) = convert(&ROWS);

    let profile = load_material_profile_from_path(
        &out.join("materials")
            .join("elegoo-abs-like-resin-grey-50um.toml"),
    )
    .unwrap();

    let printer = sc_print_primitives::PrinterId::new("elegoo-saturn-4").unwrap();
    let block = profile.per_printer[&CalibrationTarget::from_printer(&printer)];
    // The cell holds 45816, which is the span 6-8 the vendor typed.
    assert_eq!(block.normal_exposure_sec.unwrap().value(), 6.0);
}

#[test]
fn the_run_reports_every_row_it_could_not_use() {
    let (_out, _dir, notes) = convert(&ROWS);

    assert!(
        notes.contains(&SourceNote::RowsForAMachineWeDoNotShip {
            model: "ELEGOO Saturn 8K".to_owned(),
            rows: 1,
        }),
        "{notes:?}",
    );
    assert!(
        notes.contains(&SourceNote::RowsStatingNoTravel {
            model: "ELEGOO Mars 5 Ultra".to_owned(),
            rows: 1,
        }),
        "{notes:?}",
    );
    assert!(
        notes.iter().any(|note| matches!(
            note,
            SourceNote::RowsDisagree { material, .. }
                if material.as_str() == "elegoo-standard-plant-based-photopolymer-resin-translucent-50um"
        )),
        "{notes:?}",
    );
    assert!(
        notes
            .iter()
            .any(|note| matches!(note, SourceNote::RowsRestrictedToASpeedMode { rows: 1, .. })),
        "{notes:?}",
    );
}

#[test]
fn a_file_that_is_not_a_workbook_fails_the_run_rather_than_writing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let sheet = dir.path().join("settings.xlsx");
    std::fs::write(&sheet, b"not a workbook").unwrap();
    let request = ConversionRequest::from_raw(RawCliArgs {
        format: SourceFormat::ElegooSettingsSheet,
        inputs: vec![sheet],
        out: dir.path().join("library"),
        with_images: false,
        printer_library: None,
        material_curation: None,
    })
    .unwrap();

    let err = run(&request).unwrap_err();

    assert!(err.to_string().contains("not a spreadsheet"), "{err}");
}

/// Rows the published sheet lays out under merged cells, as its file
/// stores them: rows 272 and 273, whose machine and resin are stated
/// once over both, and rows 512 and 513, whose note is stated once over
/// the section it opens.
const MERGED: [[&str; 11]; 4] = [
    [
        "ELEGOO Mars 4 DLP",
        "Standard Resin",
        "",
        "Translucent",
        "30-35",
        "3-3.5",
        "0.05",
        "2+3",
        "75+230",
        "230+75",
        "1.0",
    ],
    [
        "", "", "", "clear", "35-40", "45816.0", "0.05", "2+3", "75+230", "230+75", "1.0",
    ],
    [
        "ELEGOO Mars 4 Ultra",
        "Standard Resin",
        "NOTE: \nThis parameter works for Standard Resin+ Normal Speed",
        "Grey",
        "25-30",
        "2.5-3",
        "0.05",
        "2+3",
        "75+230",
        "230+75",
        "0.5",
    ],
    [
        "ELEGOO Mars 4 Ultra",
        "Standard Resin",
        "",
        "Black",
        "25-30",
        "3-3.5",
        "0.05",
        "2+3",
        "75+230",
        "230+75",
        "0.5",
    ],
];

/// The ranges those rows are merged over, below the note and the header
/// the workbook opens with.
const MERGES: [&str; 3] = ["A3:A4", "B3:B4", "C5:C6"];

#[test]
fn a_row_under_a_merged_machine_and_resin_calibrates_that_machine() {
    let (out, _dir, notes) = convert_merged(&MERGED, &MERGES);

    let profile = load_material_profile_from_path(
        &out.join("materials")
            .join("elegoo-standard-resin-clear-50um.toml"),
    )
    .unwrap_or_else(|err| panic!("the merged row wrote no resin: {err}"));
    let printer = sc_print_primitives::PrinterId::new("elegoo-mars-4-dlp").unwrap();
    assert!(
        profile
            .per_printer
            .contains_key(&CalibrationTarget::from_printer(&printer)),
        "the merged row calibrates nothing on the Mars 4 DLP",
    );
    assert!(
        !notes
            .iter()
            .any(|note| matches!(note, SourceNote::RowsNamingNoResin { .. })),
        "{notes:?}",
    );
}

#[test]
fn a_note_merged_over_a_section_qualifies_every_row_it_spans() {
    let (_out, _dir, notes) = convert_merged(&MERGED, &MERGES);

    assert!(
        notes.contains(&SourceNote::RowsRestrictedToASpeedMode {
            advice: "NOTE: This parameter works for Standard Resin+ Normal Speed".to_owned(),
            rows: 2,
        }),
        "{notes:?}",
    );
}
