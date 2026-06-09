/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Packaging of a profile library into a portable zip archive.
//!
//! [`export_package`] reads a library directory and writes a zip that
//! mirrors the library layout: a top-level `manifest.toml` for human
//! inspection plus `printers/<id>.toml` and `materials/<id>.toml`
//! entries for the selected profiles. The selection is one of: every
//! profile, printers only, materials only, or an explicit set of ids
//! resolved against both kinds.
//!
//! The export is clock-free: the caller supplies the `exported_at`
//! timestamp (and the binary version) so the function stays pure and
//! testable. The manifest is a courtesy for whoever inspects the
//! archive; the import path ignores it.

use std::fmt;
use std::fs;
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

use sc_print_primitives::{InvalidProfileId, MaterialId, PrinterId};
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::listing::{ListLibraryError, list_material_ids, list_printer_ids};
use crate::loader::{MATERIALS_SUBDIR, PRINTERS_SUBDIR};

/// Which profiles an [`export_package`] call writes into the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportSelection {
    /// Every printer and material in the library.
    All,
    /// Every printer in the library; no materials.
    Printers,
    /// Every material in the library; no printers.
    Materials,
    /// Exactly the listed ids, each resolved against both kinds. An id
    /// that names a printer file, a material file, or both is included
    /// for whichever kind(s) it matches; an id matching neither is an
    /// error.
    Ids(Vec<String>),
}

/// Caller-supplied `[package]` fields for the archive manifest. The
/// `[contents]` lists are derived from what is actually exported and
/// are not part of this input. `name`, `description`, and `author`
/// are optional and omitted from the manifest when absent.
#[derive(Clone, Debug)]
pub struct ExportManifest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    /// Export time, formatted by the caller (RFC3339 is conventional).
    pub exported_at: String,
    /// Version of the binary that produced the archive.
    pub sc_slicer_version: String,
}

/// Returned by [`export_package`] when the selection cannot be
/// resolved, a profile file cannot be read, or the archive cannot be
/// written.
#[derive(Debug)]
pub enum ExportError {
    /// An id in an [`ExportSelection::Ids`] selection is not a valid
    /// profile slug.
    InvalidId {
        id: String,
        source: InvalidProfileId,
    },
    /// An id in an [`ExportSelection::Ids`] selection matched neither a
    /// printer nor a material file in the library.
    UnknownId { id: String },
    /// Listing a library subdirectory failed.
    List(ListLibraryError),
    /// A selected profile file could not be read for inclusion.
    ReadProfile { path: PathBuf, source: io::Error },
    /// The manifest could not be serialised to TOML.
    SerializeManifest(toml_edit::ser::Error),
    /// The zip archive could not be written.
    Zip(zip::result::ZipError),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId { id, source } => {
                write!(f, "invalid profile id {id:?}: {source}")
            }
            Self::UnknownId { id } => write!(
                f,
                "profile id {id:?} matched no printer or material in the library",
            ),
            Self::List(err) => write!(f, "{err}"),
            Self::ReadProfile { path, source } => write!(
                f,
                "failed to read profile {} for export: {source}",
                path.display(),
            ),
            Self::SerializeManifest(err) => {
                write!(f, "failed to serialise the package manifest: {err}")
            }
            Self::Zip(err) => write!(f, "failed to write the package archive: {err}"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidId { source, .. } => Some(source),
            Self::UnknownId { .. } => None,
            Self::List(err) => Some(err),
            Self::ReadProfile { source, .. } => Some(source),
            Self::SerializeManifest(err) => Some(err),
            Self::Zip(err) => Some(err),
        }
    }
}

/// Manifest name written at the archive root.
const MANIFEST_FILENAME: &str = "manifest.toml";

/// Write the selected profiles from `library_dir` into `writer` as a
/// zip archive. The archive mirrors the library layout and carries a
/// courtesy `manifest.toml`. Profiles are written as their raw on-disk
/// bytes (the export does not re-serialise them).
pub fn export_package<W: Write + Seek>(
    library_dir: &Path,
    selection: &ExportSelection,
    manifest_fields: &ExportManifest,
    writer: W,
) -> Result<(), ExportError> {
    let (printers, materials) = resolve_selection(library_dir, selection)?;

    let mut zip = ZipWriter::new(writer);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let manifest = Manifest {
        package: ManifestPackage {
            name: manifest_fields.name.as_deref(),
            description: manifest_fields.description.as_deref(),
            author: manifest_fields.author.as_deref(),
            exported_at: &manifest_fields.exported_at,
            sc_slicer_version: &manifest_fields.sc_slicer_version,
        },
        contents: ManifestContents {
            printers: printers.iter().map(|id| id.as_str().to_owned()).collect(),
            materials: materials.iter().map(|id| id.as_str().to_owned()).collect(),
        },
    };
    let manifest_toml =
        toml_edit::ser::to_string(&manifest).map_err(ExportError::SerializeManifest)?;
    write_entry(
        &mut zip,
        MANIFEST_FILENAME,
        options,
        manifest_toml.as_bytes(),
    )?;

    for id in &printers {
        let path = library_dir
            .join(PRINTERS_SUBDIR)
            .join(format!("{}.toml", id.as_str()));
        let bytes = fs::read(&path).map_err(|source| ExportError::ReadProfile {
            path: path.clone(),
            source,
        })?;
        write_entry(
            &mut zip,
            &format!("{PRINTERS_SUBDIR}/{}.toml", id.as_str()),
            options,
            &bytes,
        )?;
    }
    for id in &materials {
        let path = library_dir
            .join(MATERIALS_SUBDIR)
            .join(format!("{}.toml", id.as_str()));
        let bytes = fs::read(&path).map_err(|source| ExportError::ReadProfile {
            path: path.clone(),
            source,
        })?;
        write_entry(
            &mut zip,
            &format!("{MATERIALS_SUBDIR}/{}.toml", id.as_str()),
            options,
            &bytes,
        )?;
    }

    zip.finish().map_err(ExportError::Zip)?;
    Ok(())
}

/// Resolve the selection into sorted, de-duplicated lists of the
/// printer and material ids to export.
fn resolve_selection(
    library_dir: &Path,
    selection: &ExportSelection,
) -> Result<(Vec<PrinterId>, Vec<MaterialId>), ExportError> {
    match selection {
        ExportSelection::All => Ok((
            list_printer_ids(library_dir).map_err(ExportError::List)?,
            list_material_ids(library_dir).map_err(ExportError::List)?,
        )),
        ExportSelection::Printers => Ok((
            list_printer_ids(library_dir).map_err(ExportError::List)?,
            Vec::new(),
        )),
        ExportSelection::Materials => Ok((
            Vec::new(),
            list_material_ids(library_dir).map_err(ExportError::List)?,
        )),
        ExportSelection::Ids(ids) => resolve_ids(library_dir, ids),
    }
}

/// Resolve explicit ids against both kinds. Each id is validated as a
/// slug (which also guarantees it cannot escape the library directory),
/// then matched against `printers/<id>.toml` and `materials/<id>.toml`.
fn resolve_ids(
    library_dir: &Path,
    ids: &[String],
) -> Result<(Vec<PrinterId>, Vec<MaterialId>), ExportError> {
    let mut printers = Vec::new();
    let mut materials = Vec::new();
    for raw in ids {
        let printer_id = PrinterId::new(raw).map_err(|source| ExportError::InvalidId {
            id: raw.clone(),
            source,
        })?;
        // The two id kinds share one slug grammar, so this construction
        // cannot fail once the printer id is built; the `?` keeps the
        // code panic-free without an unreachable branch.
        let material_id = MaterialId::new(raw).map_err(|source| ExportError::InvalidId {
            id: raw.clone(),
            source,
        })?;

        let mut matched = false;
        if library_dir
            .join(PRINTERS_SUBDIR)
            .join(format!("{}.toml", printer_id.as_str()))
            .is_file()
        {
            printers.push(printer_id);
            matched = true;
        }
        if library_dir
            .join(MATERIALS_SUBDIR)
            .join(format!("{}.toml", material_id.as_str()))
            .is_file()
        {
            materials.push(material_id);
            matched = true;
        }
        if !matched {
            return Err(ExportError::UnknownId { id: raw.clone() });
        }
    }
    printers.sort();
    printers.dedup();
    materials.sort();
    materials.dedup();
    Ok((printers, materials))
}

fn write_entry<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    name: &str,
    options: SimpleFileOptions,
    bytes: &[u8],
) -> Result<(), ExportError> {
    zip.start_file(name, options).map_err(ExportError::Zip)?;
    zip.write_all(bytes)
        .map_err(|err| ExportError::Zip(err.into()))?;
    Ok(())
}

/// On-disk manifest shape. Borrows from [`ExportManifest`] and the
/// resolved id lists so serialisation does not clone the package
/// fields.
#[derive(serde::Serialize)]
struct Manifest<'a> {
    package: ManifestPackage<'a>,
    contents: ManifestContents,
}

#[derive(serde::Serialize)]
struct ManifestPackage<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<&'a str>,
    exported_at: &'a str,
    sc_slicer_version: &'a str,
}

#[derive(serde::Serialize)]
struct ManifestContents {
    printers: Vec<String>,
    materials: Vec<String>,
}
