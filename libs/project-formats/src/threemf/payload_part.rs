/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Serialization of the project part's TOML document.
//!
//! The document is versioned so future builds can change its shape
//! without breaking older readers: a reader that sees a version it
//! does not understand keeps the scene and ignores the project data
//! (reported as a warning), instead of failing the whole load.
//! Unknown keys inside a known version are ignored, so additive
//! changes do not need a version bump.

use sc_print_primitives::{MaterialId, PositiveFiniteMm, PrinterId};
use serde::{Deserialize, Serialize};

use crate::payload::ProjectPayload;

/// Version of the project-part document this build reads and
/// writes.
pub const PROJECT_PART_VERSION: i64 = 1;

/// On-disk shape of the project part (version 1).
#[derive(Debug, Serialize, Deserialize)]
struct PayloadDoc {
    version: i64,
    #[serde(default)]
    profiles: ProfilesSection,
    /// Omitted from the document while empty; an additive section, so
    /// older readers ignore it without a version bump.
    #[serde(default, skip_serializing_if = "PrintSection::is_empty")]
    print: PrintSection,
    /// Validity metadata for the persisted slices; additive and
    /// omitted while empty.
    #[serde(default, skip_serializing_if = "SlicesSection::is_empty")]
    slices: SlicesSection,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProfilesSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    printer: Option<PrinterId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resin: Option<MaterialId>,
}

/// Per-print slicing settings that live with the project.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PrintSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_layer_height_mm: Option<PositiveFiniteMm>,
}

impl PrintSection {
    fn is_empty(&self) -> bool {
        self.first_layer_height_mm.is_none()
    }
}

/// Validity metadata for the project's persisted slices. The
/// fingerprint is an opaque token, kept and written back verbatim.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SlicesSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fingerprint: Option<String>,
}

impl SlicesSection {
    fn is_empty(&self) -> bool {
        self.fingerprint.is_none()
    }
}

/// The version field alone, parsed first so an unknown version is
/// reported instead of failing on an unknown document shape.
#[derive(Debug, Deserialize)]
struct VersionProbe {
    version: i64,
}

/// What parsing the project part yielded.
// Not `Eq`: `first_layer_height` carries a `PositiveFiniteMm` (an
// `f64` newtype that is only `PartialEq`).
#[derive(Debug, PartialEq)]
pub(crate) enum PayloadParseOutcome {
    /// A version-1 document with its profile references and per-print
    /// settings.
    V1 {
        printer: Option<PrinterId>,
        resin: Option<MaterialId>,
        first_layer_height: Option<PositiveFiniteMm>,
        slice_fingerprint: Option<String>,
    },
    /// The document declares a version this build does not
    /// understand; nothing else was parsed.
    UnsupportedVersion { version: i64 },
}

/// Serializes the payload's references as the project part's TOML
/// text, together with the opaque `slice_fingerprint` token when one
/// is supplied. Embedded documents are not part of this document; they
/// are separate parts.
pub(crate) fn serialize_payload(
    payload: &ProjectPayload,
    slice_fingerprint: Option<&str>,
) -> Result<String, toml_edit::ser::Error> {
    toml_edit::ser::to_string(&PayloadDoc {
        version: PROJECT_PART_VERSION,
        profiles: ProfilesSection {
            printer: payload.printer.clone(),
            resin: payload.resin.clone(),
        },
        print: PrintSection {
            first_layer_height_mm: payload.first_layer_height,
        },
        slices: SlicesSection {
            fingerprint: slice_fingerprint.map(str::to_string),
        },
    })
}

/// Parses the project part's TOML text.
pub(crate) fn parse_payload(text: &str) -> Result<PayloadParseOutcome, toml_edit::de::Error> {
    let probe: VersionProbe = toml_edit::de::from_str(text)?;
    if probe.version != PROJECT_PART_VERSION {
        return Ok(PayloadParseOutcome::UnsupportedVersion {
            version: probe.version,
        });
    }
    let doc: PayloadDoc = toml_edit::de::from_str(text)?;
    Ok(PayloadParseOutcome::V1 {
        printer: doc.profiles.printer,
        resin: doc.profiles.resin,
        first_layer_height: doc.print.first_layer_height_mm,
        slice_fingerprint: doc.slices.fingerprint,
    })
}
