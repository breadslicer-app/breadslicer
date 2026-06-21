/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Resolution of the bundled vendor-profiles directory.
//!
//! The combined profile loader cascades from the user library to a
//! directory of bundled, read-only vendor profiles. This module
//! resolves that directory: the `SC_SLICER_ASSETS_DIR` override when
//! set, otherwise the `profiles` subdirectory of the installed data
//! root resolved by [`sc_app_paths::bundled_data_dir`].

use std::path::PathBuf;

/// Environment variable that overrides the bundled-profiles directory
/// when set. Tests and packaged installs point it at the right
/// location.
pub const ASSETS_DIR_ENV: &str = "SC_SLICER_ASSETS_DIR";

/// Resolve the directory of bundled vendor profiles: the
/// `SC_SLICER_ASSETS_DIR` override when set, otherwise the `profiles`
/// subdirectory of the installed data root. When no data root is
/// found the path does not exist, and the loader cascade falls
/// through to the user library alone.
pub fn resolve_assets_profiles_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ASSETS_DIR_ENV) {
        return PathBuf::from(dir);
    }
    sc_app_paths::bundled_data_dir().join("profiles")
}
