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

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::loader::{
    material_extension_path_in_library, material_profile_path_in_library,
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

/// Comment attached above the empty `[intent]` and `[defaults]`
/// sections of a written extension file.
const INHERITED_SECTION_COMMENT: &str = "# Values are inherited from the parent profile.";

/// Write an extension profile into a library directory at
/// `<library_dir>/materials/extensions/<id>.toml`, creating the
/// subdirectory when absent. Rejects profiles whose `extension` flag
/// is false. When the `[intent]` or `[defaults]` section is empty, a
/// comment explaining that the values are inherited from the parent
/// profile is attached above it.
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
