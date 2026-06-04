/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Public dispatch entry point.
//!
//! [`write_slice`] validates the output path against the chosen
//! [`Format`]'s expectations (file vs directory, exists vs absent)
//! and routes to the matching writer. Every writer that has not
//! landed yet returns [`WriterError::NotImplemented`].

use std::fs;
use std::path::Path;

use sc_sl1cer::contour::ContourStack;

use crate::error::WriterError;
use crate::format::Format;
use crate::options::WriterOptions;

/// Write a sliced contour stack to disk using `format`'s writer.
///
/// The output path's role depends on `format`:
///
/// - For directory-target formats ([`Format::is_directory_format`]
///   is `true`), `output` is a directory. If it does not exist it
///   is created. If it exists and is a regular file the call fails
///   with [`WriterError::OutputPathExistsAndIsNotADirectory`]; if
///   it exists and is a directory that already contains entries
///   the call fails with [`WriterError::OutputPathExistsAndIsNotEmpty`].
/// - For file-target formats, `output` is a regular file path. If
///   that path exists and is a directory the call fails with
///   [`WriterError::OutputPathInvalid`].
pub fn write_slice(
    stack: &ContourStack,
    output: &Path,
    format: Format,
    options: &WriterOptions,
) -> Result<(), WriterError> {
    let _ = stack;
    let _ = options;
    validate_output_path(output, format)?;

    match format {
        Format::SvgPerLayer | Format::SvgCombined | Format::PngPerLayer => {
            Err(WriterError::NotImplemented { format })
        }
    }
}

fn validate_output_path(output: &Path, format: Format) -> Result<(), WriterError> {
    if format.is_directory_format() {
        match fs::metadata(output) {
            Ok(meta) if meta.is_dir() => {
                let mut entries = fs::read_dir(output)?;
                if entries.next().is_some() {
                    return Err(WriterError::OutputPathExistsAndIsNotEmpty {
                        path: output.to_path_buf(),
                    });
                }
                Ok(())
            }
            Ok(_) => Err(WriterError::OutputPathExistsAndIsNotADirectory {
                path: output.to_path_buf(),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(output)?;
                Ok(())
            }
            Err(err) => Err(WriterError::Io(err)),
        }
    } else {
        match fs::metadata(output) {
            Ok(meta) if meta.is_dir() => Err(WriterError::OutputPathInvalid {
                reason: format!(
                    "format \"{format}\" writes a single file, but the output path \"{}\" is an existing directory",
                    output.display(),
                ),
            }),
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(WriterError::Io(err)),
        }
    }
}

#[cfg(test)]
mod write_slice_tests {
    use std::fs::File;

    use sc_sl1cer::contour::ContourStack;
    use sc_sl1cer::profile::{
        AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
    };
    use tempfile::tempdir;

    use super::*;

    fn empty_stack() -> ContourStack {
        ContourStack::new_unchecked(Vec::new())
    }

    fn default_options() -> WriterOptions {
        let bv = BuildVolume::new(60.0, 60.0, 100.0).unwrap();
        let res = PixelResolution::new(1920, 1080).unwrap();
        let sched = LayerSchedule::uniform(0.05, 0.05, 10).unwrap();
        let profile = SliceProfile::new(
            bv,
            res,
            sched,
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        );
        WriterOptions::from_profile(&profile, "0.0.12")
    }

    #[test]
    fn svg_per_layer_returns_not_implemented() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("layers");
        let err = write_slice(
            &empty_stack(),
            &out,
            Format::SvgPerLayer,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            WriterError::NotImplemented {
                format: Format::SvgPerLayer,
            },
        ));
    }

    #[test]
    fn svg_combined_returns_not_implemented() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("combined.svg");
        let err = write_slice(
            &empty_stack(),
            &out,
            Format::SvgCombined,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            WriterError::NotImplemented {
                format: Format::SvgCombined,
            },
        ));
    }

    #[test]
    fn png_per_layer_returns_not_implemented() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("layers");
        let err = write_slice(
            &empty_stack(),
            &out,
            Format::PngPerLayer,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            WriterError::NotImplemented {
                format: Format::PngPerLayer,
            },
        ));
    }

    #[test]
    fn directory_format_creates_missing_output_directory() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("not_there_yet");
        assert!(!out.exists());
        let err = write_slice(
            &empty_stack(),
            &out,
            Format::SvgPerLayer,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(err, WriterError::NotImplemented { .. }));
        assert!(out.is_dir(), "validation should create the directory");
    }

    #[test]
    fn directory_format_accepts_existing_empty_directory() {
        let dir = tempdir().unwrap();
        let err = write_slice(
            &empty_stack(),
            dir.path(),
            Format::SvgPerLayer,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(err, WriterError::NotImplemented { .. }));
    }

    #[test]
    fn directory_format_rejects_existing_regular_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("collision");
        File::create(&file_path).unwrap();
        let err = write_slice(
            &empty_stack(),
            &file_path,
            Format::SvgPerLayer,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            WriterError::OutputPathExistsAndIsNotADirectory { .. },
        ));
    }

    #[test]
    fn directory_format_rejects_non_empty_existing_directory() {
        let dir = tempdir().unwrap();
        File::create(dir.path().join("stale.txt")).unwrap();
        let err = write_slice(
            &empty_stack(),
            dir.path(),
            Format::SvgPerLayer,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            WriterError::OutputPathExistsAndIsNotEmpty { .. },
        ));
    }

    #[test]
    fn file_format_rejects_existing_directory() {
        let dir = tempdir().unwrap();
        let err = write_slice(
            &empty_stack(),
            dir.path(),
            Format::SvgCombined,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(err, WriterError::OutputPathInvalid { .. }));
    }

    #[test]
    fn file_format_accepts_missing_output_file() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("brand_new.svg");
        assert!(!out.exists());
        let err = write_slice(
            &empty_stack(),
            &out,
            Format::SvgCombined,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(err, WriterError::NotImplemented { .. }));
    }

    #[test]
    fn file_format_accepts_existing_regular_file() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("overwriteable.svg");
        File::create(&out).unwrap();
        let err = write_slice(
            &empty_stack(),
            &out,
            Format::SvgCombined,
            &default_options(),
        )
        .unwrap_err();
        assert!(matches!(err, WriterError::NotImplemented { .. }));
    }
}
