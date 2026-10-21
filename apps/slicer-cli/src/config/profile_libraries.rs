/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Resolution of the profile libraries the CLI consults.
//!
//! With no overrides set, the profiles are resolved in the following
//! order: the per-user library first, then every bundled root.
//! 2 environment variables override that:
//! - `SC_SLICER_LIBRARY_DIR` replaces the user library.
//! - `SC_SLICER_ASSETS_DIR` replaces the whole list of bundled assets.

use std::ffi::OsString;
use std::path::PathBuf;

use sc_print_profiles::{ProfileLibraries, ProfileSource, user_library_dir};

/// Environment variable that overrides the user profile-library
/// directory when set.
pub(crate) const LIBRARY_DIR_ENV: &str = "SC_SLICER_LIBRARY_DIR";

/// Environment variable that overrides the bundled-profiles
/// directory when set.
pub(crate) const ASSETS_DIR_ENV: &str = "SC_SLICER_ASSETS_DIR";

/// Resolve the profile libraries from the environment overrides,
/// falling back to the platform's libraries.
pub(crate) fn resolve_profile_libraries() -> ProfileLibraries {
    libraries_from_overrides(
        std::env::var_os(LIBRARY_DIR_ENV),
        std::env::var_os(ASSETS_DIR_ENV),
    )
}

/// Build the libraries from explicit override values.
fn libraries_from_overrides(
    library: Option<OsString>,
    assets: Option<OsString>,
) -> ProfileLibraries {
    let user = library
        .map(PathBuf::from)
        .or_else(|| user_library_dir().ok());
    let bundled = match assets {
        Some(dir) => vec![PathBuf::from(dir)],
        None => bundled_roots(&ProfileLibraries::platform_default()),
    };
    ProfileLibraries::new(user, bundled)
}

/// The bundled half of a libraries value, in order, owned.
fn bundled_roots(libraries: &ProfileLibraries) -> Vec<PathBuf> {
    libraries
        .roots()
        .filter(|(_, source)| matches!(source, ProfileSource::Bundled))
        .map(|(path, _)| path.to_owned())
        .collect()
}

#[cfg(test)]
mod profile_libraries_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn both_overrides_replace_both_sides() {
        let libraries = libraries_from_overrides(
            Some(OsString::from("/custom/library")),
            Some(OsString::from("/custom/assets")),
        );
        assert_eq!(
            libraries.user_library_dir(),
            Some(Path::new("/custom/library")),
        );
        assert_eq!(
            bundled_roots(&libraries),
            vec![PathBuf::from("/custom/assets")],
        );
    }

    #[test]
    fn the_assets_override_alone_keeps_the_platform_user_library() {
        let libraries = libraries_from_overrides(None, Some(OsString::from("/custom/assets")));
        assert_eq!(
            libraries.user_library_dir(),
            user_library_dir().ok().as_deref(),
        );
        assert_eq!(
            bundled_roots(&libraries),
            vec![PathBuf::from("/custom/assets")],
        );
    }

    #[test]
    fn the_library_override_alone_keeps_the_platform_bundled_roots() {
        let libraries = libraries_from_overrides(Some(OsString::from("/custom/library")), None);
        assert_eq!(
            libraries.user_library_dir(),
            Some(Path::new("/custom/library")),
        );
        assert_eq!(
            bundled_roots(&libraries),
            bundled_roots(&ProfileLibraries::platform_default()),
        );
    }

    #[test]
    fn no_overrides_yield_the_platform_libraries() {
        let libraries = libraries_from_overrides(None, None);
        let platform = ProfileLibraries::platform_default();
        assert_eq!(libraries.user_library_dir(), platform.user_library_dir(),);
        assert_eq!(bundled_roots(&libraries), bundled_roots(&platform));
    }
}
