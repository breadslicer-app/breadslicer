/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Disk writing for material-profile TOML files.
//!
//! Symmetric with the material loaders in [`crate::loader`]: the
//! path-based writer serialises a [`MaterialProfile`] to a TOML file,
//! creating the parent directory when missing; the library-based
//! writer places it at `<library_dir>/materials/<id>.toml`. Both keep
//! the profile-TOML shape owned by this crate rather than hand-rolled
//! by callers. Serialisation uses the same `toml_edit` path the
//! round-trip tests exercise, so a written file loads back equal.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::loader::MATERIALS_SUBDIR;
use crate::material_profile::MaterialProfile;

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

/// Serialise `profile` to TOML and write it to `path`, creating the
/// parent directory when it does not exist. Returns
/// [`SaveMaterialProfileError`] when serialisation or the write fails.
pub fn save_material_profile_to_path(
    path: &Path,
    profile: &MaterialProfile,
) -> Result<(), SaveMaterialProfileError> {
    let contents = toml_edit::ser::to_string(profile).map_err(|err| SaveMaterialProfileError {
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
    let path = library_dir
        .join(MATERIALS_SUBDIR)
        .join(format!("{}.toml", profile.identity.id.as_str()));
    save_material_profile_to_path(&path, profile)
}
