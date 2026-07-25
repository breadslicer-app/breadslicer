/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Disk loading for printer- and material-profile TOML files.
//!
//! Provides three layers of loader functions for both profile
//! types. The path-based loaders take a TOML file path directly.
//! The library-based loaders take a base directory and a profile
//! id; they look up `<base>/printers/<id>.toml` (or
//! `<base>/materials/<id>.toml`). The by-id loaders accept an
//! optional explicit path plus the user library and assets base
//! directories, and try them in order: explicit-path first; on
//! `NotFound` from the library, cascade to the assets directory;
//! return [`LoadPrinterProfileByIdError::NotFoundAnywhere`]
//! listing every path that was tried if no source had the profile.
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
/// when a reserved section is present).
pub fn load_material_profile_from_path(
    path: &Path,
) -> Result<MaterialProfile, LoadMaterialProfileError> {
    let contents = fs::read_to_string(path).map_err(|err| LoadMaterialProfileError {
        path: path.to_owned(),
        kind: LoadMaterialProfileErrorKind::Io(err),
    })?;
    toml_edit::de::from_str(&contents).map_err(|err| LoadMaterialProfileError {
        path: path.to_owned(),
        kind: LoadMaterialProfileErrorKind::TomlParse(err),
    })
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
    let path = library_dir
        .join(PRINTERS_SUBDIR)
        .join(format!("{}.toml", id.as_str()));
    load_printer_profile_from_path(&path)
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

/// Returned by [`load_printer_profile_by_id`] when no source had
/// the profile or one of the sources had it but failed to load.
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
                    "printer profile not found in any of the configured sources: ",
                )?;
                let paths: Vec<String> =
                    attempted.iter().map(|p| p.display().to_string()).collect();
                write!(f, "{}", paths.join(", "))
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

/// Returned by [`load_material_profile_by_id`]. Same shape as
/// [`LoadPrinterProfileByIdError`].
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
                    "material profile not found in any of the configured sources: ",
                )?;
                let paths: Vec<String> =
                    attempted.iter().map(|p| p.display().to_string()).collect();
                write!(f, "{}", paths.join(", "))
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

/// Load a printer profile by id, trying sources in order:
/// `explicit_path` first (hard error on any failure if provided),
/// then `<library_dir>/printers/<id>.toml`, then
/// `<assets_dir>/printers/<id>.toml`. `NotFound` on a library or
/// assets source cascades to the next; any other failure halts and
/// returns [`LoadPrinterProfileByIdError::Source`]. If every source
/// returned `NotFound`,
/// [`LoadPrinterProfileByIdError::NotFoundAnywhere`] lists each
/// attempted path.
pub fn load_printer_profile_by_id(
    id: &PrinterId,
    explicit_path: Option<&Path>,
    library_dir: &Path,
    assets_dir: &Path,
) -> Result<PrinterProfile, LoadPrinterProfileByIdError> {
    if let Some(path) = explicit_path {
        return load_printer_profile_from_path(path).map_err(LoadPrinterProfileByIdError::Source);
    }

    let mut attempted = Vec::with_capacity(2);
    for base in [library_dir, assets_dir] {
        let path = base
            .join(PRINTERS_SUBDIR)
            .join(format!("{}.toml", id.as_str()));
        match load_printer_profile_from_path(&path) {
            Ok(profile) => return Ok(profile),
            Err(err) => {
                if matches!(
                    &err.kind,
                    LoadPrinterProfileErrorKind::Io(io_err) if io_err.kind() == io::ErrorKind::NotFound,
                ) {
                    attempted.push(path);
                    continue;
                }
                return Err(LoadPrinterProfileByIdError::Source(err));
            }
        }
    }

    Err(LoadPrinterProfileByIdError::NotFoundAnywhere { attempted })
}

/// Load a material profile by id. Same source-cascade semantics as
/// [`load_printer_profile_by_id`].
pub fn load_material_profile_by_id(
    id: &MaterialId,
    explicit_path: Option<&Path>,
    library_dir: &Path,
    assets_dir: &Path,
) -> Result<MaterialProfile, LoadMaterialProfileByIdError> {
    if let Some(path) = explicit_path {
        return load_material_profile_from_path(path).map_err(LoadMaterialProfileByIdError::Source);
    }

    let mut attempted = Vec::with_capacity(2);
    for base in [library_dir, assets_dir] {
        let path = base
            .join(MATERIALS_SUBDIR)
            .join(format!("{}.toml", id.as_str()));
        match load_material_profile_from_path(&path) {
            Ok(profile) => return Ok(profile),
            Err(err) => {
                if matches!(
                    &err.kind,
                    LoadMaterialProfileErrorKind::Io(io_err) if io_err.kind() == io::ErrorKind::NotFound,
                ) {
                    attempted.push(path);
                    continue;
                }
                return Err(LoadMaterialProfileByIdError::Source(err));
            }
        }
    }

    Err(LoadMaterialProfileByIdError::NotFoundAnywhere { attempted })
}
