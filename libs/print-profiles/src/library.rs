/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-platform resolution of the user profile-library directory.
//!
//! Returns the directory under which user-installed printer and
//! material profiles live: `<data_dir>/profiles/`. Settings (which
//! the GUI manages) live under `<config_dir>/`, so the two
//! responsibilities stay on distinct XDG semantics.

use std::fmt;
use std::path::PathBuf;

const PROFILES_SUBDIR: &str = "profiles";

/// Resolve the per-platform user profile-library directory:
/// `<data_dir>/profiles/`. Fails with
/// [`UserLibraryDirError::HomeDirUnavailable`] when no home directory
/// can be resolved.
pub fn user_library_dir() -> Result<PathBuf, UserLibraryDirError> {
    let dirs = sc_app_paths::project_dirs().ok_or(UserLibraryDirError::HomeDirUnavailable)?;
    Ok(dirs.data_dir().join(PROFILES_SUBDIR))
}

/// Returned when [`user_library_dir`] cannot resolve the platform
/// directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserLibraryDirError {
    HomeDirUnavailable,
}

impl fmt::Display for UserLibraryDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeDirUnavailable => write!(
                f,
                "cannot determine the user data directory because no home directory could be resolved",
            ),
        }
    }
}

impl std::error::Error for UserLibraryDirError {}
