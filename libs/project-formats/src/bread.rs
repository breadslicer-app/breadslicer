/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Breadslicer's own project file, `.bread` (#145), on top of
//! `sc-bread-format`.
//!
//! The container holds the scene, the supports as data and the project
//! settings; this module writes the settings: a versioned TOML document with
//! the same profile references and print settings as the 3MF project part,
//! plus the embedded profile documents as text. A `.bread` never stores
//! slices: they are rebuilt after opening.

use std::io::{Read, Write};

use sc_bread_format::{BreadProject, HollowDocument, SupportDocument};
use sc_print_primitives::{MaterialId, PrinterId};
use sc_scene::Scene;
use serde::{Deserialize, Serialize};

use crate::error::{ReadBreadProjectError, WriteBreadProjectError};
use crate::messages::ReadProjectWarning;
use crate::payload::{EmbedProfiles, EmbeddedProfile, ProjectPayload};
use crate::read::LoadedProject;
use crate::threemf::payload_part::{PrintSection, ProfilesSection};
use crate::write::progress::WriteProgress;

/// Version of the settings document this build reads and writes.
pub const BREAD_SETTINGS_VERSION: i64 = 1;

/// On-disk shape of the settings document (version 1).
#[derive(Debug, Serialize, Deserialize)]
struct SettingsDoc {
    version: i64,
    #[serde(default)]
    profiles: ProfilesSection,
    #[serde(default, skip_serializing_if = "PrintSection::is_empty")]
    print: PrintSection,
    /// The embedded profile documents, in payload order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    embedded: Vec<EmbeddedDoc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EmbeddedDoc {
    kind: EmbeddedKind,
    id: String,
    /// The profile's TOML text, as it sat on disk.
    document: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum EmbeddedKind {
    Printer,
    Material,
    MaterialExtension,
}

/// The version field alone, read first so an unknown version is reported
/// instead of failing on a document shape this build does not know.
#[derive(Debug, Deserialize)]
struct VersionProbe {
    version: i64,
}

/// Serializes `payload` as the settings document. With [`EmbedProfiles::No`]
/// only the references are written.
fn serialize_settings(
    payload: &ProjectPayload,
    embed: EmbedProfiles,
) -> Result<String, WriteBreadProjectError> {
    let embedded = match embed {
        EmbedProfiles::No => Vec::new(),
        EmbedProfiles::Yes => payload
            .embedded
            .iter()
            .map(embedded_doc)
            .collect::<Result<_, _>>()?,
    };
    toml_edit::ser::to_string(&SettingsDoc {
        version: BREAD_SETTINGS_VERSION,
        profiles: ProfilesSection {
            printer: payload.printer.clone(),
            resin: payload.resin.clone(),
        },
        print: PrintSection {
            first_layer_height_mm: payload.first_layer_height,
            output_format: payload.output_format.clone(),
        },
        embedded,
    })
    .map_err(WriteBreadProjectError::Settings)
}

fn embedded_doc(profile: &EmbeddedProfile) -> Result<EmbeddedDoc, WriteBreadProjectError> {
    let (kind, id, bytes) = match profile {
        EmbeddedProfile::Printer { id, bytes } => (EmbeddedKind::Printer, id.as_str(), bytes),
        EmbeddedProfile::Material { id, bytes } => (EmbeddedKind::Material, id.as_str(), bytes),
        EmbeddedProfile::MaterialExtension { id, bytes } => {
            (EmbeddedKind::MaterialExtension, id.as_str(), bytes)
        }
    };
    let document = String::from_utf8(bytes.clone())
        .map_err(|_| WriteBreadProjectError::ProfileNotUtf8 { id: id.to_owned() })?;
    Ok(EmbeddedDoc {
        kind,
        id: id.to_owned(),
        document,
    })
}

/// Parses the settings document: `Ok(None)` with a warning for a version this
/// build does not read. An embedded document whose id is not a valid slug is
/// reported and left out.
fn parse_settings(
    bytes: &[u8],
    warnings: &mut Vec<ReadProjectWarning>,
) -> Result<Option<ProjectPayload>, ReadBreadProjectError> {
    let text = str::from_utf8(bytes).map_err(ReadBreadProjectError::SettingsNotUtf8)?;
    let probe: VersionProbe =
        toml_edit::de::from_str(text).map_err(ReadBreadProjectError::SettingsParse)?;
    if probe.version != BREAD_SETTINGS_VERSION {
        warnings.push(ReadProjectWarning::ProjectVersionUnsupported {
            version: probe.version,
        });
        return Ok(None);
    }
    let doc: SettingsDoc =
        toml_edit::de::from_str(text).map_err(ReadBreadProjectError::SettingsParse)?;
    let mut embedded = Vec::with_capacity(doc.embedded.len());
    for entry in doc.embedded {
        let bytes = entry.document.into_bytes();
        let profile = match entry.kind {
            EmbeddedKind::Printer => PrinterId::new(&entry.id)
                .ok()
                .map(|id| EmbeddedProfile::Printer { id, bytes }),
            EmbeddedKind::Material => MaterialId::new(&entry.id)
                .ok()
                .map(|id| EmbeddedProfile::Material { id, bytes }),
            EmbeddedKind::MaterialExtension => MaterialId::new(&entry.id)
                .ok()
                .map(|id| EmbeddedProfile::MaterialExtension { id, bytes }),
        };
        match profile {
            Some(profile) => embedded.push(profile),
            None => warnings.push(ReadProjectWarning::UnrecognizedProfilePart {
                part_name: entry.id,
            }),
        }
    }
    Ok(Some(ProjectPayload {
        printer: doc.profiles.printer,
        resin: doc.profiles.resin,
        first_layer_height: doc.print.first_layer_height_mm,
        output_format: doc.print.output_format,
        embedded,
    }))
}

/// Writes `scene`, `payload`, `supports` and `hollows` as a `.bread`
/// project. Each support document names the leaf that shows it; that leaf is
/// stored with an empty mesh, to be rebuilt from the document after opening.
/// Each hollow document names the leaf that shows the hollow, which keeps it
/// (#161).
///
/// The container is written in one pass, so `progress` reports the serialize
/// phase as begun and the compress phase as done.
pub fn write_bread_project_with_progress<W: Write>(
    scene: &Scene,
    payload: &ProjectPayload,
    embed: EmbedProfiles,
    supports: &[SupportDocument],
    hollows: &[HollowDocument],
    writer: &mut W,
    mut progress: impl FnMut(WriteProgress),
) -> Result<(), WriteBreadProjectError> {
    progress(WriteProgress::Serialize { done: 0, total: 1 });
    let project = BreadProject {
        scene: scene.clone(),
        settings: Some(serialize_settings(payload, embed)?.into_bytes()),
        supports: supports.to_vec(),
        hollows: hollows.to_vec(),
    };
    progress(WriteProgress::Serialize { done: 1, total: 1 });
    sc_bread_format::write_project(&project, writer).map_err(WriteBreadProjectError::Container)?;
    progress(WriteProgress::Compress { done: 1, total: 1 });
    Ok(())
}

/// Reads a `.bread` project. A file without a settings document, or with one
/// of a version this build does not read, still opens as a project, with an
/// empty payload: its scene sits where it was saved.
pub fn read_bread_project<R: Read>(reader: R) -> Result<LoadedProject, ReadBreadProjectError> {
    let BreadProject {
        scene,
        settings,
        supports,
        hollows,
    } = sc_bread_format::read_project(reader).map_err(ReadBreadProjectError::Container)?;
    let mut warnings = Vec::new();
    let payload = match settings {
        Some(bytes) => parse_settings(&bytes, &mut warnings)?.unwrap_or_default(),
        None => ProjectPayload::default(),
    };
    Ok(LoadedProject {
        scene,
        warnings,
        payload: Some(payload),
        slices: None,
        slice_fingerprint: None,
        supports,
        hollows,
    })
}
