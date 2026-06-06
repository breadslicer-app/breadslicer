/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output format kinds and bindings for printer profiles.
//!
//! [`FormatKind`] enumerates the resin-printer output formats the
//! slicer's planned writer crate targets. The `Unknown(String)`
//! escape hatch lets a profile that names a format the slicer does
//! not yet recognise still parse: the kind is preserved verbatim and
//! consumers can decide whether to refuse it or pass it through.
//!
//! [`OutputFormatBinding`] pairs a format kind with the format's own
//! version when the format has one. Real-world format versions are
//! heterogeneous (uint for CTB / PWMx / SL1, fixed string for Goo,
//! absent for PNG-per-layer exports), so the field is
//! `Option<NonEmptyString>` rather than a typed numeric or semver.
//!
//! [`OutputFormatBindings`] is a typed alias for a non-empty list of
//! bindings, intended to be consumed by the profiles crate's
//! `PrinterCapabilities`.

use nonempty_collections::NEVec;

use crate::non_empty_string::NonEmptyString;

/// Output format kinds the slicer's writer crate plans to support.
/// `Unknown(String)` is the escape hatch for forward compatibility.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FormatKind {
    Sl1,
    Ctb,
    Goo,
    Pwmx,
    PngPerLayer,
    SvgPerLayer,
    SvgCombined,
    Unknown(String),
}

impl FormatKind {
    /// Canonical lowercase identifier used in profile TOML.
    pub fn as_canonical(&self) -> &str {
        match self {
            Self::Sl1 => "sl1",
            Self::Ctb => "ctb",
            Self::Goo => "goo",
            Self::Pwmx => "pwmx",
            Self::PngPerLayer => "png_per_layer",
            Self::SvgPerLayer => "svg_per_layer",
            Self::SvgCombined => "svg_combined",
            Self::Unknown(s) => s,
        }
    }

    /// Parse from the canonical lowercase identifier. Unrecognised
    /// identifiers fall through to [`FormatKind::Unknown`] rather
    /// than failing.
    pub fn from_canonical(s: &str) -> Self {
        match s {
            "sl1" => Self::Sl1,
            "ctb" => Self::Ctb,
            "goo" => Self::Goo,
            "pwmx" => Self::Pwmx,
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

/// One supported output format with an optional format-specific
/// version. Real-world format versions vary in shape (uint for CTB,
/// fixed string for Goo, absent for PNG-per-layer); the field is
/// `Option<NonEmptyString>` to accommodate the union without
/// imposing semver-shaped invariants.
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

/// A non-empty list of output format bindings.
///
/// Used by the profiles crate's `PrinterCapabilities::output_formats`
/// field; the typedef lives here so the primitive crate is the single
/// home for the type referenced by both the profiles crate and any
/// future writer crate.
pub type OutputFormatBindings = NEVec<OutputFormatBinding>;
