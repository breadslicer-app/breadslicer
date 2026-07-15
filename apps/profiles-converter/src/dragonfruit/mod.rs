/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The DragonFruit source-format adapter.
//!
//! [`DragonFruitReader`] implements the [`ProfileReader`](crate::reader::ProfileReader)
//! port for DragonFruit's plugin JSON: it reads each input (a file, or a
//! directory of `.json` files), deserialises the records, and maps them
//! into our domain profiles. This is the only module that knows the
//! DragonFruit schema; the rest of the crate stays format-agnostic.

pub mod material;
pub mod printer;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::profile::ConvertedProfile;
use crate::reader::ProfileReader;
use material::{MaterialMapError, RawMaterial, map_material};
use printer::{PrinterMapError, RawPrinter, map_printer};

/// Reads DragonFruit plugin JSON into our domain profiles.
#[derive(Clone, Copy, Debug, Default)]
pub struct DragonFruitReader;

impl DragonFruitReader {
    pub fn new() -> Self {
        Self
    }
}

/// Everything that can go wrong reading DragonFruit inputs.
#[derive(Debug)]
pub enum DragonFruitError {
    /// A file could not be read.
    ReadFile { path: PathBuf, source: io::Error },
    /// A directory could not be listed.
    ReadDir { path: PathBuf, source: io::Error },
    /// A file's bytes were not valid DragonFruit JSON.
    ParseJson {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// A printer record could not be mapped to a domain profile.
    MapPrinter {
        path: PathBuf,
        index: usize,
        source: PrinterMapError,
    },
    /// A material record could not be mapped to a domain profile.
    MapMaterial {
        path: PathBuf,
        index: usize,
        source: MaterialMapError,
    },
    /// A file's records are neither printers nor materials.
    UnrecognisedRecord { path: PathBuf },
}

impl fmt::Display for DragonFruitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFile { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            Self::ReadDir { path, source } => {
                write!(f, "failed to list directory {}: {source}", path.display())
            }
            Self::ParseJson { path, source } => write!(
                f,
                "failed to parse {} as DragonFruit JSON: {source}",
                path.display(),
            ),
            Self::MapPrinter {
                path,
                index,
                source,
            } => write!(f, "{}: record #{index}: {source}", path.display()),
            Self::MapMaterial {
                path,
                index,
                source,
            } => write!(f, "{}: record #{index}: {source}", path.display()),
            Self::UnrecognisedRecord { path } => write!(
                f,
                "{}: records are neither printers nor materials",
                path.display(),
            ),
        }
    }
}

impl std::error::Error for DragonFruitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadFile { source, .. } => Some(source),
            Self::ReadDir { source, .. } => Some(source),
            Self::ParseJson { source, .. } => Some(source),
            Self::MapPrinter { source, .. } => Some(source),
            Self::MapMaterial { source, .. } => Some(source),
            Self::UnrecognisedRecord { .. } => None,
        }
    }
}

impl ProfileReader for DragonFruitReader {
    type Error = DragonFruitError;

    fn read(&self, inputs: &[&Path]) -> Result<Vec<ConvertedProfile>, DragonFruitError> {
        let mut profiles = Vec::new();
        for input in inputs {
            for file in json_files(input)? {
                read_file(&file, &mut profiles)?;
            }
        }
        Ok(profiles)
    }
}

/// Resolve one input path to the JSON files it contributes: a directory
/// yields its `.json` entries (sorted for a deterministic order); a file
/// yields itself.
fn json_files(path: &Path) -> Result<Vec<PathBuf>, DragonFruitError> {
    if !path.is_dir() {
        return Ok(vec![path.to_owned()]);
    }
    let mut files = Vec::new();
    let entries = fs::read_dir(path).map_err(|source| DragonFruitError::ReadDir {
        path: path.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| DragonFruitError::ReadDir {
            path: path.to_owned(),
            source,
        })?;
        let entry_path = entry.path();
        if entry_path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            files.push(entry_path);
        }
    }
    files.sort();
    Ok(files)
}

/// Which DragonFruit record kind a file holds.
enum RecordKind {
    Printer,
    Material,
}

/// Parse one DragonFruit file (an array of records), classify it by the
/// schema of its records, and append the converted profiles. An empty
/// array contributes nothing; a file whose records are neither kind is
/// an error.
fn read_file(path: &Path, profiles: &mut Vec<ConvertedProfile>) -> Result<(), DragonFruitError> {
    let bytes = fs::read(path).map_err(|source| DragonFruitError::ReadFile {
        path: path.to_owned(),
        source,
    })?;
    let values: Vec<Value> =
        serde_json::from_slice(&bytes).map_err(|source| DragonFruitError::ParseJson {
            path: path.to_owned(),
            source,
        })?;
    let kind = match values.first() {
        None => return Ok(()),
        Some(value) if value.get("presetId").is_some() => RecordKind::Printer,
        Some(value) if value.get("templateId").is_some() => RecordKind::Material,
        Some(_) => {
            return Err(DragonFruitError::UnrecognisedRecord {
                path: path.to_owned(),
            });
        }
    };

    match kind {
        RecordKind::Printer => {
            let records: Vec<RawPrinter> = parse_records(&bytes, path)?;
            for (index, record) in records.into_iter().enumerate() {
                let profile =
                    map_printer(record).map_err(|source| DragonFruitError::MapPrinter {
                        path: path.to_owned(),
                        index,
                        source,
                    })?;
                profiles.push(ConvertedProfile::Printer(profile));
            }
        }
        RecordKind::Material => {
            let records: Vec<RawMaterial> = parse_records(&bytes, path)?;
            for (index, record) in records.into_iter().enumerate() {
                let profile =
                    map_material(record).map_err(|source| DragonFruitError::MapMaterial {
                        path: path.to_owned(),
                        index,
                        source,
                    })?;
                profiles.push(ConvertedProfile::Material(profile));
            }
        }
    }
    Ok(())
}

/// Deserialise a file's bytes into a typed record array, tagging a
/// failure with the file path.
fn parse_records<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    path: &Path,
) -> Result<Vec<T>, DragonFruitError> {
    serde_json::from_slice(bytes).map_err(|source| DragonFruitError::ParseJson {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn record(preset_id: &str) -> String {
        format!(
            r#"{{
                "presetId": "{preset_id}", "profileVersion": 1,
                "manufacturer": "TestCorp", "family": "Series", "name": "{preset_id}",
                "buildVolumeMm": {{ "width": 100.0, "depth": 100.0, "height": 40.0 }},
                "pixelSize": {{ "x": 100.0, "y": 100.0 }},
                "display": {{ "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }}
            }}"#
        )
    }

    fn ids(profiles: &[ConvertedProfile]) -> Vec<String> {
        profiles
            .iter()
            .map(|profile| match profile {
                ConvertedProfile::Printer(printer) => printer.identity.id.as_str().to_owned(),
                ConvertedProfile::Material(_) => panic!("expected only printers"),
            })
            .collect()
    }

    #[test]
    fn reads_all_records_from_a_single_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("printers.json");
        fs::write(
            &file,
            format!("[{}, {}]", record("printer-a"), record("printer-b")),
        )
        .unwrap();

        let profiles = DragonFruitReader::new().read(&[file.as_path()]).unwrap();
        assert_eq!(ids(&profiles), vec!["printer-a", "printer-b"]);
    }

    #[test]
    fn walks_a_directory_of_json_files_in_order_and_ignores_others() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.json"),
            format!("[{}]", record("printer-a")),
        )
        .unwrap();
        fs::write(
            dir.path().join("b.json"),
            format!("[{}]", record("printer-b")),
        )
        .unwrap();
        fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

        let profiles = DragonFruitReader::new().read(&[dir.path()]).unwrap();
        assert_eq!(ids(&profiles), vec!["printer-a", "printer-b"]);
    }

    #[test]
    fn malformed_json_is_reported_with_its_path() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("broken.json");
        fs::write(&file, "{ not valid ]").unwrap();

        let err = DragonFruitReader::new()
            .read(&[file.as_path()])
            .unwrap_err();
        assert!(matches!(err, DragonFruitError::ParseJson { .. }));
        assert!(err.to_string().contains("broken.json"));
    }

    #[test]
    fn a_record_that_cannot_map_is_reported_with_its_index() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("printers.json");
        // Second record has an invalid preset id.
        fs::write(
            &file,
            format!("[{}, {}]", record("printer-a"), record("Bad_Id")),
        )
        .unwrap();

        let err = DragonFruitReader::new()
            .read(&[file.as_path()])
            .unwrap_err();
        match err {
            DragonFruitError::MapPrinter { index, .. } => assert_eq!(index, 1),
            other => panic!("expected MapPrinter, got {other}"),
        }
    }

    fn material_record(template_id: &str) -> String {
        format!(
            r#"{{
                "templateId": "{template_id}", "profileVersion": 1,
                "name": "{template_id}", "brand": "TestResin", "resinFamily": "standard",
                "layerHeightMm": 0.05, "normalExposureSec": 2.4, "bottomExposureSec": 26.0,
                "bottomLayerCount": 5, "liftDistanceMm": 6.0, "liftSpeedMmMin": 65.0,
                "retractSpeedMmMin": 150.0
            }}"#
        )
    }

    #[test]
    fn a_file_of_material_records_is_classified_as_materials() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("materials.json");
        fs::write(&file, format!("[{}]", material_record("standard-405"))).unwrap();

        let profiles = DragonFruitReader::new().read(&[file.as_path()]).unwrap();
        match profiles.as_slice() {
            [ConvertedProfile::Material(material)] => {
                assert_eq!(material.identity.id.as_str(), "standard-405");
            }
            other => panic!("expected one material, got {other:?}"),
        }
    }

    #[test]
    fn a_directory_mixing_printer_and_material_files_yields_both_kinds() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("printers.json"),
            format!("[{}]", record("printer-a")),
        )
        .unwrap();
        fs::write(
            dir.path().join("materials.json"),
            format!("[{}]", material_record("resin-a")),
        )
        .unwrap();

        let profiles = DragonFruitReader::new().read(&[dir.path()]).unwrap();
        let printers = profiles
            .iter()
            .filter(|p| matches!(p, ConvertedProfile::Printer(_)))
            .count();
        let materials = profiles
            .iter()
            .filter(|p| matches!(p, ConvertedProfile::Material(_)))
            .count();
        assert_eq!((printers, materials), (1, 1));
    }

    #[test]
    fn records_that_are_neither_kind_are_rejected() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("mystery.json");
        fs::write(&file, r#"[{ "somethingElse": true }]"#).unwrap();

        let err = DragonFruitReader::new()
            .read(&[file.as_path()])
            .unwrap_err();
        assert!(matches!(err, DragonFruitError::UnrecognisedRecord { .. }));
    }

    #[test]
    fn an_empty_array_contributes_nothing() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("empty.json");
        fs::write(&file, "[]").unwrap();

        let profiles = DragonFruitReader::new().read(&[file.as_path()]).unwrap();
        assert!(profiles.is_empty());
    }
}
