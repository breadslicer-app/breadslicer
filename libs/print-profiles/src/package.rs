/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Packaging of a profile library into a portable zip archive.
//!
//! [`export_package`] reads a library directory and writes a zip that
//! mirrors the library layout: a top-level `manifest.toml` for human
//! inspection plus `printers/<id>.toml`, `materials/<id>.toml`, and
//! `materials/extensions/<id>.toml` entries for the selected
//! profiles. The selection is one of: every profile, printers only,
//! materials only, or an explicit set of ids resolved against every
//! kind. Material extensions ride along with the materials: they are
//! included whenever materials are.
//!
//! The export is clock-free: the caller supplies the `exported_at`
//! timestamp (and the binary version) so the function stays pure and
//! testable. The manifest is a courtesy for whoever inspects the
//! archive; the import path ignores it.

use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use sc_print_primitives::{InvalidProfileId, MaterialId, PrinterId};
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

use crate::listing::{
    ListLibraryError, list_material_extension_ids, list_material_ids, list_printer_ids,
};
use crate::loader::{MATERIALS_SUBDIR, PRINTERS_SUBDIR, material_extensions_dir};

/// Zip-entry directory for material extension profiles. Mirrors the
/// on-disk `materials/extensions/` layout; zip entry names always use
/// forward slashes.
const MATERIAL_EXTENSIONS_PACKAGE_DIR: &str = "materials/extensions";

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
    let SelectedIds {
        printers,
        materials,
        material_extensions,
    } = resolve_selection(library_dir, selection)?;

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
            material_extensions: material_extensions
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
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
    for id in &material_extensions {
        let path = material_extensions_dir(library_dir).join(format!("{}.toml", id.as_str()));
        let bytes = fs::read(&path).map_err(|source| ExportError::ReadProfile {
            path: path.clone(),
            source,
        })?;
        write_entry(
            &mut zip,
            &format!("{MATERIAL_EXTENSIONS_PACKAGE_DIR}/{}.toml", id.as_str()),
            options,
            &bytes,
        )?;
    }

    zip.finish().map_err(ExportError::Zip)?;
    Ok(())
}

/// Sorted, de-duplicated ids a selection resolved to, one list per
/// profile kind.
struct SelectedIds {
    printers: Vec<PrinterId>,
    materials: Vec<MaterialId>,
    material_extensions: Vec<MaterialId>,
}

/// Resolve the selection into the ids to export.
fn resolve_selection(
    library_dir: &Path,
    selection: &ExportSelection,
) -> Result<SelectedIds, ExportError> {
    match selection {
        ExportSelection::All => Ok(SelectedIds {
            printers: list_printer_ids(library_dir).map_err(ExportError::List)?,
            materials: list_material_ids(library_dir).map_err(ExportError::List)?,
            material_extensions: list_material_extension_ids(library_dir)
                .map_err(ExportError::List)?,
        }),
        ExportSelection::Printers => Ok(SelectedIds {
            printers: list_printer_ids(library_dir).map_err(ExportError::List)?,
            materials: Vec::new(),
            material_extensions: Vec::new(),
        }),
        ExportSelection::Materials => Ok(SelectedIds {
            printers: Vec::new(),
            materials: list_material_ids(library_dir).map_err(ExportError::List)?,
            material_extensions: list_material_extension_ids(library_dir)
                .map_err(ExportError::List)?,
        }),
        ExportSelection::Ids(ids) => resolve_ids(library_dir, ids),
    }
}

/// Resolve explicit ids against every kind. Each id is validated as a
/// slug (which also guarantees it cannot escape the library directory),
/// then matched against `printers/<id>.toml`, `materials/<id>.toml`,
/// and `materials/extensions/<id>.toml`.
fn resolve_ids(library_dir: &Path, ids: &[String]) -> Result<SelectedIds, ExportError> {
    let mut printers = Vec::new();
    let mut materials = Vec::new();
    let mut material_extensions = Vec::new();
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
            materials.push(material_id.clone());
            matched = true;
        }
        if material_extensions_dir(library_dir)
            .join(format!("{}.toml", material_id.as_str()))
            .is_file()
        {
            material_extensions.push(material_id);
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
    material_extensions.sort();
    material_extensions.dedup();
    Ok(SelectedIds {
        printers,
        materials,
        material_extensions,
    })
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

/// A profile in the package that would overwrite an existing file in
/// the destination library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collision {
    pub id: String,
    pub library_path: PathBuf,
}

/// Every collision an import would cause, grouped by kind. Returned
/// inside [`ImportError::Collisions`] when collisions exist and the
/// caller did not pass `force`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionReport {
    pub printers: Vec<Collision>,
    pub materials: Vec<Collision>,
    pub material_extensions: Vec<Collision>,
}

/// Returned by [`import_package`] when the archive cannot be read, the
/// import would overwrite existing profiles without `force`, or a
/// profile cannot be written to the destination.
#[derive(Debug)]
pub enum ImportError {
    /// The archive could not be opened or an entry header read.
    Archive(zip::result::ZipError),
    /// An entry's bytes could not be decompressed.
    ReadEntry { name: String, source: io::Error },
    /// The import would overwrite existing profiles and `force` was
    /// not set. No files were written.
    Collisions(CollisionReport),
    /// A destination subdirectory could not be created.
    CreateSubdir { path: PathBuf, source: io::Error },
    /// A profile file could not be written to the destination.
    WriteProfile { path: PathBuf, source: io::Error },
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(err) => write!(f, "failed to read the package archive: {err}"),
            Self::ReadEntry { name, source } => {
                write!(f, "failed to read package entry {name:?}: {source}")
            }
            Self::Collisions(report) => write!(
                f,
                "import would overwrite {} existing profile(s)",
                report.printers.len() + report.materials.len() + report.material_extensions.len(),
            ),
            Self::CreateSubdir { path, source } => write!(
                f,
                "failed to create the destination directory {}: {source}",
                path.display(),
            ),
            Self::WriteProfile { path, source } => {
                write!(f, "failed to write profile {}: {source}", path.display(),)
            }
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Archive(err) => Some(err),
            Self::ReadEntry { source, .. } => Some(source),
            Self::Collisions(_) => None,
            Self::CreateSubdir { source, .. } => Some(source),
            Self::WriteProfile { source, .. } => Some(source),
        }
    }
}

/// Which profile kind a package entry belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Printer,
    Material,
    MaterialExtension,
}

impl Kind {
    /// Directory of this kind's entries inside the zip archive.
    fn package_dir(self) -> &'static str {
        match self {
            Self::Printer => PRINTERS_SUBDIR,
            Self::Material => MATERIALS_SUBDIR,
            Self::MaterialExtension => MATERIAL_EXTENSIONS_PACKAGE_DIR,
        }
    }

    /// Directory of this kind's files inside a library directory.
    fn dest_dir(self, library_dir: &Path) -> PathBuf {
        match self {
            Self::Printer => library_dir.join(PRINTERS_SUBDIR),
            Self::Material => library_dir.join(MATERIALS_SUBDIR),
            Self::MaterialExtension => material_extensions_dir(library_dir),
        }
    }
}

/// Install the profiles from a package archive into `dest_library_dir`.
///
/// Only entries shaped exactly like `printers/<slug>.toml`,
/// `materials/<slug>.toml`, or `materials/extensions/<slug>.toml` are
/// considered; the courtesy `manifest.toml` and any other entry
/// (nested paths, non-slug stems) are ignored. Each destination path
/// is rebuilt from the validated slug, so a crafted entry name cannot
/// escape `dest_library_dir`.
///
/// Collisions (a destination file that already exists) are collected
/// across every kind before anything is written. When any exist and
/// `force` is false, [`ImportError::Collisions`] is returned and the
/// destination is left untouched. Otherwise the profiles are written,
/// creating the destination subdirectories as needed.
pub fn import_package<R: Read + Seek>(
    reader: R,
    dest_library_dir: &Path,
    force: bool,
) -> Result<(), ImportError> {
    let mut archive = ZipArchive::new(reader).map_err(ImportError::Archive)?;

    let mut entries: Vec<(Kind, String, usize)> = Vec::new();
    for index in 0..archive.len() {
        let name = archive
            .by_index(index)
            .map_err(ImportError::Archive)?
            .name()
            .to_owned();
        if let Some((kind, id)) = classify_entry(&name) {
            entries.push((kind, id, index));
        }
    }

    if !force {
        let report = collisions(dest_library_dir, &entries);
        if !report.printers.is_empty()
            || !report.materials.is_empty()
            || !report.material_extensions.is_empty()
        {
            return Err(ImportError::Collisions(report));
        }
    }

    for (kind, id, index) in &entries {
        let mut bytes = Vec::new();
        archive
            .by_index(*index)
            .map_err(ImportError::Archive)?
            .read_to_end(&mut bytes)
            .map_err(|source| ImportError::ReadEntry {
                name: format!("{}/{id}.toml", kind.package_dir()),
                source,
            })?;

        let subdir = kind.dest_dir(dest_library_dir);
        fs::create_dir_all(&subdir).map_err(|source| ImportError::CreateSubdir {
            path: subdir.clone(),
            source,
        })?;
        let path = subdir.join(format!("{id}.toml"));
        fs::write(&path, &bytes).map_err(|source| ImportError::WriteProfile {
            path: path.clone(),
            source,
        })?;
    }

    Ok(())
}

/// Collect the destination collisions for the classified entries,
/// sorted by id within each kind.
fn collisions(dest_library_dir: &Path, entries: &[(Kind, String, usize)]) -> CollisionReport {
    let mut printers = Vec::new();
    let mut materials = Vec::new();
    let mut material_extensions = Vec::new();
    for (kind, id, _) in entries {
        let path = kind.dest_dir(dest_library_dir).join(format!("{id}.toml"));
        if path.is_file() {
            let collision = Collision {
                id: id.clone(),
                library_path: path,
            };
            match kind {
                Kind::Printer => printers.push(collision),
                Kind::Material => materials.push(collision),
                Kind::MaterialExtension => material_extensions.push(collision),
            }
        }
    }
    printers.sort_by(|a, b| a.id.cmp(&b.id));
    materials.sort_by(|a, b| a.id.cmp(&b.id));
    material_extensions.sort_by(|a, b| a.id.cmp(&b.id));
    CollisionReport {
        printers,
        materials,
        material_extensions,
    }
}

/// Classify a package entry name. Returns the kind and the validated
/// slug for entries shaped exactly `printers/<slug>.toml`,
/// `materials/<slug>.toml`, or `materials/extensions/<slug>.toml`;
/// everything else yields `None`.
fn classify_entry(name: &str) -> Option<(Kind, String)> {
    if let Some(stem) = entry_stem(name, PRINTERS_SUBDIR)
        && PrinterId::new(stem).is_ok()
    {
        return Some((Kind::Printer, stem.to_owned()));
    }
    if let Some(stem) = entry_stem(name, MATERIAL_EXTENSIONS_PACKAGE_DIR)
        && MaterialId::new(stem).is_ok()
    {
        return Some((Kind::MaterialExtension, stem.to_owned()));
    }
    if let Some(stem) = entry_stem(name, MATERIALS_SUBDIR)
        && MaterialId::new(stem).is_ok()
    {
        return Some((Kind::Material, stem.to_owned()));
    }
    None
}

/// Strip a `<prefix>/` and a trailing `.toml` from an entry name,
/// rejecting names with a further path separator (nested directories).
fn entry_stem<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = name.strip_prefix(prefix)?.strip_prefix('/')?;
    if rest.contains('/') {
        return None;
    }
    rest.strip_suffix(".toml")
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
    material_extensions: Vec<String>,
}
