/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Application identity and on-disk path resolution.
//!
//! Single owner of the SoulCrafted Slicer's identity (reverse-DNS
//! `it.soulcrafted.slicer`; the `directories` qualifier / organization
//! / application triple) and of the directories derived from it: the per-user
//! platform directories and the installed read-only data root that packaged
//! builds ship.

use std::path::{Path, PathBuf};

use directories::{ProjectDirs, UserDirs};

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

/// Shared by the desktop entry basename, the AppStream component id and the
/// Wayland `app_id`. A window advertising anything else cannot be matched back
/// to the launcher entry by the desktop shell.
pub const APP_ID: &str = "it.soulcrafted.slicer";

/// The per-user platform directories for this application, or `None` when no
/// home directory can be resolved.
pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

/// The current user's home directory, or `None` when it cannot be resolved.
/// Used as the fallback starting location for file dialogs.
pub fn home_dir() -> Option<PathBuf> {
    UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// Every read-only data root a build can ship, highest precedence first:
/// the installed roots (each existing `<base>/<application>` across the system
/// data directories on Linux, the bundle's `Contents/Resources` or an
/// executable-relative directory on macOS, executable-relative on Windows)
/// followed by the source-tree `assets/` root when it exists.
///
/// The user's writable library (`XDG_DATA_HOME`, resolved via [`project_dirs`])
/// is deliberately absent here: it is a distinct, user-owned layer the profile
/// loader merges on top of these roots. Keeping it out is what stops a bundled
/// lookup from resolving to the user's own library.
pub fn bundled_data_dirs() -> Vec<PathBuf> {
    bundled_data_dirs_from(installed_data_dirs(), dev_assets_dir())
}

/// [`bundled_data_dirs`] for the given installed roots and source-tree root.
fn bundled_data_dirs_from(mut roots: Vec<PathBuf>, dev: PathBuf) -> Vec<PathBuf> {
    if dev.is_dir() {
        roots.push(dev);
    }
    roots
}

/// Linux: every existing `<base>/<application>` across the system XDG
/// data directories (`XDG_DATA_DIRS`).
#[cfg(target_os = "linux")]
fn installed_data_dirs() -> Vec<PathBuf> {
    let bases = data_search_bases(std::env::var_os("XDG_DATA_DIRS"));
    all_app_dirs(&bases, APPLICATION)
}

/// macOS / Windows: every existing data directory relative to the
/// running executable.
#[cfg(not(target_os = "linux"))]
fn installed_data_dirs() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    all_app_dirs(&exe_relative_bases(dir), APPLICATION)
}

/// Lists where an installed build can keep its data, highest precedence first.
/// Returns candidates: whether each one exists is the caller's to check.
#[cfg(target_os = "macos")]
fn exe_relative_bases(exe_dir: &Path) -> Vec<PathBuf> {
    let mut bases = Vec::with_capacity(3);
    // A packaged build keeps its data in the bundle's `Contents/Resources` and
    // never beside the binary: the signature seals `Contents/`, and Apple keeps
    // non-executable payload out of `Contents/MacOS/`. Reached through the
    // parent rather than by appending `..`, so that the bases stay canonical.
    if let Some(contents) = exe_dir.parent() {
        bases.push(contents.join("Resources"));
    }
    // Unpackaged layouts: a build directory or an archive holding the binaries
    // beside their data, with no bundle around either.
    bases.push(exe_dir.to_path_buf());
    bases.push(exe_dir.join("share"));
    bases
}

/// Lists where an installed build can keep its data, highest precedence first.
/// Returns candidates: whether each one exists is the caller's to check.
#[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
fn exe_relative_bases(exe_dir: &Path) -> Vec<PathBuf> {
    vec![exe_dir.to_path_buf(), exe_dir.join("share")]
}

/// The system XDG data directories: each entry of `xdg_data_dirs`, the value of
/// `XDG_DATA_DIRS` (or the spec defaults `/usr/local/share:/usr/share` when it
/// is unset or empty). `XDG_DATA_HOME` is not among them: it is the writable
/// per-user location (the user profile library), never an install root.
#[cfg(target_os = "linux")]
fn data_search_bases(xdg_data_dirs: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let dirs = xdg_data_dirs
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
