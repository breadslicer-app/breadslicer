/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! PROVISIONAL resolution of the bundled vendor-profiles directory.
//!
//! The combined profile loader cascades from the user library to a
//! directory of bundled, read-only vendor profiles. That directory
//! has no single correct location: it depends on how the
//! application was installed, which differs per platform and per
//! packaging mechanism (Flatpak, Snap, Nix, a macOS app bundle, a
//! Windows installer, a bare `cargo install`).
//!
//! The resolution implemented here is PROVISIONAL. It honours an
//! explicit environment-variable override and otherwise falls back
//! to a development-tree path baked in at compile time. It does NOT
//! implement the installation-aware resolution that a shipped build
//! requires: searching `XDG_DATA_DIRS` on Linux (for Flatpak / Snap
//! / distro packages) and resolving relative to the executable (for
//! macOS bundles, Windows, and Nix). That resolution MUST be built
//! before any packaged release ships; until then, packaged installs
//! must set the override variable explicitly.

use std::path::PathBuf;

/// Environment variable that overrides the bundled-assets directory
/// when set. The primary mechanism while the resolution below is
/// provisional: tests and packaged installs point it at the right
/// location.
pub const ASSETS_DIR_ENV: &str = "SC_SLICER_ASSETS_DIR";

/// PROVISIONAL: resolve the directory of bundled vendor profiles.
///
/// Returns the `SC_SLICER_ASSETS_DIR` override when set; otherwise
/// the compile-time development-tree location. A shipped build whose
/// override is unset will get a path that does not exist, and the
/// loader cascade will fall through to the user library alone. See
/// the module docs: the installation-aware resolution is pending.
pub fn resolve_assets_profiles_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ASSETS_DIR_ENV) {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/profiles")
}
