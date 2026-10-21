/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The spreadsheet, as rows of text.
//!
//! A workbook is an OPC package: one zip entry holds the cells and
//! another the strings they refer to by index. This module turns the
//! first worksheet into rows of plain text and knows nothing about what
//! any column means.

use std::fmt;
use std::fs::File;
use std::io;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};

/// Where the cells live in the package.
const SHEET_PART: &str = "xl/worksheets/sheet1.xml";
/// Where a cell's text lives when the cell holds an index into it.
const STRINGS_PART: &str = "xl/sharedStrings.xml";

/// One worksheet's cells, row by row, each row indexed by column, as the
/// sheet shows them: a cell under a merged range holds that range's
/// value.
///
/// Row `n` of the sheet sits at index `n - 1`, so a row the file leaves
/// out is an empty row here. A row is as long as its rightmost non-empty
/// cell, so a short row is a row whose trailing cells are empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sheet {
    rows: Vec<Vec<String>>,
}

/// Returned by [`Sheet::open`] when the file is not a workbook this can
/// read.
#[derive(Debug)]
pub enum SheetReadError {
    /// The file could not be opened.
    Open { path: PathBuf, source: io::Error },
    /// It is not a zip archive, so it is not a workbook.
    NotAWorkbook {
        path: PathBuf,
        source: zip::result::ZipError,
    },
    /// It is a zip archive but holds no worksheet where one belongs.
    MissingPart { path: PathBuf, part: &'static str },
    /// A part is not the XML it should be.
    Xml {
        path: PathBuf,
        part: &'static str,
        source: quick_xml::errors::Error,
    },
}

impl fmt::Display for SheetReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { path, source } => {
                write!(f, "failed to open {}: {source}", path.display())
            }
            Self::NotAWorkbook { path, source } => write!(
                f,
                "{} is not a spreadsheet this can read: {source}",
                path.display(),
            ),
            Self::MissingPart { path, part } => write!(
                f,
                "{} holds no {part}, so it carries no worksheet",
                path.display(),
            ),
            Self::Xml { path, part, source } => {
                write!(f, "{} has a malformed {part}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for SheetReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open { source, .. } => Some(source),
            Self::NotAWorkbook { source, .. } => Some(source),
            Self::MissingPart { .. } => None,
            Self::Xml { source, .. } => Some(source),
        }
    }
}

impl Sheet {
    /// Read the first worksheet of the workbook at `path`.
    pub fn open(path: &Path) -> Result<Self, SheetReadError> {
        let file = File::open(path).map_err(|source| SheetReadError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|source| SheetReadError::NotAWorkbook {
                path: path.to_path_buf(),
                source,
            })?;
        let strings = match part(&mut archive, STRINGS_PART) {
            Some(bytes) => shared_strings(&bytes).map_err(|source| SheetReadError::Xml {
                path: path.to_path_buf(),
                part: STRINGS_PART,
                source,
            })?,
            None => Vec::new(),
        };
        let cells = part(&mut archive, SHEET_PART).ok_or(SheetReadError::MissingPart {
            path: path.to_path_buf(),
            part: SHEET_PART,
        })?;
        let rows = worksheet(&cells, &strings).map_err(|source| SheetReadError::Xml {
            path: path.to_path_buf(),
            part: SHEET_PART,
            source,
        })?;
        Ok(Self { rows })
    }

    /// Build one directly from its rows, for tests and fixtures.
    pub fn from_rows(rows: Vec<Vec<String>>) -> Self {
        Self { rows }
    }

    /// Every row, in the order the sheet lists them.
    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }
}

/// The bytes of one part, if the archive holds it.
fn part<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Option<Vec<u8>> {
    let mut entry = archive.by_name(name).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The strings a cell can refer to by index, in index order.
fn shared_strings(bytes: &[u8]) -> Result<Vec<String>, quick_xml::errors::Error> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut strings = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) if e.local_name().as_ref() == b"si" => {
                current.clear();
                depth = 1;
            }
            Event::Text(t) if depth > 0 => current.push_str(&t.decode()?),
            Event::End(e) if e.local_name().as_ref() == b"si" => {
                depth = 0;
                strings.push(std::mem::take(&mut current));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(strings)
}

/// How a cell says its text should be read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CellKind {
    /// The value is an index into the shared strings.
    Shared,
    /// The value is the text itself.
    Literal,
}

/// Read the rows of one worksheet, every cell resolved to its text and
/// every merged range spread over the cells it covers.
fn worksheet(
    bytes: &[u8],
    strings: &[String],
) -> Result<Vec<Vec<String>>, quick_xml::errors::Error> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut merges: Vec<Merge> = Vec::new();
    let mut column = 0usize;
    let mut kind = CellKind::Literal;
    let mut reading_value = false;
    let mut value = String::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"row" => {
                    // The file leaves out rows with nothing in them, and
                    // a merge names rows by number, so the gap is kept.
                    if let Some(number) = attribute(&e, b"r").and_then(|r| r.parse::<usize>().ok())
                    {
                        rows.resize(rows.len().max(number.saturating_sub(1)), Vec::new());
                    }
                    row = Vec::new();
                }
                b"c" => {
                    column = column_of(&e);
                    kind = kind_of(&e);
                    value.clear();
                }
                b"v" | b"t" => reading_value = true,
                b"mergeCell" => {
                    if let Some(merge) = attribute(&e, b"ref").as_deref().and_then(merge_of) {
                        merges.push(merge);
                    }
                }
                _ => {}
            },
            Event::Text(t) if reading_value => value.push_str(&t.decode()?),
            Event::End(e) => match e.local_name().as_ref() {
                b"v" | b"t" => reading_value = false,
                b"c" => place(&mut row, column, resolve(kind, &value, strings)),
                b"row" => rows.push(std::mem::take(&mut row)),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    for merge in &merges {
        spread(&mut rows, merge);
    }
    Ok(rows)
}

/// A range of cells the sheet shows as one, holding the value of its
/// top-left cell. Rows and columns count from 0, both ends included.
struct Merge {
    top: usize,
    left: usize,
    bottom: usize,
    right: usize,
}

/// Read the range an `A1:B2` style reference names.
fn merge_of(reference: &str) -> Option<Merge> {
    let (first, last) = reference.split_once(':')?;
    let (left, top) = cell_at(first)?;
    let (right, bottom) = cell_at(last)?;
    Some(Merge {
        top,
        left,
        bottom,
        right,
    })
}

/// Copy `merge`'s value into every cell it covers, which is how the
/// sheet shows it.
///
/// A range whose first cell is empty is left alone, so no row grows
/// past its last non-empty cell.
fn spread(rows: &mut [Vec<String>], merge: &Merge) {
    let value = match rows.get(merge.top).and_then(|row| row.get(merge.left)) {
        Some(value) if !value.is_empty() => value.clone(),
        _ => return,
    };
    for row in rows.iter_mut().take(merge.bottom + 1).skip(merge.top) {
        for column in merge.left..=merge.right {
            place(row, column, value.clone());
        }
    }
}

/// The text a cell holds, following its own index when it has one.
fn resolve(kind: CellKind, value: &str, strings: &[String]) -> String {
    match kind {
        CellKind::Shared => value
            .parse::<usize>()
            .ok()
            .and_then(|index| strings.get(index))
            .cloned()
            .unwrap_or_default(),
        CellKind::Literal => value.to_owned(),
    }
}

/// Put `value` at `column`, padding the row so the columns line up.
fn place(row: &mut Vec<String>, column: usize, value: String) {
    if row.len() <= column {
        row.resize(column + 1, String::new());
    }
    row[column] = value;
}

/// Which column a cell sits in, from its `A1` style reference.
///
/// Absent or unreadable, it is the next column after the row's last, so
/// a sheet without references still reads left to right.
fn column_of(cell: &BytesStart<'_>) -> usize {
    let reference = attribute(cell, b"r").unwrap_or_default();
    let letters: String = reference
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    column_named(&letters)
}

/// Find the column and row an `A1` style reference names, both counted
/// from 0.
fn cell_at(reference: &str) -> Option<(usize, usize)> {
    let (letters, number) = reference.split_at(reference.find(|c: char| c.is_ascii_digit())?);
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let row = number.parse::<usize>().ok()?.checked_sub(1)?;
    Some((column_named(letters), row))
}

/// Count which column `letters` name, from 0 for `A`.
fn column_named(letters: &str) -> usize {
    letters
        .bytes()
        .fold(0usize, |acc, byte| {
            acc * 26 + usize::from(byte.to_ascii_uppercase() - b'A') + 1
        })
        .saturating_sub(1)
}

/// Read `element`'s attribute `name`, if it carries one.
fn attribute(element: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attr| attr.key.local_name().as_ref() == name)
        .and_then(|attr| String::from_utf8(attr.value.into_owned()).ok())
}

/// Whether a cell's value is its text or an index into the strings.
fn kind_of(cell: &BytesStart<'_>) -> CellKind {
    let is_shared = cell
        .attributes()
        .flatten()
        .find(|attr| attr.key.local_name().as_ref() == b"t")
        .is_some_and(|attr| attr.value.as_ref() == b"s");
    if is_shared {
        CellKind::Shared
    } else {
        CellKind::Literal
    }
}

#[cfg(test)]
mod sheet_tests {
    use super::*;

    const STRINGS: &str = r#"<?xml version="1.0"?>
<sst><si><t>ELEGOO Mars 5</t></si><si><t>Standard Resin</t></si></sst>"#;

    const CELLS: &str = r#"<?xml version="1.0"?>
<worksheet><sheetData>
  <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
  <row r="2"><c r="A2"><v>2.5</v></c><c r="C2"><v>0.05</v></c></row>
</sheetData></worksheet>"#;

    fn rows_of(cells: &str, strings: &str) -> Vec<Vec<String>> {
        let shared = shared_strings(strings.as_bytes()).unwrap();
        worksheet(cells.as_bytes(), &shared).unwrap()
    }

    #[test]
    fn a_cell_that_indexes_the_strings_reads_as_that_string() {
        let rows = rows_of(CELLS, STRINGS);

        assert_eq!(rows[0], vec!["ELEGOO Mars 5", "Standard Resin"]);
    }

    #[test]
    fn a_gap_in_a_row_keeps_the_columns_lined_up() {
        let rows = rows_of(CELLS, STRINGS);

        assert_eq!(rows[1], vec!["2.5", "", "0.05"]);
    }

    #[test]
    fn a_reference_past_the_first_26_columns_still_lands_right() {
        let rows = rows_of(
            r#"<worksheet><sheetData><row><c r="AA1"><v>x</v></c></row></sheetData></worksheet>"#,
            "<sst/>",
        );

        assert_eq!(rows[0].len(), 27);
        assert_eq!(rows[0][26], "x");
    }

    #[test]
    fn an_inline_string_reads_as_its_text() {
        let rows = rows_of(
            r#"<worksheet><sheetData><row><c r="A1" t="inlineStr"><is><t>Grey</t></is></c></row></sheetData></worksheet>"#,
            "<sst/>",
        );

        assert_eq!(rows[0], vec!["Grey"]);
    }

    #[test]
    fn an_index_no_string_answers_to_reads_as_empty() {
        let rows = rows_of(
            r#"<worksheet><sheetData><row><c r="A1" t="s"><v>9</v></c></row></sheetData></worksheet>"#,
            STRINGS,
        );

        assert_eq!(rows[0], vec![""]);
    }

    #[test]
    fn a_merged_cell_reads_as_its_value_in_every_row_it_spans() {
        let rows = rows_of(
            r#"<worksheet><sheetData>
  <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>Grey</v></c></row>
  <row r="2"><c r="B2"><v>Black</v></c></row>
</sheetData><mergeCells count="1"><mergeCell ref="A1:A2"/></mergeCells></worksheet>"#,
            STRINGS,
        );

        assert_eq!(rows[1], vec!["ELEGOO Mars 5", "Black"]);
    }

    #[test]
    fn a_merged_cell_reads_as_its_value_in_every_column_it_spans() {
        let rows = rows_of(
            r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>1</v></c></row></sheetData><mergeCells><mergeCell ref="A1:C1"/></mergeCells></worksheet>"#,
            STRINGS,
        );

        assert_eq!(rows[0], vec!["Standard Resin"; 3]);
    }

    #[test]
    fn a_merge_whose_first_cell_is_empty_leaves_the_rows_it_spans_alone() {
        let rows = rows_of(
            r#"<worksheet><sheetData><row r="1"><c r="A1"><v>x</v></c></row><row r="2"><c r="A2"><v>y</v></c></row></sheetData><mergeCells><mergeCell ref="C1:C2"/></mergeCells></worksheet>"#,
            "<sst/>",
        );

        assert_eq!(rows[1], vec!["y"]);
    }

    #[test]
    fn a_row_the_sheet_leaves_out_keeps_its_place() {
        let rows = rows_of(
            r#"<worksheet><sheetData><row r="1"><c r="A1"><v>1</v></c></row><row r="3"><c r="A3"><v>3</v></c></row></sheetData></worksheet>"#,
            "<sst/>",
        );

        assert_eq!(rows, vec![vec!["1"], vec![], vec!["3"]]);
    }

    #[test]
    fn a_merge_lands_on_the_rows_it_names_when_the_sheet_leaves_some_out() {
        let rows = rows_of(
            r#"<worksheet><sheetData><row r="2"><c r="A2"><v>x</v></c></row><row r="4"><c r="B4"><v>y</v></c></row></sheetData><mergeCells><mergeCell ref="A2:A4"/></mergeCells></worksheet>"#,
            "<sst/>",
        );

        assert_eq!(rows[3], vec!["x", "y"]);
    }

    #[test]
    fn a_reference_reads_as_its_column_and_row_counted_from_0() {
        assert_eq!(cell_at("A1"), Some((0, 0)));
        assert_eq!(cell_at("AA12"), Some((26, 11)));
    }

    #[test]
    fn a_reference_that_names_no_cell_reads_as_none() {
        for reference in ["", "A", "12", "A0", "$A$1", "A-1"] {
            assert_eq!(cell_at(reference), None, "{reference:?}");
        }
        assert!(merge_of("A1").is_none());
        assert!(merge_of("A1:B").is_none());
    }

    #[test]
    fn a_file_that_is_not_a_workbook_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.xlsx");
        std::fs::write(&path, b"not a zip").unwrap();

        let err = Sheet::open(&path).unwrap_err();

        assert!(matches!(err, SheetReadError::NotAWorkbook { .. }), "{err}");
    }

    #[test]
    fn a_file_that_is_not_there_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();

        let err = Sheet::open(&dir.path().join("absent.xlsx")).unwrap_err();

        assert!(matches!(err, SheetReadError::Open { .. }), "{err}");
    }
}
