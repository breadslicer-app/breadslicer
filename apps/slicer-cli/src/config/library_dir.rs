/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Resolution of the user profile-library directory, with an
//! environment-variable override.
//!
//! By default the library is the per-platform user data directory
//! resolved by [`sc_print_profiles::user_library_dir`]. The
//! `SC_SLICER_LIBRARY_DIR` override points it elsewhere, which lets
//! a user relocate their library and lets the integration tests
//! drive the `profiles` commands against a fixture library.

use std::path::PathBuf;

use sc_print_profiles::{UserLibraryDirError, user_library_dir};

/// Environment variable that overrides the user profile-library
/// directory when set.
pub const LIBRARY_DIR_ENV: &str = "SC_SLICER_LIBRARY_DIR";

/// Resolve the user profile-library directory: the
/// `SC_SLICER_LIBRARY_DIR` override when set, otherwise the
/// per-platform default. Fails only when the default cannot be
/// resolved and no override is set.
pub fn resolve_user_library_dir() -> Result<PathBuf, UserLibraryDirError> {
    if let Some(dir) = std::env::var_os(LIBRARY_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    user_library_dir()
}
