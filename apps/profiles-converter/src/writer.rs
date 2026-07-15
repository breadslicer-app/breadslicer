/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The format-agnostic core writer.
//!
//! [`write_profiles`] lays a set of [`ConvertedProfile`] values out
//! under a destination library root: printers at `printers/<id>.toml`
//! and materials at `materials/<id>.toml`, serialised with `toml_edit`.
//! When `copy_images` is on, a printer's source image is copied to
//! `images/<id>.<ext>` and the profile's `image_asset_path` is set to
//! that path, relative to the library root. The writer knows nothing
//! about where the profiles came from.

use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::profile::ConvertedProfile;

/// Subdirectory of the library root that holds printer profiles.
const PRINTERS_SUBDIR: &str = "printers";
/// Subdirectory of the library root that holds material profiles.
const MATERIALS_SUBDIR: &str = "materials";
/// Subdirectory of the library root that holds preview images.
const IMAGES_SUBDIR: &str = "images";

/// How many profiles of each kind a [`write_profiles`] call wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteReport {
    pub printers: usize,
    pub materials: usize,
    pub images: usize,
}

/// Returned by [`write_profiles`] when a profile cannot be serialised,
/// an image cannot be copied, or a file cannot be written.
#[derive(Debug)]
pub enum WriteError {
    /// A profile could not be serialised to TOML.
    Serialize {
        id: String,
        source: toml_edit::ser::Error,
    },
    /// A destination subdirectory could not be created.
    CreateDir { path: PathBuf, source: io::Error },
    /// A profile file could not be written.
    WriteFile { path: PathBuf, source: io::Error },
    /// A source image could not be copied into the library.
    CopyImage {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize { id, source } => {
                write!(f, "failed to serialise profile {id:?}: {source}")
            }
            Self::CreateDir { path, source } => write!(
                f,
                "failed to create the destination directory {}: {source}",
                path.display(),
            ),
            Self::WriteFile { path, source } => {
                write!(f, "failed to write profile {}: {source}", path.display())
            }
            Self::CopyImage { from, to, source } => write!(
                f,
                "failed to copy image {} to {}: {source}",
                from.display(),
                to.display(),
            ),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize { source, .. } => Some(source),
            Self::CreateDir { source, .. } => Some(source),
            Self::WriteFile { source, .. } => Some(source),
            Self::CopyImage { source, .. } => Some(source),
        }
    }
}

/// Write every profile into `out_dir`, creating the `printers/`,
/// `materials/`, and (when images are copied) `images/` subdirectories
/// as needed. Returns the per-kind counts.
pub fn write_profiles(
    profiles: &[ConvertedProfile],
    out_dir: &Path,
    copy_images: bool,
) -> Result<WriteReport, WriteError> {
    let mut report = WriteReport::default();
    for profile in profiles {
        match profile {
            ConvertedProfile::Printer(printer) => {
                let id = printer.profile.identity.id.as_str();
                let domain = match (copy_images, &printer.image_source) {
                    (true, Some(source)) => {
                        let file_name = image_file_name(id, source);
                        copy_image(out_dir, source, &file_name)?;
                        report.images += 1;
                        let mut with_image = printer.profile.clone();
                        with_image.identity.image_asset_path =
                            Some(format!("{IMAGES_SUBDIR}/{file_name}"));
                        Cow::Owned(with_image)
                    }
                    _ => Cow::Borrowed(&printer.profile),
                };
                let toml = toml_edit::ser::to_string(domain.as_ref()).map_err(|source| {
                    WriteError::Serialize {
                        id: id.to_owned(),
                        source,
                    }
                })?;
                write_one(out_dir, PRINTERS_SUBDIR, id, &toml)?;
                report.printers += 1;
            }
            ConvertedProfile::Material(material) => {
                let id = material.identity.id.as_str();
                let toml = toml_edit::ser::to_string(material).map_err(|source| {
                    WriteError::Serialize {
                        id: id.to_owned(),
                        source,
                    }
                })?;
                write_one(out_dir, MATERIALS_SUBDIR, id, &toml)?;
                report.materials += 1;
            }
        }
    }
    Ok(report)
}

/// The destination file name for a printer's image: `<id>.<ext>`, where
/// the extension is taken from the source (defaulting to `png`).
fn image_file_name(id: &str, source: &Path) -> String {
    let ext = source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "png".to_owned());
    format!("{id}.{ext}")
}

/// Copy `source` into `<out_dir>/images/<file_name>`.
fn copy_image(out_dir: &Path, source: &Path, file_name: &str) -> Result<(), WriteError> {
    let dir = out_dir.join(IMAGES_SUBDIR);
    fs::create_dir_all(&dir).map_err(|err| WriteError::CreateDir {
        path: dir.clone(),
        source: err,
    })?;
    let dest = dir.join(file_name);
    fs::copy(source, &dest).map_err(|err| WriteError::CopyImage {
        from: source.to_owned(),
        to: dest,
        source: err,
    })?;
    Ok(())
}

/// Write one profile's TOML to `<out_dir>/<subdir>/<id>.toml`.
fn write_one(out_dir: &Path, subdir: &str, id: &str, toml: &str) -> Result<(), WriteError> {
    let dir = out_dir.join(subdir);
    fs::create_dir_all(&dir).map_err(|source| WriteError::CreateDir {
        path: dir.clone(),
        source,
    })?;
    let path = dir.join(format!("{id}.toml"));
    fs::write(&path, toml).map_err(|source| WriteError::WriteFile { path, source })
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use sc_print_profiles::{MaterialProfile, PrinterProfile};
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
        let reloaded: PrinterProfile =
            toml_edit::de::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reloaded, profile);
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
        let reloaded: MaterialProfile =
            toml_edit::de::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
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
        let reloaded: PrinterProfile =
            toml_edit::de::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
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
        let reloaded: PrinterProfile =
            toml_edit::de::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
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
        assert!(matches!(err, WriteError::CopyImage { .. }));
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
