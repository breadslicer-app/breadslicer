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

/// The single installed, read-only data root a packaged build ships:
/// the highest-precedence entry of [`bundled_data_dirs`], or the
/// source-tree `assets/` root as a development fallback.
///
/// Prefer [`bundled_data_dirs`] where the full set of read-only roots
/// matters (e.g. merging a profile library); this single-root helper
/// exists for callers that only need one location.
pub fn bundled_data_dir() -> PathBuf {
    bundled_data_dirs()
        .into_iter()
        .next()
        .unwrap_or_else(dev_assets_dir)
}

/// Every read-only data root a build can ship, highest precedence
/// first: the installed roots (each existing `<base>/<application>`
/// across the system data directories on Linux, or executable-relative
/// on macOS / Windows) followed by the source-tree `assets/` root when
/// it exists.
///
/// The user's writable library (`XDG_DATA_HOME`, resolved via
/// [`project_dirs`]) is deliberately absent here: it is a distinct,
/// user-owned layer the profile loader merges on top of these roots.
/// Keeping it out is what stops a bundled lookup from resolving to the
/// user's own library.
pub fn bundled_data_dirs() -> Vec<PathBuf> {
    let mut roots = installed_data_dirs();
    let dev = dev_assets_dir();
    if dev.is_dir() {
        roots.push(dev);
    }
    roots
}

/// Linux: every existing `<base>/<application>` across the system XDG
/// data directories (`XDG_DATA_DIRS`).
#[cfg(target_os = "linux")]
fn installed_data_dirs() -> Vec<PathBuf> {
    all_app_dirs(&data_search_bases(), APPLICATION)
}

/// macOS / Windows: every existing data directory relative to the
/// running executable (beside the binary, or under a sibling `share/`).
#[cfg(not(target_os = "linux"))]
fn installed_data_dirs() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    all_app_dirs(&[dir.to_path_buf(), dir.join("share")], APPLICATION)
}

/// The system XDG data directories: each `XDG_DATA_DIRS` entry (or the
/// spec defaults `/usr/local/share:/usr/share`). `XDG_DATA_HOME` is not
/// among them: it is the writable per-user location (the user profile
/// library), never an install root.
#[cfg(target_os = "linux")]
fn data_search_bases() -> Vec<PathBuf> {
    let dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::ffi::OsString::from("/usr/local/share:/usr/share"));
    std::env::split_paths(&dirs).collect()
}

/// Every `<base>/<app>` that is an existing directory, in base order.
fn all_app_dirs(bases: &[PathBuf], app: &str) -> Vec<PathBuf> {
    bases
        .iter()
        .map(|base| base.join(app))
        .filter(|p| p.is_dir())
        .collect()
}

/// The source-tree data root. `CARGO_MANIFEST_DIR` is
/// `<workspace>/libs/app-paths`; the `assets/` root sits two levels up.
fn dev_assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

#[cfg(test)]
mod tests_paths;
