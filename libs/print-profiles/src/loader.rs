/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Disk loading for printer- and material-profile TOML files.
//!
//! Provides explicit-path loader functions for both profile types.
//! Each loader reads the file, runs it through the typed
//! deserialisation path (which enforces field invariants and
//! rejects reserved sections), and returns either a typed value
//! or a focused error that carries the path that failed plus the
//! underlying cause.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::material_profile::MaterialProfile;
use crate::printer_profile::PrinterProfile;

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
