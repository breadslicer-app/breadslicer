/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The selectable source formats.
//!
//! [`SourceFormat`] names each input format the converter can read. It
//! is chosen explicitly on the command line (`--format`), never guessed
//! from the input. Adding a format means adding a variant here and the
//! adapter it dispatches to; nothing in the core writer changes.

use std::fmt;
use std::str::FromStr;

/// A source profile format the converter knows how to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFormat {
    /// The DragonFruit slicer's plugin JSON profiles.
    DragonFruit,
    /// ELEGOO's published resin settings spreadsheet.
    ElegooSettingsSheet,
    /// Anycubic's resin settings wiki page, saved as HTML.
    AnycubicWikiPage,
    /// Creality's resin settings blog post, saved as HTML.
    CrealityBlogPost,
    /// UVtools' printer catalog: the PrusaSlicer printer profiles it
    /// bundles.
    UvtoolsCatalog,
}

impl SourceFormat {
    /// The stable command-line token for this format.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DragonFruit => "dragonfruit",
            Self::ElegooSettingsSheet => "elegoo-settings",
            Self::AnycubicWikiPage => "anycubic-wiki",
            Self::CrealityBlogPost => "creality-blog",
            Self::UvtoolsCatalog => "uvtools",
        }
    }
}

impl fmt::Display for SourceFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned by [`SourceFormat::from_str`] when the `--format` value
/// names no known format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownSourceFormat {
    pub got: String,
}

impl fmt::Display for UnknownSourceFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown source format \"{}\"; supported: dragonfruit, elegoo-settings, anycubic-wiki, creality-blog, uvtools",
            self.got,
        )
    }
}

impl std::error::Error for UnknownSourceFormat {}

impl FromStr for SourceFormat {
    type Err = UnknownSourceFormat;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "dragonfruit" => Ok(Self::DragonFruit),
            "elegoo-settings" => Ok(Self::ElegooSettingsSheet),
            "anycubic-wiki" => Ok(Self::AnycubicWikiPage),
            "creality-blog" => Ok(Self::CrealityBlogPost),
            "uvtools" => Ok(Self::UvtoolsCatalog),
            other => Err(UnknownSourceFormat {
                got: other.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_dragonfruit_token() {
        assert_eq!("dragonfruit".parse(), Ok(SourceFormat::DragonFruit));
    }

    #[test]
    fn as_str_round_trips_through_from_str() {
        let format = SourceFormat::DragonFruit;
        assert_eq!(format.as_str().parse(), Ok(format));
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(SourceFormat::DragonFruit.to_string(), "dragonfruit");
    }

    #[test]
    fn parses_the_elegoo_settings_token() {
        assert_eq!(
            "elegoo-settings".parse(),
            Ok(SourceFormat::ElegooSettingsSheet)
        );
        assert_eq!(
            SourceFormat::ElegooSettingsSheet.to_string(),
            "elegoo-settings"
        );
    }

    #[test]
    fn every_format_round_trips_through_its_token() {
        for format in [
            SourceFormat::DragonFruit,
            SourceFormat::ElegooSettingsSheet,
            SourceFormat::AnycubicWikiPage,
            SourceFormat::CrealityBlogPost,
            SourceFormat::UvtoolsCatalog,
        ] {
            assert_eq!(format.as_str().parse(), Ok(format));
        }
    }

    #[test]
    fn rejects_an_unknown_token() {
        let err = "lychee".parse::<SourceFormat>().unwrap_err();
        assert_eq!(err.got, "lychee");
        assert!(err.to_string().contains("dragonfruit"));
    }
}
