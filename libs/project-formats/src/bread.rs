/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Breadslicer's own project file, `.bread` (#145), on top of the
//! `bread-format` crate.
//!
//! The container holds the scene, the supports as data and the project
//! settings; this module writes the settings: a versioned TOML document with
//! the same profile references and print settings as the 3MF project part,
//! plus the embedded profile documents as text. A `.bread` never stores
//! slices: they are rebuilt after opening.

use std::io::{Read, Write};

use sc_print_primitives::{MaterialId, PrinterId};
use sc_scene::Scene;
use serde::{Deserialize, Serialize};

use crate::bread_file::{self, HollowDocument, Opened, SupportDocument};
use crate::error::{ReadBreadProjectError, WriteBreadProjectError};
use crate::messages::ReadProjectWarning;
use crate::payload::{EmbedProfiles, EmbeddedProfile, ProjectPayload, ReviewDecision};
use crate::read::LoadedProject;
use crate::threemf::payload_part::{PrintSection, ProfilesSection};
use crate::write::progress::WriteProgress;

/// Version of the settings document this build reads and writes.
pub const BREAD_SETTINGS_VERSION: i64 = 1;

/// On-disk shape of the settings document (version 1).
#[derive(Debug, Serialize, Deserialize)]
struct SettingsDoc {
    version: i64,
    /// How many build plates the scene lays out (#545); left out with one.
    #[serde(default, skip_serializing_if = "one_plate")]
    plates: u32,
    /// The plates' names (#545), in their order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plate_names: Vec<String>,
    #[serde(default)]
    profiles: ProfilesSection,
    #[serde(default, skip_serializing_if = "PrintSection::is_empty")]
    print: PrintSection,
    /// The embedded profile documents, in payload order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    embedded: Vec<EmbeddedDoc>,
    /// The findings of the review the person chose to ignore (#432).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ignored: Vec<IgnoredDoc>,
}

/// Whether `plates` is the one plate every scene has, not written.
fn one_plate(plates: &u32) -> bool {
    *plates <= 1
}

/// One entry of the `ignored` list.
#[derive(Debug, Serialize, Deserialize)]
struct IgnoredDoc {
    check: String,
    first_layer: u64,
    last_layer: u64,
    focus_um: [i64; 3],
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
pub(crate) fn serialize_settings(
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
        plates: payload.plates,
        plate_names: payload.plate_names.clone(),
        profiles: ProfilesSection {
            printer: payload.printer.clone(),
            resin: payload.resin.clone(),
        },
        print: PrintSection {
            first_layer_height_mm: payload.first_layer_height,
            output_format: payload.output_format.clone(),
        },
        embedded,
        ignored: payload
            .review_decisions
            .iter()
            .map(|decision| IgnoredDoc {
                check: decision.check.clone(),
                first_layer: decision.first_layer,
                last_layer: decision.last_layer,
                focus_um: decision.focus_um,
            })
            .collect(),
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
pub(crate) fn parse_settings(
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
        review_decisions: doc
            .ignored
            .into_iter()
            .map(|entry| ReviewDecision {
                check: entry.check,
                first_layer: entry.first_layer,
                last_layer: entry.last_layer,
                focus_um: entry.focus_um,
            })
            .collect(),
        plates: doc.plates,
        plate_names: doc.plate_names,
    }))
}

/// Writes `scene`, `payload`, `supports` and `hollows` as a `.bread`
/// project. Each support document names its model and the leaf that shows
/// the supports; that leaf is stored with an empty mesh, to be rebuilt from
/// the document after opening. Each hollow document names the leaf that
/// shows the hollow, which keeps it, and the mesh it was hollowed from, which
/// the file stores as one more mesh (#161, #211).
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
    let settings = serialize_settings(payload, embed)?.into_bytes();
    let file = bread_file::to_file(scene, Some(settings), supports, hollows)?;
    progress(WriteProgress::Serialize { done: 1, total: 1 });
    bread_format::write_file(&file, writer).map_err(WriteBreadProjectError::Container)?;
    progress(WriteProgress::Compress { done: 1, total: 1 });
    Ok(())
}

/// Reads a `.bread` project. A file without a settings document, or with one
/// of a version this build does not read, still opens as a project, with an
/// empty payload: its scene sits where it was saved.
///
/// A file saved before #211 keeps its supports and hollows in chunks the
/// crate skips as drafts of the format; they are read here, so that such a
/// file opens with them, and saving it again writes them as they are now.
pub fn read_bread_project<R: Read>(mut reader: R) -> Result<LoadedProject, ReadBreadProjectError> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(ReadBreadProjectError::Io)?;
    let file =
        bread_format::read_file(bytes.as_slice()).map_err(ReadBreadProjectError::Container)?;
    let drafts = if file.supports.is_empty() && file.hollows.is_empty() {
        bread_file::draft_documents(&bytes)
    } else {
        bread_file::Drafts::default()
    };
    drop(bytes);
    let Opened {
        scene,
        settings,
        supports,
        hollows,
        surface_notes,
    } = bread_file::from_file(file, drafts)?;
    let mut warnings: Vec<ReadProjectWarning> = surface_notes
        .iter()
        .map(|note| ReadProjectWarning::SurfacePaintDropped {
            reason: note.to_string(),
        })
        .collect();
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
