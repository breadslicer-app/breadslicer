/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output format catalogue and inference from file paths.
//!
//! [`Format`] enumerates every writer this crate exposes. Each
//! variant knows whether its on-disk shape is a single file (e.g.
//! one combined SVG, a `.sl1` package) or a directory of per-layer
//! files; the [`write_slice`](crate::write_slice) dispatcher uses
//! that bit to validate the output path before calling the writer.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// On-disk output format.
///
/// Variants are kebab-cased in CLI input via [`FromStr`]:
/// `svg-per-layer`, `svg-combined`, `png-per-layer`. As more writers
/// land (SL1, CTB, GOO, 3MF Slice) they extend this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// One SVG file per layer, written into a target directory.
    SvgPerLayer,
    /// A single SVG file containing every layer as a `<g>` group.
    SvgCombined,
    /// One 8-bit grayscale PNG per layer, written into a target
    /// directory.
    PngPerLayer,
}

impl Format {
    /// Whether this format writes one file per layer into an output
    /// directory (`true`) or a single packed file at the output path
    /// (`false`).
    #[inline]
    pub fn is_directory_format(self) -> bool {
        match self {
            Self::SvgPerLayer | Self::PngPerLayer => true,
            Self::SvgCombined => false,
        }
    }

    /// Canonical kebab-cased name used in CLI input and error
    /// messages.
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SvgPerLayer => "svg-per-layer",
            Self::SvgCombined => "svg-combined",
            Self::PngPerLayer => "png-per-layer",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned when [`Format::from_str`] receives an unknown name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownFormat {
    pub got: String,
}

impl fmt::Display for UnknownFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown format \"{}\"; expected one of: svg-per-layer, svg-combined, png-per-layer",
            self.got,
        )
    }
}

impl std::error::Error for UnknownFormat {}

impl FromStr for Format {
    type Err = UnknownFormat;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "svg-per-layer" => Ok(Self::SvgPerLayer),
            "svg-combined" => Ok(Self::SvgCombined),
            "png-per-layer" => Ok(Self::PngPerLayer),
            other => Err(UnknownFormat {
                got: other.to_owned(),
            }),
        }
    }
}

/// Infer a [`Format`] from an output path's extension.
///
/// Today the only file-target format is [`Format::SvgCombined`]
/// (`.svg`). Directory-target formats have no extension to infer
/// from, so callers must supply an explicit `--format` for them.
/// As packed printer formats land (`.sl1`, `.ctb`, `.goo`, ...) they
/// extend the match below.
pub fn format_from_path(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "svg" => Some(Format::SvgCombined),
        _ => None,
    }
}

#[cfg(test)]
mod format_tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn from_str_parses_svg_per_layer() {
        assert_eq!(Format::from_str("svg-per-layer"), Ok(Format::SvgPerLayer));
    }

    #[test]
    fn from_str_parses_svg_combined() {
        assert_eq!(Format::from_str("svg-combined"), Ok(Format::SvgCombined));
    }

    #[test]
    fn from_str_parses_png_per_layer() {
        assert_eq!(Format::from_str("png-per-layer"), Ok(Format::PngPerLayer));
    }

    #[test]
    fn from_str_rejects_unknown_name() {
        let err = Format::from_str("ctb").unwrap_err();
        assert_eq!(err.got, "ctb");
    }

    #[test]
    fn from_str_is_case_sensitive() {
        assert!(Format::from_str("SVG-Per-Layer").is_err());
    }

    #[test]
    fn unknown_format_display_lists_valid_choices() {
        let msg = format!("{}", UnknownFormat { got: "xyz".into() });
        assert!(msg.contains("xyz"), "msg: {msg}");
        assert!(msg.contains("svg-per-layer"), "msg: {msg}");
        assert!(msg.contains("svg-combined"), "msg: {msg}");
        assert!(msg.contains("png-per-layer"), "msg: {msg}");
    }

    #[test]
    fn is_directory_format_for_per_layer_variants() {
        assert!(Format::SvgPerLayer.is_directory_format());
        assert!(Format::PngPerLayer.is_directory_format());
    }

    #[test]
    fn is_directory_format_for_combined_svg_is_false() {
        assert!(!Format::SvgCombined.is_directory_format());
    }

    #[test]
    fn as_str_round_trips_through_from_str() {
        for f in [
            Format::SvgPerLayer,
            Format::SvgCombined,
            Format::PngPerLayer,
        ] {
            assert_eq!(Format::from_str(f.as_str()), Ok(f));
        }
    }

    #[test]
    fn format_from_path_returns_combined_for_dot_svg() {
        assert_eq!(
            format_from_path(&PathBuf::from("out.svg")),
            Some(Format::SvgCombined),
        );
    }

    #[test]
    fn format_from_path_is_case_insensitive_on_extension() {
        assert_eq!(
            format_from_path(&PathBuf::from("OUT.SVG")),
            Some(Format::SvgCombined),
        );
    }

    #[test]
    fn format_from_path_returns_none_for_unknown_extension() {
        assert_eq!(format_from_path(&PathBuf::from("out.ctb")), None);
    }

    #[test]
    fn format_from_path_returns_none_for_no_extension() {
        assert_eq!(format_from_path(&PathBuf::from("out")), None);
    }

    #[test]
    fn format_display_renders_kebab_name() {
        assert_eq!(format!("{}", Format::SvgPerLayer), "svg-per-layer");
        assert_eq!(format!("{}", Format::SvgCombined), "svg-combined");
        assert_eq!(format!("{}", Format::PngPerLayer), "png-per-layer");
    }
}
