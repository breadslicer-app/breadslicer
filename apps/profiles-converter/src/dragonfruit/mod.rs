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

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sc_print_primitives::{MaterialId, PrinterId};
use sc_print_profiles::MaterialProfile;
use serde_json::Value;

use crate::profile::{ConvertedPrinter, ConvertedProfile};
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
    /// Two material records share an id but cannot be merged.
    ConflictingMaterial {
        id: String,
        conflict: MaterialConflict,
    },
    /// Two printer records describe one machine but disagree on more
    /// than the container it writes.
    ConflictingPrinter { id: String },
}

/// Why two material records that share an id could not be merged.
#[derive(Clone, Copy, Debug)]
pub enum MaterialConflict {
    /// They disagree on the resin's shared identity, intent, or defaults.
    SharedFields,
    /// They give different settings for a printer they both target.
    PrinterSettings,
}

impl fmt::Display for MaterialConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SharedFields => {
                write!(f, "records disagree on the resin's shared fields")
            }
            Self::PrinterSettings => write!(
                f,
                "records give different settings for a printer they both target",
            ),
        }
    }
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
            Self::ConflictingMaterial { id, conflict } => {
                write!(f, "material {id:?}: {conflict}")
            }
            Self::ConflictingPrinter { id } => write!(
                f,
                "printer {id:?}: records describe one machine but disagree beyond the container it writes",
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
            Self::ConflictingMaterial { .. } => None,
            Self::ConflictingPrinter { .. } => None,
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
        merge_records(profiles)
    }
}

/// Fold records that describe the same thing into one profile.
///
/// DragonFruit ships a single resin as several records, one per target
/// printer and often in separate files (the same resin calibrated for
/// each of a family's printer variants). Each record maps to a
/// material with a single `per_printer` entry; this pass combines records
/// with the same id into one profile that carries every per-printer
/// block. Records that share an id but disagree on the resin's shared
/// fields, or on the settings for a printer they both target, are a
/// conflict rather than a silent overwrite.
///
/// It keys its presets by output container too, so one machine arrives
/// once per container it writes. Those records describe the same
/// machine, so they fold into one profile that declares every one of
/// those containers.
fn merge_records(
    profiles: Vec<ConvertedProfile>,
) -> Result<Vec<ConvertedProfile>, DragonFruitError> {
    let mut printers: BTreeMap<PrinterId, ConvertedPrinter> = BTreeMap::new();
    let mut materials: BTreeMap<MaterialId, MaterialProfile> = BTreeMap::new();
    for profile in profiles {
        match profile {
            ConvertedProfile::Printer(incoming) => {
                match printers.entry(incoming.profile.identity.id.clone()) {
                    Entry::Vacant(slot) => {
                        slot.insert(incoming);
                    }
                    Entry::Occupied(mut slot) => merge_printer_into(slot.get_mut(), incoming)?,
                }
            }
            ConvertedProfile::Material(incoming) => {
                match materials.entry(incoming.identity.id.clone()) {
                    Entry::Vacant(slot) => {
                        slot.insert(incoming);
                    }
                    Entry::Occupied(mut slot) => merge_material_into(slot.get_mut(), incoming)?,
                }
            }
        }
    }
    let mut merged: Vec<ConvertedProfile> = printers
        .into_values()
        .map(ConvertedProfile::Printer)
        .collect();
    merged.extend(materials.into_values().map(ConvertedProfile::Material));
    Ok(merged)
}

/// Merge `incoming` into `existing`, which name one machine, or report
/// them as a conflict.
///
/// The containers a machine writes are the one field 2 records for it
/// are expected to differ in; anything else means the 2 records are not
/// describing the same machine after all.
fn merge_printer_into(
    existing: &mut ConvertedPrinter,
    incoming: ConvertedPrinter,
) -> Result<(), DragonFruitError> {
    let mut probe = existing.profile.clone();
    probe.capabilities.output_formats = incoming.profile.capabilities.output_formats.clone();
    if probe != incoming.profile {
        return Err(DragonFruitError::ConflictingPrinter {
            id: existing.profile.identity.id.as_str().to_owned(),
        });
    }
    for binding in incoming.profile.capabilities.output_formats {
        if !existing
            .profile
            .capabilities
            .output_formats
            .iter()
            .any(|held| *held == binding)
        {
            existing.profile.capabilities.output_formats.push(binding);
        }
    }
    if existing.image_source.is_none() {
        existing.image_source = incoming.image_source;
    }
    Ok(())
}

/// Merge `incoming` into `existing`, which share a material id, or report
/// the first disagreement.
fn merge_material_into(
    existing: &mut MaterialProfile,
    incoming: MaterialProfile,
) -> Result<(), DragonFruitError> {
    if existing.identity != incoming.identity
        || existing.intent != incoming.intent
        || existing.defaults != incoming.defaults
    {
        return Err(DragonFruitError::ConflictingMaterial {
            id: existing.identity.id.as_str().to_owned(),
            conflict: MaterialConflict::SharedFields,
        });
    }
    for (printer_id, settings) in incoming.per_printer {
        match existing.per_printer.entry(printer_id) {
            Entry::Vacant(slot) => {
                slot.insert(settings);
            }
            Entry::Occupied(slot) => {
                if *slot.get() != settings {
                    return Err(DragonFruitError::ConflictingMaterial {
                        id: existing.identity.id.as_str().to_owned(),
                        conflict: MaterialConflict::PrinterSettings,
                    });
                }
            }
        }
    }
    Ok(())
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
            let base = path.parent().unwrap_or_else(|| Path::new(""));
            let records: Vec<RawPrinter> = parse_records(&bytes, path)?;
            for (index, record) in records.into_iter().enumerate() {
                let mapped =
                    map_printer(record).map_err(|source| DragonFruitError::MapPrinter {
                        path: path.to_owned(),
                        index,
                        source,
                    })?;
                let image_source = mapped
                    .image_ref
                    .as_deref()
                    .map(|reference| resolve_image(base, reference));
                profiles.push(ConvertedProfile::Printer(ConvertedPrinter {
                    profile: mapped.profile,
                    image_source,
                }));
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

/// Resolve a record's image reference against the directory of the file
/// it came from, stripping a leading `./`. An absolute reference is
/// returned unchanged.
fn resolve_image(base: &Path, reference: &str) -> PathBuf {
    let relative = reference.strip_prefix("./").unwrap_or(reference);
    base.join(relative)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sc_print_primitives::FormatKind;
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
                ConvertedProfile::Printer(printer) => {
                    printer.profile.identity.id.as_str().to_owned()
                }
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
        assert_eq!(
            ids(&profiles),
            vec!["testcorp-printer-a", "testcorp-printer-b"],
        );
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
        assert_eq!(
            ids(&profiles),
            vec!["testcorp-printer-a", "testcorp-printer-b"],
        );
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
        // The second record names no manufacturer, so no machine.
        let nameless = r#"{
            "presetId": "printer-b", "profileVersion": 1,
            "manufacturer": "", "family": "Series", "name": "printer-b",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        fs::write(&file, format!("[{}, {nameless}]", record("printer-a"))).unwrap();

        let err = DragonFruitReader::new()
            .read(&[file.as_path()])
            .unwrap_err();
        match err {
            DragonFruitError::MapPrinter { index, .. } => assert_eq!(index, 1),
            other => panic!("expected MapPrinter, got {other}"),
        }
    }

    /// The same machine as the source ships it for 2 containers: one
    /// preset per container, identical in everything else.
    fn record_for_container(container: &str) -> String {
        format!(
            r#"{{
                "presetId": "elegoo-saturn-4-{container}", "profileVersion": 1,
                "manufacturer": "ELEGOO", "family": "Saturn", "name": "Saturn 4",
                "buildVolumeMm": {{ "width": 100.0, "depth": 100.0, "height": 40.0 }},
                "pixelSize": {{ "x": 100.0, "y": 100.0 }},
                "display": {{
                    "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".{container}"
                }}
            }}"#
        )
    }

    #[test]
    fn one_machine_shipped_per_container_becomes_one_profile_declaring_both() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("printers.json");
        fs::write(
            &file,
            format!(
                "[{}, {}]",
                record_for_container("goo"),
                record_for_container("ctb"),
            ),
        )
        .unwrap();

        let profiles = DragonFruitReader::new().read(&[file.as_path()]).unwrap();

        assert_eq!(ids(&profiles), vec!["elegoo-saturn-4"]);
        let ConvertedProfile::Printer(printer) = &profiles[0] else {
            panic!("expected a printer");
        };
        let declared: Vec<FormatKind> = printer
            .profile
            .capabilities
            .output_formats
            .iter()
            .map(|binding| binding.format.clone())
            .collect();
        assert_eq!(declared, vec![FormatKind::Goo, FormatKind::Ctb]);
    }

    #[test]
    fn two_machines_of_one_name_that_differ_in_hardware_are_a_conflict() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("printers.json");
        let taller = record_for_container("ctb").replace("\"height\": 40.0", "\"height\": 90.0");
        fs::write(
            &file,
            format!("[{}, {taller}]", record_for_container("goo")),
        )
        .unwrap();

        let err = DragonFruitReader::new()
            .read(&[file.as_path()])
            .unwrap_err();

        assert!(
            matches!(&err, DragonFruitError::ConflictingPrinter { id } if id == "elegoo-saturn-4"),
            "err: {err}",
        );
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

    #[test]
    fn resolves_the_image_path_against_the_files_directory() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("printers.json");
        let json = r#"[{
            "presetId": "img", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Img",
            "imageAssetPath": "./assets/img.png",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }]"#;
        fs::write(&file, json).unwrap();

        let profiles = DragonFruitReader::new().read(&[file.as_path()]).unwrap();
        match profiles.as_slice() {
            [ConvertedProfile::Printer(printer)] => {
                assert_eq!(
                    printer.image_source,
                    Some(dir.path().join("assets/img.png")),
                );
            }
            other => panic!("expected one printer, got {other:?}"),
        }
    }

    fn material_for(template_id: &str, name: &str, preset: &str, normal_exposure: f64) -> String {
        format!(
            r#"{{
                "templateId": "{template_id}", "profileVersion": 1,
                "name": "{name}", "brand": "TestResin", "resinFamily": "standard",
                "validForPresets": ["{preset}"],
                "layerHeightMm": 0.05, "normalExposureSec": {normal_exposure},
                "bottomExposureSec": 26.0, "bottomLayerCount": 5, "liftDistanceMm": 6.0,
                "liftSpeedMmMin": 65.0, "retractSpeedMmMin": 150.0
            }}"#
        )
    }

    #[test]
    fn material_records_sharing_an_id_merge_their_per_printer_blocks() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.json"),
            format!("[{}]", material_for("blu", "Blu", "printer-a", 2.4)),
        )
        .unwrap();
        fs::write(
            dir.path().join("b.json"),
            format!("[{}]", material_for("blu", "Blu", "printer-b", 2.4)),
        )
        .unwrap();

        let profiles = DragonFruitReader::new().read(&[dir.path()]).unwrap();
        match profiles.as_slice() {
            [ConvertedProfile::Material(material)] => {
                assert_eq!(material.identity.id.as_str(), "blu");
                let printers: Vec<&str> =
                    material.per_printer.keys().map(|id| id.as_str()).collect();
                assert_eq!(printers, vec!["printer-a", "printer-b"]);
            }
            other => panic!("expected one merged material, got {other:?}"),
        }
    }

    #[test]
    fn materials_sharing_an_id_but_disagreeing_on_shared_fields_conflict() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.json"),
            format!("[{}]", material_for("blu", "Blu", "printer-a", 2.4)),
        )
        .unwrap();
        fs::write(
            dir.path().join("b.json"),
            format!("[{}]", material_for("blu", "Renamed", "printer-b", 2.4)),
        )
        .unwrap();

        let err = DragonFruitReader::new().read(&[dir.path()]).unwrap_err();
        assert!(matches!(
            err,
            DragonFruitError::ConflictingMaterial {
                conflict: MaterialConflict::SharedFields,
                ..
            },
        ));
    }

    #[test]
    fn materials_disagreeing_on_a_shared_printer_block_conflict() {
        let dir = tempdir().unwrap();
        // Same id, same shared fields, same target printer, but the
        // per-printer exposure differs.
        fs::write(
            dir.path().join("a.json"),
            format!("[{}]", material_for("blu", "Blu", "printer-a", 2.4)),
        )
        .unwrap();
        fs::write(
            dir.path().join("b.json"),
            format!("[{}]", material_for("blu", "Blu", "printer-a", 3.0)),
        )
        .unwrap();

        let err = DragonFruitReader::new().read(&[dir.path()]).unwrap_err();
        assert!(matches!(
            err,
            DragonFruitError::ConflictingMaterial {
                conflict: MaterialConflict::PrinterSettings,
                ..
            },
        ));
    }
}
