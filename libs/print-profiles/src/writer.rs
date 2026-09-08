/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Disk writing for printer- and material-profile TOML files.
//!
//! [`save_printer_profile_to_path`] and
//! [`save_material_profile_to_path`] serialise a [`PrinterProfile`] or
//! [`MaterialProfile`] to the given TOML file, creating the parent
//! directory when missing. [`save_printer_profile_to_library`] and
//! [`save_material_profile_to_library`] derive the destination from a
//! library directory instead: `<library_dir>/printers/<id>.toml` or
//! `<library_dir>/materials/<id>.toml`. Written files lay every nested
//! struct out as a `[section]` table and load back equal through the
//! loaders in [`crate::loader`].
//!
//! [`save_material_extension_to_library`] places extension-flagged
//! profiles at `<library_dir>/materials/extensions/<id>.toml` and
//! annotates their empty `[intent]` / `[defaults]` sections with a
//! comment explaining that the values come from the parent profile.
//!
//! [`save_printer_image_to_library`] copies a printer's preview image
//! to `<library_dir>/images/<id>.<ext>` and returns the
//! [`ImageAssetRef`] to store in the profile's `image_asset_path`.
//!
//! [`delete_material_profile_from_library`] and
//! [`delete_material_extension_from_library`] remove one material
//! file or one extension file by id. A missing file is reported as
//! a `NotFound` I/O error.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sc_print_primitives::{MaterialId, PrinterId};

use crate::image_asset_ref::ImageAssetRef;
use crate::loader::{
    IMAGES_SUBDIR, material_extension_path_in_library, material_profile_path_in_library,
    printer_profile_path_in_library,
};
use crate::material_profile::MaterialProfile;
use crate::printer_profile::PrinterProfile;

/// Returned by the material-profile writers when serialisation or the
/// write fails. The path field names the file that was being written;
/// the kind field carries the underlying cause.
#[derive(Debug)]
pub struct SaveMaterialProfileError {
    pub path: PathBuf,
    pub kind: SaveMaterialProfileErrorKind,
}

#[derive(Debug)]
pub enum SaveMaterialProfileErrorKind {
    Io(io::Error),
    TomlSerialize(toml_edit::ser::Error),
}

impl fmt::Display for SaveMaterialProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to save material profile to {}: {}",
            self.path.display(),
            self.kind,
        )
    }
}

impl fmt::Display for SaveMaterialProfileErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::TomlSerialize(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SaveMaterialProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            SaveMaterialProfileErrorKind::Io(err) => Some(err),
            SaveMaterialProfileErrorKind::TomlSerialize(err) => Some(err),
        }
    }
}

/// Serialise a value to the on-disk profile dialect: every nested
/// struct as a `[section]` table.
fn sectioned_toml<T: serde::Serialize>(value: &T) -> Result<String, toml_edit::ser::Error> {
    toml_edit::ser::to_string_pretty(value)
}

/// Serialise `profile` to TOML and write it to `path`, creating the
/// parent directory when it does not exist. Returns
/// [`SaveMaterialProfileError`] when serialisation or the write fails.
pub fn save_material_profile_to_path(
    path: &Path,
    profile: &MaterialProfile,
) -> Result<(), SaveMaterialProfileError> {
    let contents = sectioned_toml(profile).map_err(|err| SaveMaterialProfileError {
        path: path.to_owned(),
        kind: SaveMaterialProfileErrorKind::TomlSerialize(err),
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| SaveMaterialProfileError {
            path: parent.to_owned(),
            kind: SaveMaterialProfileErrorKind::Io(err),
        })?;
    }
    fs::write(path, contents).map_err(|err| SaveMaterialProfileError {
        path: path.to_owned(),
        kind: SaveMaterialProfileErrorKind::Io(err),
    })
}

/// Write `profile` into a library directory at
/// `<library_dir>/materials/<id>.toml`, creating the `materials/`
/// subdirectory when absent. Delegates to
/// [`save_material_profile_to_path`].
pub fn save_material_profile_to_library(
    library_dir: &Path,
    profile: &MaterialProfile,
) -> Result<(), SaveMaterialProfileError> {
    let path = material_profile_path_in_library(library_dir, &profile.identity.id);
    save_material_profile_to_path(&path, profile)
}

/// Returned by the printer-profile writers when serialisation or the
/// write fails.
#[derive(Debug)]
pub struct SavePrinterProfileError {
    /// Names the file that was being written
    pub path: PathBuf,
    /// Carries the underlying cause
    pub kind: SavePrinterProfileErrorKind,
}

#[derive(Debug)]
pub enum SavePrinterProfileErrorKind {
    Io(io::Error),
    TomlSerialize(toml_edit::ser::Error),
}

impl fmt::Display for SavePrinterProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to save printer profile to {}: {}",
            self.path.display(),
            self.kind,
        )
    }
}

impl fmt::Display for SavePrinterProfileErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::TomlSerialize(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SavePrinterProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            SavePrinterProfileErrorKind::Io(err) => Some(err),
            SavePrinterProfileErrorKind::TomlSerialize(err) => Some(err),
        }
    }
}

/// Serialise `profile` to TOML and write it to `path`, creating the
/// parent directory when it does not exist. Returns
/// [`SavePrinterProfileError`] when serialisation or the write fails.
pub fn save_printer_profile_to_path(
    path: &Path,
    profile: &PrinterProfile,
) -> Result<(), SavePrinterProfileError> {
    let contents = sectioned_toml(profile).map_err(|err| SavePrinterProfileError {
        path: path.to_owned(),
        kind: SavePrinterProfileErrorKind::TomlSerialize(err),
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| SavePrinterProfileError {
            path: parent.to_owned(),
            kind: SavePrinterProfileErrorKind::Io(err),
        })?;
    }
    fs::write(path, contents).map_err(|err| SavePrinterProfileError {
        path: path.to_owned(),
        kind: SavePrinterProfileErrorKind::Io(err),
    })
}

/// Write `profile` into a library directory at
/// `<library_dir>/printers/<id>.toml`, creating the `printers/`
/// subdirectory when absent. Delegates to
/// [`save_printer_profile_to_path`].
pub fn save_printer_profile_to_library(
    library_dir: &Path,
    profile: &PrinterProfile,
) -> Result<(), SavePrinterProfileError> {
    let path = printer_profile_path_in_library(library_dir, &profile.identity.id);
    save_printer_profile_to_path(&path, profile)
}

/// Returned by [`save_printer_image_to_library`] when the images
/// directory cannot be created or the source cannot be copied.
#[derive(Debug)]
pub struct SavePrinterImageError {
    /// Names the destination that was being written
    pub path: PathBuf,
    /// Carries the underlying cause
    pub kind: SavePrinterImageErrorKind,
}

#[derive(Debug)]
pub enum SavePrinterImageErrorKind {
    CreateDir(io::Error),
    /// Copying from the named source failed (missing or unreadable
    /// source, or unwritable destination).
    Copy {
        from: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for SavePrinterImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to save printer image to {}: {}",
            self.path.display(),
            self.kind,
        )
    }
}

impl fmt::Display for SavePrinterImageErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDir(err) => write!(f, "cannot create the images directory: {err}"),
            Self::Copy { from, source } => {
                write!(f, "cannot copy {}: {source}", from.display())
            }
        }
    }
}

impl std::error::Error for SavePrinterImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            SavePrinterImageErrorKind::CreateDir(err) => Some(err),
            SavePrinterImageErrorKind::Copy { source, .. } => Some(source),
        }
    }
}

/// Copy the image at `source` into a library directory as
/// `<library_dir>/images/<id>.<ext>`, creating the `images/`
/// subdirectory when absent, and return the reference to store in the
/// printer's `image_asset_path`. The extension is the source's,
/// lowercased; a source without one is stored as `.png`.
pub fn save_printer_image_to_library(
    library_dir: &Path,
    id: &PrinterId,
    source: &Path,
) -> Result<ImageAssetRef, SavePrinterImageError> {
    let ext = source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "png".to_owned());
    let file_name = format!("{}.{ext}", id.as_str());
    let dir = library_dir.join(IMAGES_SUBDIR);
    let dest = dir.join(&file_name);
    fs::create_dir_all(&dir).map_err(|err| SavePrinterImageError {
        path: dest.clone(),
        kind: SavePrinterImageErrorKind::CreateDir(err),
    })?;
    fs::copy(source, &dest).map_err(|err| SavePrinterImageError {
        path: dest.clone(),
        kind: SavePrinterImageErrorKind::Copy {
            from: source.to_owned(),
            source: err,
        },
    })?;
    Ok(ImageAssetRef::under_subdir(IMAGES_SUBDIR, &file_name))
}

/// Returned by [`save_material_extension_to_library`] when the profile
/// is not flagged as an extension, or when serialisation or the write
/// fails. The path field names the file that was being written.
#[derive(Debug)]
pub struct SaveMaterialExtensionError {
    pub path: PathBuf,
    pub kind: SaveMaterialExtensionErrorKind,
}

#[derive(Debug)]
pub enum SaveMaterialExtensionErrorKind {
    /// The profile's `extension` flag is false; writing it into the
    /// extensions directory would misfile a full profile.
    NotAnExtension,
    Io(io::Error),
    TomlSerialize(toml_edit::ser::Error),
    /// Reparsing the serialised profile to attach the section
    /// comments failed.
    TomlReparse(toml_edit::TomlError),
}

impl fmt::Display for SaveMaterialExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to save material extension to {}: {}",
            self.path.display(),
            self.kind,
        )
    }
}

impl fmt::Display for SaveMaterialExtensionErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnExtension => {
                write!(f, "the profile's extension flag is false")
            }
            Self::Io(err) => write!(f, "{err}"),
            Self::TomlSerialize(err) => write!(f, "{err}"),
            Self::TomlReparse(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SaveMaterialExtensionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            SaveMaterialExtensionErrorKind::NotAnExtension => None,
            SaveMaterialExtensionErrorKind::Io(err) => Some(err),
            SaveMaterialExtensionErrorKind::TomlSerialize(err) => Some(err),
            SaveMaterialExtensionErrorKind::TomlReparse(err) => Some(err),
        }
    }
}

/// Comment attached above the empty inherited sections of a written
/// extension file.
const INHERITED_SECTION_COMMENT: &str = "# Values are inherited from the parent profile.";

/// Write an extension profile into a library directory at
/// `<library_dir>/materials/extensions/<id>.toml`, creating the
/// subdirectory when absent. Rejects profiles whose `extension` flag
/// is false. When the `[intent]`, `[defaults]` or `[physical]`
/// section is empty, a comment explaining that the values are
/// inherited from the parent profile is attached above it.
pub fn save_material_extension_to_library(
    library_dir: &Path,
    profile: &MaterialProfile,
) -> Result<(), SaveMaterialExtensionError> {
    let path = material_extension_path_in_library(library_dir, &profile.identity.id);
    if !profile.extension {
        return Err(SaveMaterialExtensionError {
            path,
            kind: SaveMaterialExtensionErrorKind::NotAnExtension,
        });
    }
    // Attaching comments needs an editable document. ser::to_document yields
    // one directly but in the inline-table shape; only to_string_pretty emits
    // sections, hence serialise-then-reparse.
    let serialised = sectioned_toml(profile).map_err(|err| SaveMaterialExtensionError {
        path: path.clone(),
        kind: SaveMaterialExtensionErrorKind::TomlSerialize(err),
    })?;
    let mut doc: toml_edit::DocumentMut =
        serialised
            .parse()
            .map_err(|err| SaveMaterialExtensionError {
                path: path.clone(),
                kind: SaveMaterialExtensionErrorKind::TomlReparse(err),
            })?;
    annotate_inherited_section(&mut doc, "intent");
    annotate_inherited_section(&mut doc, "defaults");
    annotate_inherited_section(&mut doc, "physical");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| SaveMaterialExtensionError {
            path: parent.to_owned(),
            kind: SaveMaterialExtensionErrorKind::Io(err),
        })?;
    }
    fs::write(&path, doc.to_string()).map_err(|err| SaveMaterialExtensionError {
        path,
        kind: SaveMaterialExtensionErrorKind::Io(err),
    })
}

/// Attach the inherited-values comment above the named section when
/// the section is an empty table, keeping whatever whitespace prefix
/// the serialiser produced.
fn annotate_inherited_section(doc: &mut toml_edit::DocumentMut, key: &str) {
    if let Some(table) = doc.get_mut(key).and_then(|item| item.as_table_mut())
        && table.is_empty()
    {
        let existing = table
            .decor()
            .prefix()
            .and_then(|prefix| prefix.as_str())
            .unwrap_or("")
            .to_owned();
        table
            .decor_mut()
            .set_prefix(format!("{existing}{INHERITED_SECTION_COMMENT}\n"));
    }
}

/// Returned by the delete operations when the file at `path` could not
/// be removed, including when it does not exist.
#[derive(Debug)]
pub struct DeleteMaterialFileError {
    /// Names the file that was being removed
    pub path: PathBuf,
    /// Carries the underlying cause
    pub source: io::Error,
}

impl fmt::Display for DeleteMaterialFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to delete {}: {}",
            self.path.display(),
            self.source,
        )
    }
}

impl std::error::Error for DeleteMaterialFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Remove the full material profile `<library_dir>/materials/<id>.toml`.
/// Leaves any extension of the same id in place.
pub fn delete_material_profile_from_library(
    library_dir: &Path,
    id: &MaterialId,
) -> Result<(), DeleteMaterialFileError> {
    let path = material_profile_path_in_library(library_dir, id);
    fs::remove_file(&path).map_err(|source| DeleteMaterialFileError { path, source })
}

/// Remove the material extension
/// `<library_dir>/materials/extensions/<id>.toml`. Leaves the full
/// profile of the same id in place.
pub fn delete_material_extension_from_library(
    library_dir: &Path,
    id: &MaterialId,
) -> Result<(), DeleteMaterialFileError> {
    let path = material_extension_path_in_library(library_dir, id);
    fs::remove_file(&path).map_err(|source| DeleteMaterialFileError { path, source })
}
