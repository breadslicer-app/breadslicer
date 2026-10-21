/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Disk loading for printer- and material-profile TOML files.
//!
//! Provides 2 layers of loader functions for both profile
//! types. The path-based loaders take a TOML file path directly.
//! The library-based loaders take a base directory and a profile
//! id; they look up `<base>/printers/<id>.toml` (or
//! `<base>/materials/<id>.toml`). Resolution by id across several
//! libraries belongs to [`ProfileLibraries`](crate::ProfileLibraries).
//!
//! Each loader runs the file contents through the typed
//! deserialisation path (which enforces field invariants and
//! rejects reserved sections) and returns either a typed value or
//! a focused error that carries the path that failed plus the
//! underlying cause.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sc_print_primitives::{MaterialId, PrinterId};

use crate::material_profile::MaterialProfile;
use crate::printer_profile::PrinterProfile;

pub(crate) const PRINTERS_SUBDIR: &str = "printers";
pub(crate) const MATERIALS_SUBDIR: &str = "materials";
pub(crate) const MATERIAL_EXTENSIONS_SUBDIR: &str = "extensions";
pub(crate) const IMAGES_SUBDIR: &str = "images";

/// Directory holding material extension profiles:
/// `<library_dir>/materials/extensions/`. Extensions live apart from
/// full profiles so the by-id material loaders never resolve them.
pub(crate) fn material_extensions_dir(library_dir: &Path) -> PathBuf {
    library_dir
        .join(MATERIALS_SUBDIR)
        .join(MATERIAL_EXTENSIONS_SUBDIR)
}

/// Path of a printer profile inside a library directory:
/// `<library_dir>/printers/<id>.toml`. Purely a path computation; the
/// file may or may not exist.
pub fn printer_profile_path_in_library(library_dir: &Path, id: &PrinterId) -> PathBuf {
    library_dir
        .join(PRINTERS_SUBDIR)
        .join(format!("{}.toml", id.as_str()))
}

/// Path of a material profile inside a library directory:
/// `<library_dir>/materials/<id>.toml`. Purely a path computation; the
/// file may or may not exist.
pub fn material_profile_path_in_library(library_dir: &Path, id: &MaterialId) -> PathBuf {
    library_dir
        .join(MATERIALS_SUBDIR)
        .join(format!("{}.toml", id.as_str()))
}

/// Path of a material extension inside a library directory:
/// `<library_dir>/materials/extensions/<id>.toml`. Purely a path
/// computation; the file may or may not exist.
pub fn material_extension_path_in_library(library_dir: &Path, id: &MaterialId) -> PathBuf {
    material_extensions_dir(library_dir).join(format!("{}.toml", id.as_str()))
}

/// Returned by [`load_printer_profile_from_path`] when the read or
/// the parse fails. The path field names the file that was being
/// loaded; the kind field carries the underlying cause.
#[derive(Debug)]
pub struct LoadPrinterProfileError {
    pub path: PathBuf,
    pub kind: LoadPrinterProfileErrorKind,
}

#[derive(Debug)]
pub enum LoadPrinterProfileErrorKind {
    Io(io::Error),
    TomlParse(toml_edit::de::Error),
}

impl fmt::Display for LoadPrinterProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to load printer profile from {}: {}",
            self.path.display(),
            self.kind,
        )
    }
}

impl fmt::Display for LoadPrinterProfileErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::TomlParse(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LoadPrinterProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            LoadPrinterProfileErrorKind::Io(err) => Some(err),
            LoadPrinterProfileErrorKind::TomlParse(err) => Some(err),
        }
    }
}

/// Returned by [`load_material_profile_from_path`] when the read or
/// the parse fails. Same shape as
/// [`LoadPrinterProfileError`].
#[derive(Debug)]
pub struct LoadMaterialProfileError {
    pub path: PathBuf,
    pub kind: LoadMaterialProfileErrorKind,
}

#[derive(Debug)]
pub enum LoadMaterialProfileErrorKind {
    Io(io::Error),
    TomlParse(toml_edit::de::Error),
}

impl fmt::Display for LoadMaterialProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to load material profile from {}: {}",
            self.path.display(),
            self.kind,
        )
    }
}

impl fmt::Display for LoadMaterialProfileErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::TomlParse(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LoadMaterialProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            LoadMaterialProfileErrorKind::Io(err) => Some(err),
            LoadMaterialProfileErrorKind::TomlParse(err) => Some(err),
        }
    }
}

/// Read a TOML printer profile from the given path. Returns
/// [`LoadPrinterProfileError`] when the file cannot be read or the
/// contents do not parse as a valid [`PrinterProfile`] (including
/// when a reserved section is present).
pub fn load_printer_profile_from_path(
    path: &Path,
) -> Result<PrinterProfile, LoadPrinterProfileError> {
    let contents = fs::read_to_string(path).map_err(|err| LoadPrinterProfileError {
        path: path.to_owned(),
        kind: LoadPrinterProfileErrorKind::Io(err),
    })?;
    toml_edit::de::from_str(&contents).map_err(|err| LoadPrinterProfileError {
        path: path.to_owned(),
        kind: LoadPrinterProfileErrorKind::TomlParse(err),
    })
}

/// Read a TOML material profile from the given path. Returns
/// [`LoadMaterialProfileError`] when the file cannot be read or the
/// contents do not parse as a valid [`MaterialProfile`] (including
/// when a reserved section is present). The comment lines above each
/// calibration block become that block's note.
pub fn load_material_profile_from_path(
    path: &Path,
) -> Result<MaterialProfile, LoadMaterialProfileError> {
    let contents = fs::read_to_string(path).map_err(|err| LoadMaterialProfileError {
        path: path.to_owned(),
        kind: LoadMaterialProfileErrorKind::Io(err),
    })?;
    let parse_error = |err: toml_edit::de::Error| LoadMaterialProfileError {
        path: path.to_owned(),
        kind: LoadMaterialProfileErrorKind::TomlParse(err),
    };
    let mut profile: MaterialProfile = toml_edit::de::from_str(&contents).map_err(parse_error)?;
    let document: toml_edit::DocumentMut = contents
        .parse()
        .map_err(|err: toml_edit::TomlError| parse_error(err.into()))?;
    profile.notes = crate::calibration_notes::read(&document);
    Ok(profile)
}

/// Load a printer profile from a library directory by id. Resolves
/// to `<library_dir>/printers/<id>.toml` and delegates to
/// [`load_printer_profile_from_path`]. The library directory is
/// either the platform-resolved user library (see
/// [`crate::library::user_library_dir`]) or a workspace assets
/// directory.
pub fn load_printer_profile_from_library(
    library_dir: &Path,
    id: &PrinterId,
) -> Result<PrinterProfile, LoadPrinterProfileError> {
    load_printer_profile_from_path(&printer_profile_path_in_library(library_dir, id))
}

/// Load a material profile from a library directory by id. Resolves
/// to `<library_dir>/materials/<id>.toml` and delegates to
/// [`load_material_profile_from_path`].
pub fn load_material_profile_from_library(
    library_dir: &Path,
    id: &MaterialId,
) -> Result<MaterialProfile, LoadMaterialProfileError> {
    load_material_profile_from_path(&material_profile_path_in_library(library_dir, id))
}

/// Returned by [`load_material_extension_from_library`] when the file
/// cannot be loaded as a material profile or loads fine but does not
/// carry the extension flag.
#[derive(Debug)]
pub enum LoadMaterialExtensionError {
    /// The file failed to load or parse as a material profile.
    Profile(LoadMaterialProfileError),
    /// The file parsed as a material profile but its `extension` flag
    /// is false, so it is a full profile misplaced in the extensions
    /// directory.
    NotAnExtension { path: PathBuf },
}

impl fmt::Display for LoadMaterialExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(err) => err.fmt(f),
            Self::NotAnExtension { path } => write!(
                f,
                "the file {} is not a material extension: its extension flag is missing or false",
                path.display(),
            ),
        }
    }
}

impl std::error::Error for LoadMaterialExtensionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Profile(err) => Some(err),
            Self::NotAnExtension { .. } => None,
        }
    }
}

/// Load a material extension from a library directory by id. Resolves
/// to `<library_dir>/materials/extensions/<id>.toml`, delegates to
/// [`load_material_profile_from_path`], and rejects files whose
/// `extension` flag is false.
pub fn load_material_extension_from_library(
    library_dir: &Path,
    id: &MaterialId,
) -> Result<MaterialProfile, LoadMaterialExtensionError> {
    let path = material_extension_path_in_library(library_dir, id);
    let profile =
        load_material_profile_from_path(&path).map_err(LoadMaterialExtensionError::Profile)?;
    if !profile.extension {
        return Err(LoadMaterialExtensionError::NotAnExtension { path });
    }
    Ok(profile)
}

/// Load the document in a library's extensions slot for this id, if
/// one is there: `Ok(None)` when the file does not exist, so callers
/// never match on [`io::ErrorKind`] themselves. The `extension` flag
/// is deliberately NOT validated here; the semantic checks belong to
/// [`EffectiveMaterial::compose`](crate::EffectiveMaterial::compose),
/// which reports a flag or identity problem as an issue instead of an
/// error.
pub fn load_material_extension_if_present(
    library_dir: &Path,
    id: &MaterialId,
) -> Result<Option<MaterialProfile>, LoadMaterialProfileError> {
    let path = material_extension_path_in_library(library_dir, id);
    match load_material_profile_from_path(&path) {
        Ok(profile) => Ok(Some(profile)),
        Err(err) => {
            if matches!(
                &err.kind,
                LoadMaterialProfileErrorKind::Io(io_err) if io_err.kind() == io::ErrorKind::NotFound,
            ) {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

/// Returned by
/// [`ProfileLibraries::load_printer_by_id`](crate::ProfileLibraries::load_printer_by_id)
/// when no source had the profile or one of the sources had it but
/// failed to load.
#[derive(Debug)]
pub enum LoadPrinterProfileByIdError {
    /// A source produced the file but loading or parsing it failed.
    /// Wraps the focused error from
    /// [`load_printer_profile_from_path`].
    Source(LoadPrinterProfileError),
    /// Every source returned `NotFound`. The attempted paths are
    /// listed in the order they were tried.
    NotFoundAnywhere { attempted: Vec<PathBuf> },
}

impl fmt::Display for LoadPrinterProfileByIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(err) => err.fmt(f),
            Self::NotFoundAnywhere { attempted } => {
                write!(
                    f,
                    "printer profile not found in any of the configured sources"
                )?;
                if attempted.is_empty() {
                    return write!(f, " (no profile library roots configured)");
                }
                let paths: Vec<String> =
                    attempted.iter().map(|p| p.display().to_string()).collect();
                write!(f, ": {}", paths.join(", "))
            }
        }
    }
}

impl std::error::Error for LoadPrinterProfileByIdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(err) => Some(err),
            Self::NotFoundAnywhere { .. } => None,
        }
    }
}

/// Returned by
/// [`ProfileLibraries::load_material_by_id`](crate::ProfileLibraries::load_material_by_id).
/// Same shape as [`LoadPrinterProfileByIdError`].
#[derive(Debug)]
pub enum LoadMaterialProfileByIdError {
    Source(LoadMaterialProfileError),
    NotFoundAnywhere { attempted: Vec<PathBuf> },
}

impl fmt::Display for LoadMaterialProfileByIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(err) => err.fmt(f),
            Self::NotFoundAnywhere { attempted } => {
                write!(
                    f,
                    "material profile not found in any of the configured sources"
                )?;
                if attempted.is_empty() {
                    return write!(f, " (no profile library roots configured)");
                }
                let paths: Vec<String> =
                    attempted.iter().map(|p| p.display().to_string()).collect();
                write!(f, ": {}", paths.join(", "))
            }
        }
    }
}

impl std::error::Error for LoadMaterialProfileByIdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(err) => Some(err),
            Self::NotFoundAnywhere { .. } => None,
        }
    }
}
