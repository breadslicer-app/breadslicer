/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The conversion pipeline.
//!
//! [`run`] ties the pieces together: it dispatches the request's
//! [`SourceFormat`](crate::format::SourceFormat) to the matching reader,
//! reads the input paths into domain profiles, and hands them to the
//! writer. This is the seam where a new source format is wired in, and
//! the only place that pairs a format with its adapter.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::dragonfruit::{DragonFruitError, DragonFruitReader};
use crate::format::SourceFormat;
use crate::reader::ProfileReader;
use crate::request::ConversionRequest;
use crate::writer::{WriteError, WriteReport, write_profiles};

/// Returned by [`run`] when reading a source or writing the library
/// fails.
#[derive(Debug)]
pub enum RunError {
    Read(DragonFruitError),
    Write(WriteError),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(err) => write!(f, "{err}"),
            Self::Write(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(err) => Some(err),
            Self::Write(err) => Some(err),
        }
    }
}

/// Convert the request's inputs and write the result into its output
/// root, returning the per-kind counts.
pub fn run(request: &ConversionRequest) -> Result<WriteReport, RunError> {
    let paths: Vec<&Path> = request.inputs().iter().map(PathBuf::as_path).collect();
    let profiles = match request.format() {
        SourceFormat::DragonFruit => DragonFruitReader::new()
            .read(&paths)
            .map_err(RunError::Read)?,
    };
    write_profiles(&profiles, request.out(), request.with_images()).map_err(RunError::Write)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::cli::RawCliArgs;

    fn request(inputs: Vec<PathBuf>, out: PathBuf, with_images: bool) -> ConversionRequest {
        ConversionRequest::from_raw(RawCliArgs {
            format: SourceFormat::DragonFruit,
            inputs,
            out,
            with_images,
        })
        .unwrap()
    }

    #[test]
    fn converts_a_dragonfruit_printer_file_end_to_end() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("printers.json");
        fs::write(
            &input,
            r#"[{
                "presetId": "e2e-printer", "profileVersion": 1,
                "manufacturer": "TestCorp", "family": "Series", "name": "E2E",
                "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
                "pixelSize": { "x": 100.0, "y": 100.0 },
                "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
            }]"#,
        )
        .unwrap();
        let out = dir.path().join("library");

        let report = run(&request(vec![input], out.clone(), false)).unwrap();

        assert_eq!(report.printers, 1);
        assert!(out.join("printers").join("e2e-printer.toml").is_file());
    }

    #[test]
    fn reports_a_read_error_from_a_broken_input() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("broken.json");
        fs::write(&input, "{ not json ]").unwrap();
        let out = dir.path().join("library");

        let err = run(&request(vec![input], out, false)).unwrap_err();
        assert!(matches!(err, RunError::Read(_)));
    }
}
