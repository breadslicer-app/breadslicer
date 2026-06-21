/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Application identity and on-disk path resolution.
//!
//! Single owner of the SoulCrafted Slicer's identity (reverse-DNS
//! `it.soulcrafted.Slicer`; the `directories` qualifier / organization
//! / application triple) and of the directories derived from it: the
//! per-user platform directories and the installed read-only data root
//! that packaged builds ship.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

const QUALIFIER: &str = "it";
const ORGANIZATION: &str = "soulcrafted";

// On Linux the data/config path is just `<application>`, so a bare
// `slicer` would collide with unrelated tools. macOS and Windows
// already namespace via the bundle id or the organization directory,
// so they keep the short name.
#[cfg(target_os = "linux")]
const APPLICATION: &str = "soulcrafted-slicer";
#[cfg(not(target_os = "linux"))]
const APPLICATION: &str = "slicer";

/// The per-user platform directories for this application, or `None`
/// when no home directory can be resolved.
pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

/// Resolve the installed, read-only data root that packaged builds
/// ship: the directory whose subdirectories hold bundled vendor
/// profiles and other shipped assets.
///
/// On Linux this searches the XDG data directories; on macOS and
/// Windows it resolves relative to the executable. When neither finds
/// an installed directory the result is the source-tree `assets/`
/// root, which exists in a development checkout and is absent (so the
/// caller's lookup simply skips it) in an installed build.
pub fn bundled_data_dir() -> PathBuf {
    installed_data_dir().unwrap_or_else(dev_assets_dir)
}

/// Linux: the first existing `<base>/<application>` across the XDG data
/// directories (`XDG_DATA_HOME` then each `XDG_DATA_DIRS` entry).
#[cfg(target_os = "linux")]
fn installed_data_dir() -> Option<PathBuf> {
    find_app_dir(&data_search_bases(), APPLICATION)
}

/// macOS / Windows: the first existing data directory relative to the
/// running executable (beside the binary, or under a sibling `share/`).
#[cfg(not(target_os = "linux"))]
fn installed_data_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let bases = [dir.to_path_buf(), dir.join("share")];
    find_app_dir(&bases, APPLICATION)
}

/// The XDG data directories, in lookup order: `XDG_DATA_HOME` (or its
/// default) followed by each `XDG_DATA_DIRS` entry (or their defaults).
#[cfg(target_os = "linux")]
fn data_search_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(base) = directories::BaseDirs::new() {
        bases.push(base.data_dir().to_path_buf());
    }
    let dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::ffi::OsString::from("/usr/local/share:/usr/share"));
    bases.extend(std::env::split_paths(&dirs));
    bases
}

/// Return the first `<base>/<app>` that is an existing directory.
fn find_app_dir(bases: &[PathBuf], app: &str) -> Option<PathBuf> {
    bases.iter().map(|base| base.join(app)).find(|p| p.is_dir())
}

/// The source-tree data root. `CARGO_MANIFEST_DIR` is
/// `<workspace>/libs/app-paths`; the `assets/` root sits two levels up.
fn dev_assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

#[cfg(test)]
mod tests_paths;
