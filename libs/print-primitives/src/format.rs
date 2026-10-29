/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output format kinds and bindings for printer profiles.
//!
//! [`FormatKind`] names a resin-printer output format. Its `Unknown(String)`
//! variant keeps a name this workspace does not know, so a profile that
//! declares such a format still parses; consumers decide whether to refuse it
//! or pass it through.
//!
//! [`OutputFormatBinding`] pairs a format kind with the format's own version
//! when the format has one. Real-world format versions are heterogeneous (uint
//! for CTB / PWMx / SL1, fixed string for Goo, absent for PNG-per-layer
//! exports), so the field is an `Option<NonEmptyString>`.
//!
//! [`OutputFormatBindings`] is a non-empty list of bindings.

use std::fmt;
use std::str::FromStr;

use nonempty_collections::NEVec;

use crate::non_empty_string::NonEmptyString;

const VERSION_SEPARATOR: char = '@';

/// A resin-printer output format, by the name a printer profile gives it.
/// `Unknown(String)` keeps a name this workspace does not know.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FormatKind {
    Sl1,
    Ctb,
    Goo,
    Pwmx,
    /// Creality's CXDLP container (Halot printers).
    Cxdlp,
    /// Anycubic's ZIP container family (`.pwsz`, `.pm7`, `.pm7m`, ...): the
    /// version names the variant, which is also the file extension.
    Azf,
    /// Anycubic's binary file family (`.pwmx`, `.pwma`, `.pm3`, `.pws`, ...):
    /// the version names the variant, which is also the file extension.
    Aff,
    PngPerLayer,
    SvgPerLayer,
    SvgCombined,
    Unknown(String),
}

impl FormatKind {
    /// Return the canonical lowercase identifier used in profile TOML.
    pub fn as_canonical(&self) -> &str {
        match self {
            Self::Sl1 => "sl1",
            Self::Ctb => "ctb",
            Self::Goo => "goo",
            Self::Pwmx => "pwmx",
            Self::Cxdlp => "cxdlp",
            Self::Azf => "azf",
            Self::Aff => "aff",
            Self::PngPerLayer => "png_per_layer",
            Self::SvgPerLayer => "svg_per_layer",
            Self::SvgCombined => "svg_combined",
            Self::Unknown(s) => s,
        }
    }

    /// Parse from the canonical lowercase identifier. An unrecognised
    /// identifier becomes [`FormatKind::Unknown`].
    pub fn from_canonical(s: &str) -> Self {
        match s {
            "sl1" => Self::Sl1,
            "ctb" => Self::Ctb,
            "goo" => Self::Goo,
            "pwmx" => Self::Pwmx,
            "cxdlp" => Self::Cxdlp,
            "azf" => Self::Azf,
            "aff" => Self::Aff,
            "png_per_layer" => Self::PngPerLayer,
            "svg_per_layer" => Self::SvgPerLayer,
            "svg_combined" => Self::SvgCombined,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for FormatKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_canonical())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for FormatKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_canonical(&s))
    }
}

/// A supported output format with an optional format-specific version.
/// Real-world format versions vary in shape (uint for CTB, fixed string for
/// Goo, absent for PNG-per-layer); the field is `Option<NonEmptyString>` to
/// hold any of them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct OutputFormatBinding {
    pub format: FormatKind,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub version: Option<NonEmptyString>,
}

impl fmt::Display for OutputFormatBinding {
    /// Write the family and the version joined by `@`, as in `goo@v3.0`, or the
    /// bare family, as in `goo`, when the binding has no version. The version
    /// keeps its case.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let family = self.format.as_canonical();
        match &self.version {
            Some(version) => write!(f, "{family}{VERSION_SEPARATOR}{version}"),
            None => f.write_str(family),
        }
    }
}

impl FromStr for OutputFormatBinding {
    type Err = EmptyBindingPart;

    /// Parse the form [`Display`](fmt::Display) writes: `goo@v3.0`, or `goo`
    /// for a binding without a version.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (family, version) = match s.split_once(VERSION_SEPARATOR) {
            Some((family, version)) => (family, Some(version)),
            None => (s, None),
        };
        if family.is_empty() {
            return Err(EmptyBindingPart::Family);
        }
        let version = version
            .map(|version| NonEmptyString::new(version).map_err(|_| EmptyBindingPart::Version))
            .transpose()?;
        Ok(Self {
            format: FormatKind::from_canonical(family),
            version,
        })
    }
}

/// Returned when a binding's text form leaves its family or its version empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyBindingPart {
    /// Nothing comes before the `@`, or the text is empty.
    Family,
    /// Nothing comes after the `@`.
    Version,
}

impl fmt::Display for EmptyBindingPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Family => f.write_str("the binding names no format family"),
            Self::Version => write!(
                f,
                "the binding has no version after \"{VERSION_SEPARATOR}\""
            ),
        }
    }
}

impl std::error::Error for EmptyBindingPart {}

/// A non-empty list of output format bindings.
pub type OutputFormatBindings = NEVec<OutputFormatBinding>;
