/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Public dispatch entry points: which writer a chosen format
//! reaches.

use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use sc_sl1cer::contour::ContourStack;

use crate::error::WriterError;
use crate::format::Format;
use crate::goo::v3_0;
use crate::options::WriterOptions;
use crate::print_program::PrintProgram;
use crate::printer_file_format::{CtbVariant, GooVersion, PrinterFileFormat};

/// Write a sliced contour stack to disk using `format`'s writer.
///
/// [`Format::ThreeMf`] is not written here: it fails with
/// [`WriterError::SceneContextRequired`], and
/// [`write_scene_slices`](crate::write_scene_slices) writes it
/// instead.
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
    validate_output_path(output, format)?;

    match format {
        Format::SvgPerLayer => crate::debug::svg::write_per_layer(stack, output, options),
        Format::SvgCombined => crate::debug::svg::write_combined(stack, output, options),
        Format::PngPerLayer => crate::debug::png::write_per_layer(stack, output, options),
        // The 3MF slice format records one stack per scene leaf; a
        // merged stack has already lost that association, so this
        // entry point cannot produce it.
        Format::ThreeMf => Err(WriterError::SceneContextRequired { format }),
    }
}

/// Write a sliced contour stack as the printer file `format` names,
/// running the schedule in `program`.
///
/// `output` is a regular file path, which is left untouched unless
/// the whole file can be attempted: an unwritable format, a path
/// that is an existing directory, and a program whose layers do not
/// pair with the stack's are all refused before the file is opened.
///
/// # Errors
///
/// [`WriterError::PrinterFormatNotImplemented`] for a format whose
/// writer has not landed, [`WriterError::ProgramLayerCountMismatch`]
/// when the program and the stack disagree on the layer count, and
/// [`WriterError::OutputPathInvalid`] when `output` is a directory.
pub fn write_printer_file(
    stack: &ContourStack,
    output: &Path,
    format: PrinterFileFormat,
    program: &PrintProgram,
) -> Result<(), WriterError> {
    match format {
        PrinterFileFormat::Goo(GooVersion::V3_0) => {}
        PrinterFileFormat::Goo(GooVersion::V5_1)
        | PrinterFileFormat::Ctb(CtbVariant::PlainV5 | CtbVariant::EncryptedV5) => {
            return Err(WriterError::PrinterFormatNotImplemented { format });
        }
    }
    if program.layers.len() != stack.layers().len() {
        return Err(WriterError::ProgramLayerCountMismatch {
            program_layers: program.layers.len(),
            stack_layers: stack.layers().len(),
        });
    }
    validate_file_output_path(output, format)?;

    let mut out = BufWriter::new(File::create(output)?);
    v3_0::file::write_file(program, stack, &mut out)?;
    out.flush().map_err(WriterError::Io)
}

pub(crate) fn validate_output_path(output: &Path, format: Format) -> Result<(), WriterError> {
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
        validate_file_output_path(output, format)
    }
}

fn validate_file_output_path(output: &Path, format: impl fmt::Display) -> Result<(), WriterError> {
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

#[cfg(test)]
mod write_slice_tests {
    use std::fs::File;

    use sc_print_primitives::{AaLevel, BuildVolume, Mirror, PixelResolution};
    use sc_sl1cer::contour::ContourStack;
    use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};
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
    fn svg_per_layer_with_empty_stack_succeeds() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("layers");
        write_slice(
            &empty_stack(),
            &out,
            Format::SvgPerLayer,
            &default_options(),
        )
        .unwrap();
        assert!(out.is_dir());
    }

    #[test]
    fn svg_combined_with_empty_stack_succeeds() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("combined.svg");
        write_slice(
            &empty_stack(),
            &out,
            Format::SvgCombined,
            &default_options(),
        )
        .unwrap();
        assert!(out.is_file());
    }

    #[test]
    fn png_per_layer_with_empty_stack_succeeds() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("layers");
        write_slice(
            &empty_stack(),
            &out,
            Format::PngPerLayer,
            &default_options(),
        )
        .unwrap();
        assert!(out.is_dir());
    }

    #[test]
    fn threemf_through_the_merged_stack_entry_point_is_refused() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.slices.3mf");
        let err =
            write_slice(&empty_stack(), &out, Format::ThreeMf, &default_options()).unwrap_err();
        assert!(matches!(
            err,
            WriterError::SceneContextRequired {
                format: Format::ThreeMf,
            },
        ));
        assert!(!out.exists(), "the refusal must not create the file");
    }

    #[test]
    fn directory_format_creates_missing_output_directory() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("not_there_yet");
        assert!(!out.exists());
        write_slice(
            &empty_stack(),
            &out,
            Format::PngPerLayer,
            &default_options(),
        )
        .unwrap();
        assert!(out.is_dir(), "validation should create the directory");
    }

    #[test]
    fn directory_format_accepts_existing_empty_directory() {
        let dir = tempdir().unwrap();
        write_slice(
            &empty_stack(),
            dir.path(),
            Format::PngPerLayer,
            &default_options(),
        )
        .unwrap();
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
        write_slice(
            &empty_stack(),
            &out,
            Format::SvgCombined,
            &default_options(),
        )
        .unwrap();
        assert!(out.is_file());
    }

    #[test]
    fn file_format_accepts_existing_regular_file() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("overwriteable.svg");
        File::create(&out).unwrap();
        write_slice(
            &empty_stack(),
            &out,
            Format::SvgCombined,
            &default_options(),
        )
        .unwrap();
        assert!(out.is_file());
        let bytes = fs::metadata(&out).unwrap().len();
        assert!(bytes > 0, "writer should have populated the file");
    }
}
