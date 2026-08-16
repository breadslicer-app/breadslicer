/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The format-agnostic core writer.
//!
//! [`write_profiles`] installs a set of [`ConvertedProfile`] values
//! into a destination library root through the `sc_print_profiles`
//! save operations.
//! When `copy_images` is on, a printer's source image is installed
//! beside the profile and the profile's `image_asset_path` is set to
//! the returned reference.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;

use sc_print_profiles::{
    SaveMaterialProfileError, SavePrinterImageError, SavePrinterProfileError,
    save_material_profile_to_library, save_printer_image_to_library,
    save_printer_profile_to_library,
};

use crate::profile::ConvertedProfile;

/// How many profiles of each kind a [`write_profiles`] call wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteReport {
    pub printers: usize,
    pub materials: usize,
    pub images: usize,
}

/// Returned by [`write_profiles`] when one of the library save
/// operations fails; each variant wraps that operation's error.
#[derive(Debug)]
pub enum WriteError {
    Printer(SavePrinterProfileError),
    Material(SaveMaterialProfileError),
    Image(SavePrinterImageError),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Printer(err) => err.fmt(f),
            Self::Material(err) => err.fmt(f),
            Self::Image(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Printer(err) => Some(err),
            Self::Material(err) => Some(err),
            Self::Image(err) => Some(err),
        }
    }
}

/// Write every profile into the library rooted at `out_dir`, creating
/// its subdirectories as needed. Returns the per-kind counts.
pub fn write_profiles(
    profiles: &[ConvertedProfile],
    out_dir: &Path,
    copy_images: bool,
) -> Result<WriteReport, WriteError> {
    let mut report = WriteReport::default();
    for profile in profiles {
        match profile {
            ConvertedProfile::Printer(printer) => {
                let domain = match (copy_images, &printer.image_source) {
                    (true, Some(source)) => {
                        let reference = save_printer_image_to_library(
                            out_dir,
                            &printer.profile.identity.id,
                            source,
                        )
                        .map_err(WriteError::Image)?;
                        report.images += 1;
                        let mut with_image = printer.profile.clone();
                        with_image.identity.image_asset_path = Some(reference.as_str().to_owned());
                        Cow::Owned(with_image)
                    }
                    _ => Cow::Borrowed(&printer.profile),
                };
                save_printer_profile_to_library(out_dir, domain.as_ref())
                    .map_err(WriteError::Printer)?;
                report.printers += 1;
            }
            ConvertedProfile::Material(material) => {
                save_material_profile_to_library(out_dir, material)
                    .map_err(WriteError::Material)?;
                report.materials += 1;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::fs;
    use std::path::PathBuf;

    use sc_print_profiles::{
        MaterialProfile, PrinterProfile, load_material_profile_from_path,
        load_printer_profile_from_path,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::profile::ConvertedPrinter;
    use crate::reader::ProfileReader;

    const PRINTER_TOML: &str = r#"
[identity]
id = "test-printer"
manufacturer = "TestCorp"
family = "Test"
name = "Test Printer"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 5.0, y_mm = 5.0, z_mm = 50.0 }
pixel_resolution = { x = 500, y = 500 }
pixel_size_microns = { x = 10.0, y = 10.0 }
mirror = { x = false, y = false }

[capabilities]
output_formats = [{ format = "png_per_layer" }]
supports_grayscale = true
two_stage_lift = false
has_camera = false

[constraints]
"#;

    const MATERIAL_TOML: &str = r#"
[identity]
id = "test-resin"
brand = "TestCorp"
family = "Test"
name = "Test Resin"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]

[per_printer."test-printer"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 5
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

    fn sample_printer() -> PrinterProfile {
        toml_edit::de::from_str(PRINTER_TOML).unwrap()
    }

    fn sample_material() -> MaterialProfile {
        toml_edit::de::from_str(MATERIAL_TOML).unwrap()
    }

    fn printer(profile: PrinterProfile, image_source: Option<PathBuf>) -> ConvertedProfile {
        ConvertedProfile::Printer(ConvertedPrinter {
            profile,
            image_source,
        })
    }

    #[test]
    fn writes_a_printer_that_reloads_to_the_same_profile() {
        let dir = tempdir().unwrap();
        let profile = sample_printer();

        let report = write_profiles(&[printer(profile.clone(), None)], dir.path(), false).unwrap();
        assert_eq!(
            report,
            WriteReport {
                printers: 1,
                materials: 0,
                images: 0,
            }
        );

        let path = dir.path().join("printers").join("test-printer.toml");
        let reloaded: PrinterProfile = load_printer_profile_from_path(&path).unwrap();
        assert_eq!(reloaded, profile);
    }

    #[test]
    fn a_printer_is_written_in_the_library_dialect() {
        let dir = tempdir().unwrap();
        write_profiles(&[printer(sample_printer(), None)], dir.path(), false).unwrap();

        let toml =
            fs::read_to_string(dir.path().join("printers").join("test-printer.toml")).unwrap();
        assert!(toml.contains("[identity]"), "{toml}");
        assert!(toml.contains("[hardware.build_volume]"), "{toml}");
        assert!(toml.contains("[capabilities]"), "{toml}");
        assert!(
            toml.contains("[[capabilities.output_formats]]"),
            "output formats are an array of tables: {toml}",
        );
        assert!(
            !toml.contains("[hardware]\n"),
            "no empty parent header before its sub-tables: {toml}",
        );
        assert!(
            !toml.contains("identity = {"),
            "top-level structs must not be inline: {toml}",
        );
    }

    #[test]
    fn a_material_is_written_with_sectioned_headers() {
        let dir = tempdir().unwrap();
        write_profiles(
            &[ConvertedProfile::Material(sample_material())],
            dir.path(),
            false,
        )
        .unwrap();

        let toml =
            fs::read_to_string(dir.path().join("materials").join("test-resin.toml")).unwrap();
        assert!(toml.contains("[identity]"), "{toml}");
        assert!(toml.contains("[intent]"), "{toml}");
        assert!(toml.contains("[per_printer.test-printer]"), "{toml}");
        assert!(!toml.contains("intent = {"), "{toml}");
    }

    #[test]
    fn writes_a_material_that_reloads_to_the_same_profile() {
        let dir = tempdir().unwrap();
        let material = sample_material();

        let report = write_profiles(
            &[ConvertedProfile::Material(material.clone())],
            dir.path(),
            false,
        )
        .unwrap();
        assert_eq!(report.materials, 1);

        let path = dir.path().join("materials").join("test-resin.toml");
        let reloaded: MaterialProfile = load_material_profile_from_path(&path).unwrap();
        assert_eq!(reloaded, material);
    }

    #[test]
    fn counts_both_kinds_and_creates_both_subdirs() {
        let dir = tempdir().unwrap();
        let profiles = vec![
            printer(sample_printer(), None),
            ConvertedProfile::Material(sample_material()),
        ];

        let report = write_profiles(&profiles, dir.path(), false).unwrap();
        assert_eq!(
            report,
            WriteReport {
                printers: 1,
                materials: 1,
                images: 0,
            }
        );
        assert!(
            dir.path()
                .join("printers")
                .join("test-printer.toml")
                .is_file()
        );
        assert!(
            dir.path()
                .join("materials")
                .join("test-resin.toml")
                .is_file()
        );
    }

    #[test]
    fn writing_an_empty_set_creates_nothing() {
        let dir = tempdir().unwrap();
        let report = write_profiles(&[], dir.path(), false).unwrap();
        assert_eq!(report, WriteReport::default());
        assert!(!dir.path().join("printers").exists());
        assert!(!dir.path().join("materials").exists());
    }

    #[test]
    fn copies_the_image_and_wires_the_asset_path() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.png");
        fs::write(&source, b"fake-png-bytes").unwrap();

        let report =
            write_profiles(&[printer(sample_printer(), Some(source))], dir.path(), true).unwrap();
        assert_eq!(report.images, 1);

        let image = dir.path().join("images").join("test-printer.png");
        assert_eq!(fs::read(&image).unwrap(), b"fake-png-bytes");

        let path = dir.path().join("printers").join("test-printer.toml");
        let reloaded: PrinterProfile = load_printer_profile_from_path(&path).unwrap();
        assert_eq!(
            reloaded.identity.image_asset_path.as_deref(),
            Some("images/test-printer.png"),
        );
    }

    #[test]
    fn image_is_not_copied_when_copy_images_is_off() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.png");
        fs::write(&source, b"fake-png-bytes").unwrap();

        let report = write_profiles(
            &[printer(sample_printer(), Some(source))],
            dir.path(),
            false,
        )
        .unwrap();
        assert_eq!(report.images, 0);
        assert!(!dir.path().join("images").exists());

        let path = dir.path().join("printers").join("test-printer.toml");
        let reloaded: PrinterProfile = load_printer_profile_from_path(&path).unwrap();
        assert_eq!(reloaded.identity.image_asset_path, None);
    }

    #[test]
    fn a_missing_source_image_is_reported() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.png");

        let err = write_profiles(
            &[printer(sample_printer(), Some(missing))],
            dir.path(),
            true,
        )
        .unwrap_err();
        assert!(matches!(err, WriteError::Image(_)));
    }

    /// A trivial reader that hands back a fixed set, used to check that
    /// the port feeds the writer end to end.
    struct FixedReader(Vec<ConvertedProfile>);

    impl ProfileReader for FixedReader {
        type Error = Infallible;

        fn read(&self, _inputs: &[&Path]) -> Result<Vec<ConvertedProfile>, Self::Error> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn a_reader_feeds_the_writer_through_the_port() {
        let dir = tempdir().unwrap();
        let reader = FixedReader(vec![
            printer(sample_printer(), None),
            ConvertedProfile::Material(sample_material()),
        ]);

        let profiles = reader.read(&[]).unwrap();
        let report = write_profiles(&profiles, dir.path(), false).unwrap();
        assert_eq!(
            report,
            WriteReport {
                printers: 1,
                materials: 1,
                images: 0,
            }
        );
    }
}
