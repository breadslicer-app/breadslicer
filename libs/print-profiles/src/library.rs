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

use directories::ProjectDirs;

// These three constants must stay in sync with the GUI's
// app_settings.rs so the profile library and the on-disk settings
// resolve to the same per-platform app identity. Drift means user
// profiles and user settings land under different directories.
const QUALIFIER: &str = "it";
const ORGANIZATION: &str = "soulcrafted";

// On Linux the XDG path is just `<application>`, so a bare `slicer`
// would collide with unrelated tools. macOS and Windows already
// namespace via the bundle id or the organization directory, so they
// keep the short name.
#[cfg(target_os = "linux")]
const APPLICATION: &str = "soulcrafted-slicer";
#[cfg(not(target_os = "linux"))]
const APPLICATION: &str = "slicer";

const PROFILES_SUBDIR: &str = "profiles";

/// Resolve the per-platform user profile-library directory:
/// `<data_dir>/profiles/`. Fails with
/// [`UserLibraryDirError::HomeDirUnavailable`] when
/// [`ProjectDirs::from`] cannot determine a home directory.
pub fn user_library_dir() -> Result<PathBuf, UserLibraryDirError> {
    let dirs = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .ok_or(UserLibraryDirError::HomeDirUnavailable)?;
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
