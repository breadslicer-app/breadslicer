/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Discriminator for the `slice --target-format` flag value.
//!
//! The flag spans the two output vocabularies the writer crate keeps
//! apart, and reaches one of its two entry points accordingly:
//! `Format` names the slice outputs `write_slice` produces, and
//! `PrinterFileFormat` names the files `write_printer_file` produces
//! for a machine. Someone naming a format on the command line is
//! choosing between all of them at once, so the parsed value carries
//! whichever vocabulary the word landed in.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use sc_sla_formats::format::format_from_path;
use sc_sla_formats::{Format, GooVersion, PrinterFileFormat, printer_file_format_from_path};

/// Every spelling the flag accepts, in help-text order.
pub(crate) const ACCEPTED_TARGET_FORMATS: &str =
    "svg-per-layer, svg-combined, png-per-layer, 3mf, goo";

/// A `--target-format` flag value: a slice output or a printer file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetFormat {
    Slices(Format),
    PrinterFile(PrinterFileFormat),
}

impl TargetFormat {
    /// Name the target format an output path's extension asks for.
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        format_from_path(path)
            .map(Self::Slices)
            .or_else(|| printer_file_format_from_path(path).map(Self::PrinterFile))
    }

    /// Report whether the target path is a directory to fill rather
    /// than a single file to write.
    pub(crate) fn is_directory_format(self) -> bool {
        match self {
            Self::Slices(format) => format.is_directory_format(),
            Self::PrinterFile(_) => false,
        }
    }
}

impl fmt::Display for TargetFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Slices(format) => format.fmt(f),
            Self::PrinterFile(format) => format.fmt(f),
        }
    }
}

/// Returned when a flag value names no output this build writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnknownTargetFormat {
    value: String,
}

impl fmt::Display for UnknownTargetFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown target format \"{}\"; expected one of: {ACCEPTED_TARGET_FORMATS}",
            self.value,
        )
    }
}

impl std::error::Error for UnknownTargetFormat {}

impl FromStr for TargetFormat {
    type Err = UnknownTargetFormat;

    /// The bare family name `goo` names the one GOO revision that has
    /// a writer here. It stops being unambiguous the day a second
    /// one lands, and has to become an error naming both revisions
    /// rather than keep pointing at V3.0.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(format) = Format::from_str(s) {
            return Ok(Self::Slices(format));
        }
        match s {
            "goo" => Ok(Self::PrinterFile(PrinterFileFormat::Goo(GooVersion::V3_0))),
            other => Err(UnknownTargetFormat {
                value: other.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod target_format_tests {
    use std::path::PathBuf;

    use super::*;

    const GOO_V3_0: TargetFormat =
        TargetFormat::PrinterFile(PrinterFileFormat::Goo(GooVersion::V3_0));

    #[test]
    fn every_slice_output_name_parses_into_the_slice_vocabulary() {
        for (name, format) in [
            ("svg-per-layer", Format::SvgPerLayer),
            ("svg-combined", Format::SvgCombined),
            ("png-per-layer", Format::PngPerLayer),
            ("3mf", Format::ThreeMf),
        ] {
            assert_eq!(
                TargetFormat::from_str(name),
                Ok(TargetFormat::Slices(format)),
                "{name}",
            );
        }
    }

    #[test]
    fn the_bare_goo_family_name_parses_into_the_printer_file_vocabulary() {
        assert_eq!(TargetFormat::from_str("goo"), Ok(GOO_V3_0));
    }

    #[test]
    fn parsing_is_case_sensitive() {
        assert!(TargetFormat::from_str("GOO").is_err());
        assert!(TargetFormat::from_str("SVG-Combined").is_err());
    }

    #[test]
    fn a_format_without_a_writer_is_refused() {
        for name in ["ctb", "sl1", "lumen"] {
            assert!(TargetFormat::from_str(name).is_err(), "{name}");
        }
    }

    #[test]
    fn the_failure_names_the_value_and_every_accepted_spelling() {
        let message = TargetFormat::from_str("xyz").unwrap_err().to_string();
        assert!(message.contains("xyz"), "message: {message}");
        for accepted in ACCEPTED_TARGET_FORMATS.split(", ") {
            assert!(message.contains(accepted), "{accepted} in: {message}");
        }
    }

    #[test]
    fn every_accepted_spelling_parses() {
        for accepted in ACCEPTED_TARGET_FORMATS.split(", ") {
            assert!(TargetFormat::from_str(accepted).is_ok(), "{accepted}");
        }
    }

    #[test]
    fn a_slice_output_extension_infers_its_own_vocabulary() {
        assert_eq!(
            TargetFormat::from_path(&PathBuf::from("plate.svg")),
            Some(TargetFormat::Slices(Format::SvgCombined)),
        );
        assert_eq!(
            TargetFormat::from_path(&PathBuf::from("plate.slices.3mf")),
            Some(TargetFormat::Slices(Format::ThreeMf)),
        );
    }

    #[test]
    fn a_dot_goo_extension_infers_the_printer_file() {
        assert_eq!(
            TargetFormat::from_path(&PathBuf::from("plate.goo")),
            Some(GOO_V3_0)
        );
    }

    #[test]
    fn an_extension_no_writer_claims_infers_nothing() {
        assert_eq!(TargetFormat::from_path(&PathBuf::from("plate.ctb")), None);
        assert_eq!(TargetFormat::from_path(&PathBuf::from("plate")), None);
    }

    #[test]
    fn only_the_per_layer_slice_outputs_fill_a_directory() {
        assert!(TargetFormat::Slices(Format::SvgPerLayer).is_directory_format());
        assert!(TargetFormat::Slices(Format::PngPerLayer).is_directory_format());
        assert!(!TargetFormat::Slices(Format::SvgCombined).is_directory_format());
        assert!(!TargetFormat::Slices(Format::ThreeMf).is_directory_format());
        assert!(!GOO_V3_0.is_directory_format());
    }

    #[test]
    fn a_printer_file_renders_the_way_a_binding_spells_it() {
        assert_eq!(GOO_V3_0.to_string(), "goo@v3.0");
        assert_eq!(
            TargetFormat::Slices(Format::PngPerLayer).to_string(),
            "png-per-layer",
        );
    }
}
