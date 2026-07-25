/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Reading of 3MF project packages.
//!
//! The container is parsed once; the scene loads through the
//! package-level entry of `sc-mesh-formats`, and the project data is
//! then extracted from the same [`Package`]. Extraction never fails
//! a load for data we merely do not understand: an absent project
//! part means a plain model, an unknown payload version or a foreign
//! slicer's project data becomes a warning.

use std::io::{Read, Seek};

use sc_3mf::package::{DecompressionProgress, Package, PartName};
use sc_mesh_formats::threemf::read_threemf_from_package;
use sc_print_primitives::{MaterialId, PrinterId};

use crate::error::ReadThreeMfProjectError;
use crate::messages::ReadProjectWarning;
use crate::payload::{EmbeddedProfile, ProjectPayload};
use crate::read::LoadedProject;
use crate::threemf::foreign::detect_foreign_dialects;
use crate::threemf::payload_part::{PayloadParseOutcome, parse_payload};
use crate::threemf::{
    MATERIAL_EXTENSION_PART_PREFIX, MATERIAL_PROFILE_PART_PREFIX, PRINTER_PROFILE_PART_PREFIX,
    PROFILES_PART_PREFIX, PROJECT_PART_NAME,
};

/// Reads a 3MF package as a project: the scene plus the project
/// payload when the package carries our project part.
pub fn read_threemf_project<R>(reader: &mut R) -> Result<LoadedProject, ReadThreeMfProjectError>
where
    R: Read + Seek,
{
    read_threemf_project_with_progress(reader, |_| {})
}

/// [`read_threemf_project`], reporting container decompression
/// progress. The ticks cover the package extraction only; the model
/// and payload parsing that follow run on the already-inflated bytes.
pub fn read_threemf_project_with_progress<R>(
    reader: &mut R,
    progress: impl FnMut(DecompressionProgress),
) -> Result<LoadedProject, ReadThreeMfProjectError>
where
    R: Read + Seek,
{
    let package = Package::read_from_with_progress(reader, progress)?;
    let (scene, mesh_warnings) = read_threemf_from_package(&package)?;
    let mut warnings: Vec<ReadProjectWarning> = mesh_warnings
        .into_iter()
        .map(ReadProjectWarning::ThreeMf)
        .collect();
    for dialect in detect_foreign_dialects(&package) {
        warnings.push(ReadProjectWarning::ForeignProjectData { dialect });
    }
    let payload = extract_payload(&package, &mut warnings)?;
    Ok(LoadedProject {
        scene,
        warnings,
        payload,
    })
}

/// Extracts the project payload from the package, if present and of
/// a version this build understands.
fn extract_payload(
    package: &Package,
    warnings: &mut Vec<ReadProjectWarning>,
) -> Result<Option<ProjectPayload>, ReadThreeMfProjectError> {
    let part = PartName::new(PROJECT_PART_NAME).expect("constant part name is valid");
    let Some(bytes) = package.part_data(&part) else {
        return Ok(None);
    };
    let text = str::from_utf8(bytes).map_err(ReadThreeMfProjectError::PayloadNotUtf8)?;
    match parse_payload(text).map_err(ReadThreeMfProjectError::PayloadParse)? {
        PayloadParseOutcome::UnsupportedVersion { version } => {
            warnings.push(ReadProjectWarning::ProjectVersionUnsupported { version });
            Ok(None)
        }
        PayloadParseOutcome::V1 { printer, resin } => Ok(Some(ProjectPayload {
            printer,
            resin,
            embedded: collect_embedded(package, warnings),
        })),
    }
}

/// Collects the embedded profile documents, in part order. Parts
/// under the profiles prefix that do not match the expected naming
/// are reported and skipped.
fn collect_embedded(
    package: &Package,
    warnings: &mut Vec<ReadProjectWarning>,
) -> Vec<EmbeddedProfile> {
    let mut embedded = Vec::new();
    for name in package.part_names() {
        let raw = name.as_str();
        if !raw.starts_with(PROFILES_PART_PREFIX) {
            continue;
        }
        match classify_profile_part(raw) {
            Some(kind) => {
                let bytes = package
                    .part_data(name)
                    .expect("iterated part names resolve to their parts")
                    .to_vec();
                embedded.push(match kind {
                    ProfilePartKind::Printer(id) => EmbeddedProfile::Printer { id, bytes },
                    ProfilePartKind::Material(id) => EmbeddedProfile::Material { id, bytes },
                    ProfilePartKind::MaterialExtension(id) => {
                        EmbeddedProfile::MaterialExtension { id, bytes }
                    }
                });
            }
            None => warnings.push(ReadProjectWarning::UnrecognizedProfilePart {
                part_name: raw.to_string(),
            }),
        }
    }
    embedded
}

/// Which profile kind a part name under the profiles prefix maps to.
enum ProfilePartKind {
    Printer(PrinterId),
    Material(MaterialId),
    MaterialExtension(MaterialId),
}

/// Classifies a part name against the embedded-profile naming: one
/// of the three profile directories, a valid id slug as the stem,
/// and a `.toml` suffix. The extensions directory nests inside the
/// materials directory, so it is checked first; the no-slash rule in
/// [`part_stem`] keeps nested names out of the shallower matches.
fn classify_profile_part(name: &str) -> Option<ProfilePartKind> {
    if let Some(stem) = part_stem(name, MATERIAL_EXTENSION_PART_PREFIX) {
        return MaterialId::new(stem)
            .ok()
            .map(ProfilePartKind::MaterialExtension);
    }
    if let Some(stem) = part_stem(name, MATERIAL_PROFILE_PART_PREFIX) {
        return MaterialId::new(stem).ok().map(ProfilePartKind::Material);
    }
    if let Some(stem) = part_stem(name, PRINTER_PROFILE_PART_PREFIX) {
        return PrinterId::new(stem).ok().map(ProfilePartKind::Printer);
    }
    None
}

/// Strips `prefix` and a trailing `.toml` from a part name,
/// rejecting names with a further path separator (nested
/// directories).
fn part_stem<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = name.strip_prefix(prefix)?;
    if rest.contains('/') {
        return None;
    }
    rest.strip_suffix(".toml")
}
