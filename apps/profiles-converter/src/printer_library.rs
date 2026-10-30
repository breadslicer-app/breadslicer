/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Our own printer profiles, consulted for what a machine can do.
//!
//! A source format states a resin's settings; what the machine those
//! settings are for is capable of is ours to say, and a printer profile
//! is where we say it. So a conversion reads the library we ship rather
//! than taking a third party's word for the hardware.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sc_print_primitives::PrinterId;
use sc_print_profiles::{LoadPrinterProfileError, PrinterProfile, load_printer_profile_from_path};

/// What the printer profiles under a library root say about each
/// machine.
#[derive(Clone, Debug, Default)]
pub struct PrinterLibrary {
    profiles: BTreeMap<PrinterId, PrinterProfile>,
}

/// Returned by [`PrinterLibrary::open`] when the library cannot be read.
#[derive(Debug)]
pub enum PrinterLibraryError {
    /// The `printers` directory could not be listed.
    ReadDir { path: PathBuf, source: io::Error },
    /// A profile in it could not be loaded.
    Load {
        path: PathBuf,
        source: Box<LoadPrinterProfileError>,
    },
}

impl fmt::Display for PrinterLibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadDir { path, source } => write!(
                f,
                "failed to list the printer library at {}: {source}",
                path.display(),
            ),
            Self::Load { path, source } => {
                write!(f, "failed to load {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for PrinterLibraryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadDir { source, .. } => Some(source),
            Self::Load { source, .. } => Some(source),
        }
    }
}

impl PrinterLibrary {
    /// Read every printer profile under `<root>/printers`.
    ///
    /// A profile that does not load is a failure rather than a machine
    /// left out: this is our own library, and a conversion that quietly
    /// treated it as absent would write resins as if the machine could
    /// do less than it can.
    pub fn open(root: &Path) -> Result<Self, PrinterLibraryError> {
        let dir = root.join("printers");
        let entries = fs::read_dir(&dir).map_err(|source| PrinterLibraryError::ReadDir {
            path: dir.clone(),
            source,
        })?;
        let mut profiles = BTreeMap::new();
        for entry in entries {
            let entry = entry.map_err(|source| PrinterLibraryError::ReadDir {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
            {
                continue;
            }
            let profile = load_printer_profile_from_path(&path).map_err(|source| {
                PrinterLibraryError::Load {
                    path: path.clone(),
                    source: Box::new(source),
                }
            })?;
            profiles.insert(profile.identity.id.clone(), profile);
        }
        Ok(Self { profiles })
    }

    /// Find whether the machine `id` names splits its travel into 2
    /// stages, or `None` when the library holds no profile for it.
    pub fn splits_travel(&self, id: &PrinterId) -> Option<bool> {
        self.profiles
            .get(id)
            .map(|profile| profile.capabilities.two_stage_lift)
    }

    /// Find the profile the library holds for the machine `id` names.
    pub fn profile(&self, id: &PrinterId) -> Option<&PrinterProfile> {
        self.profiles.get(id)
    }

    /// Every profile the library holds, by id.
    pub fn profiles(&self) -> impl Iterator<Item = &PrinterProfile> {
        self.profiles.values()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    const PROFILE: &str = "\
[identity]
id = \"testcorp-e2e\"
manufacturer = \"TestCorp\"
family = \"Series\"
name = \"E2E\"
profile_version = \"1.0.0\"

[hardware.build_volume]
x_mm = 100.0
y_mm = 100.0
z_mm = 40.0

[hardware.pixel_resolution]
x = 1000
y = 1000

[hardware.pixel_size_microns]
x = 100.0
y = 100.0

[hardware.mirror]
x = true
y = false

[capabilities]
supports_grayscale = true
two_stage_lift = true
has_camera = false

[[capabilities.output_formats]]
format = \"goo\"

[constraints]

[time_calibration]
";

    fn library_with(profile: &str, file: &str) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers");
        fs::create_dir_all(&printers).unwrap();
        fs::write(printers.join(file), profile).unwrap();
        dir
    }

    fn printer(id: &str) -> PrinterId {
        PrinterId::new(id).unwrap()
    }

    #[test]
    fn a_machine_answers_with_the_capability_its_profile_declares() {
        let dir = library_with(PROFILE, "testcorp-e2e.toml");
        let library = PrinterLibrary::open(dir.path()).unwrap();

        assert_eq!(library.splits_travel(&printer("testcorp-e2e")), Some(true));
    }

    #[test]
    fn a_machine_with_no_profile_answers_nothing() {
        let dir = library_with(PROFILE, "testcorp-e2e.toml");
        let library = PrinterLibrary::open(dir.path()).unwrap();

        assert_eq!(library.splits_travel(&printer("testcorp-other")), None);
    }

    #[test]
    fn the_id_a_profile_declares_is_the_key_not_its_filename() {
        let dir = library_with(PROFILE, "renamed.toml");
        let library = PrinterLibrary::open(dir.path()).unwrap();

        assert_eq!(library.splits_travel(&printer("testcorp-e2e")), Some(true));
    }

    #[test]
    fn a_profile_that_does_not_load_fails_the_whole_library() {
        let dir = library_with("not a profile", "broken.toml");
        let err = PrinterLibrary::open(dir.path()).unwrap_err();

        assert!(matches!(err, PrinterLibraryError::Load { .. }), "{err}");
    }

    #[test]
    fn a_root_without_a_printers_directory_is_reported() {
        let dir = tempdir().unwrap();
        let err = PrinterLibrary::open(dir.path()).unwrap_err();

        assert!(matches!(err, PrinterLibraryError::ReadDir { .. }), "{err}");
        assert!(err.to_string().contains("printers"), "{err}");
    }
}
