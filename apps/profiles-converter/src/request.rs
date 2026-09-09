/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The validated conversion request.
//!
//! [`ConversionRequest`] is the runnable shape of the command line: the
//! chosen source format, a non-empty list of input paths (so "convert
//! nothing" cannot be represented), the output root, and whether preview
//! images are copied. Turning the raw parsed arguments into it is the
//! single point where the "at least one input" rule is enforced.

use std::fmt;
use std::path::{Path, PathBuf};

use nonempty_collections::NEVec;

use crate::cli::RawCliArgs;
use crate::format::SourceFormat;

/// A conversion the tool can actually run: the source format, a
/// non-empty set of input paths, the destination library root, and the
/// image-copying choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversionRequest {
    format: SourceFormat,
    inputs: NEVec<PathBuf>,
    out: PathBuf,
    with_images: bool,
    printer_library: Option<PathBuf>,
}

/// Returned by [`ConversionRequest::from_raw`] when no input path was
/// given, so there would be nothing to convert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoInputsGiven;

impl fmt::Display for NoInputsGiven {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no input paths given; pass at least one input file or directory")
    }
}

impl std::error::Error for NoInputsGiven {}

impl ConversionRequest {
    /// Validate the raw arguments into a runnable request. Fails with
    /// [`NoInputsGiven`] when no input path was provided.
    pub fn from_raw(raw: RawCliArgs) -> Result<Self, NoInputsGiven> {
        let inputs = NEVec::try_from_vec(raw.inputs).ok_or(NoInputsGiven)?;
        Ok(Self {
            format: raw.format,
            inputs,
            out: raw.out,
            with_images: raw.with_images,
            printer_library: raw.printer_library,
        })
    }

    /// The source format to read.
    pub fn format(&self) -> SourceFormat {
        self.format
    }

    /// The input paths to convert.
    pub fn inputs(&self) -> &NEVec<PathBuf> {
        &self.inputs
    }

    /// The destination profile-library root.
    pub fn out(&self) -> &Path {
        self.out.as_path()
    }

    /// Whether preview images are copied.
    pub fn with_images(&self) -> bool {
        self.with_images
    }

    /// The profile library to consult for what each machine can do, if
    /// one was given.
    pub fn printer_library(&self) -> Option<&Path> {
        self.printer_library.as_deref()
    }

    /// A one-line human-facing description of what this request covers.
    pub fn summary(&self) -> String {
        let printers = match &self.printer_library {
            Some(path) => path.display().to_string(),
            None => "none".to_owned(),
        };
        format!(
            "format: {}; inputs: {}; output: {}; images: {}; printer library: {printers}",
            self.format,
            self.inputs.iter().count(),
            self.out.display(),
            if self.with_images { "on" } else { "off" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> RawCliArgs {
        RawCliArgs {
            format: SourceFormat::DragonFruit,
            inputs: Vec::new(),
            out: PathBuf::from("/dst"),
            with_images: false,
            printer_library: None,
        }
    }

    #[test]
    fn no_inputs_is_rejected() {
        let err = ConversionRequest::from_raw(raw()).unwrap_err();
        assert_eq!(err, NoInputsGiven);
        assert!(err.to_string().contains("at least one"));
    }

    #[test]
    fn accepts_a_non_empty_input_list() {
        let request = ConversionRequest::from_raw(RawCliArgs {
            inputs: vec![PathBuf::from("/src/a.json"), PathBuf::from("/src/b.json")],
            ..raw()
        })
        .unwrap();
        assert_eq!(
            request.summary(),
            "format: dragonfruit; inputs: 2; output: /dst; images: off; printer library: none",
        );
    }

    #[test]
    fn summary_names_the_printer_library_when_one_is_given() {
        let request = ConversionRequest::from_raw(RawCliArgs {
            inputs: vec![PathBuf::from("/src/a.json")],
            printer_library: Some(PathBuf::from("/lib")),
            ..raw()
        })
        .unwrap();
        assert_eq!(
            request.summary(),
            "format: dragonfruit; inputs: 1; output: /dst; images: off; printer library: /lib",
        );
        assert_eq!(request.printer_library(), Some(Path::new("/lib")));
    }

    #[test]
    fn summary_reports_images_on() {
        let request = ConversionRequest::from_raw(RawCliArgs {
            inputs: vec![PathBuf::from("/src/a.json")],
            with_images: true,
            ..raw()
        })
        .unwrap();
        assert_eq!(
            request.summary(),
            "format: dragonfruit; inputs: 1; output: /dst; images: on; printer library: none",
        );
    }
}
