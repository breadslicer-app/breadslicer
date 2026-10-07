/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Reading of 3MF project packages.
//!
//! The container is parsed once; the scene loads through the package-level
//! entry of `sc-mesh-formats`, and the project data is then extracted from the
//! same [`Package`]. Extraction never fails a load for data we merely do not
//! understand: an absent project part means a plain model, an unknown payload
//! version or a foreign slicer's project data becomes a warning.

use std::io::{Read, Seek};

use sc_3mf::model::parse_model;
use sc_3mf::package::{DecompressionProgress, Package, PartName};
use sc_mesh_formats::threemf::read_threemf_from_package;
use sc_print_primitives::{MaterialId, PrinterId};
use sc_sl1cer::contour::ContourStack;

use crate::error::ReadThreeMfProjectError;
use crate::messages::ReadProjectWarning;
use crate::payload::{EmbeddedProfile, ProjectPayload};
use crate::read::LoadedProject;
use crate::threemf::foreign::detect_foreign_dialects;
use crate::threemf::payload_part::{PayloadParseOutcome, parse_payload};
use crate::threemf::slice_mapping::contours_from_slice_stack;
use crate::threemf::{
    MATERIAL_EXTENSION_PART_PREFIX, MATERIAL_PROFILE_PART_PREFIX, PRINTER_PROFILE_PART_PREFIX,
    PROFILES_PART_PREFIX, PROJECT_PART_NAME, SLICE_PART_NAME,
};

/// Reads a 3MF package as a project: the scene plus the project payload when
/// the package carries our project part.
pub fn read_threemf_project<R>(reader: &mut R) -> Result<LoadedProject, ReadThreeMfProjectError>
where
    R: Read + Seek,
{
    read_threemf_project_with_progress(reader, |_| {})
}

/// Reads as [`read_threemf_project`] does, and reports container decompression
/// progress. The ticks cover the package extraction only; the model and payload
/// parsing that follow run on the already-inflated bytes.
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
    let (payload, slice_fingerprint) = extract_payload(&package, &mut warnings)?;
    let slices = extract_slices(&package);
    Ok(LoadedProject {
        scene,
        warnings,
        payload,
        slices,
        slice_fingerprint,
    })
}

/// Reads the standalone slice model part and maps it back to a contour stack,
/// or `None` when the package carries no slice part. A slice part that does not
/// parse or does not map back (a corrupt or foreign-shaped part) is also
/// treated as absent, silently: the scene loads slice-less, which is exactly
/// what the caller does when the slices fail their fingerprint check, so a bad
/// part only ever drops slices that would not have survived revalidation
/// anyway.
fn extract_slices(package: &Package) -> Option<ContourStack> {
    let part = PartName::new(SLICE_PART_NAME).expect("constant part name is valid");
    let bytes = package.part_data(&part)?;
    let document = parse_model(bytes).ok()?;
    let stack = document.slice_stacks.first()?;
    contours_from_slice_stack(stack).ok()
}

/// Extracts the project payload and the opaque persisted-slices token from the
/// package, if present and of a version this build understands.
fn extract_payload(
    package: &Package,
    warnings: &mut Vec<ReadProjectWarning>,
) -> Result<(Option<ProjectPayload>, Option<String>), ReadThreeMfProjectError> {
    let part = PartName::new(PROJECT_PART_NAME).expect("constant part name is valid");
    let Some(bytes) = package.part_data(&part) else {
        return Ok((None, None));
    };
    let text = str::from_utf8(bytes).map_err(ReadThreeMfProjectError::PayloadNotUtf8)?;
    match parse_payload(text).map_err(ReadThreeMfProjectError::PayloadParse)? {
        PayloadParseOutcome::UnsupportedVersion { version } => {
            warnings.push(ReadProjectWarning::ProjectVersionUnsupported { version });
            Ok((None, None))
        }
        PayloadParseOutcome::V1 {
            printer,
            resin,
            first_layer_height,
            output_format,
            slice_fingerprint,
        } => Ok((
            Some(ProjectPayload {
                printer,
                resin,
                first_layer_height,
                output_format,
                embedded: collect_embedded(package, warnings),
            }),
            slice_fingerprint,
        )),
    }
}

/// Collects the embedded profile documents, in part order. Parts under the
/// profiles prefix that do not match the expected naming are reported and
/// skipped.
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
        match classify_profile_part(name) {
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

const PROFILE_PART_EXTENSION: &str = "toml";

/// Classifies a part name against the embedded-profile naming: 1 of the 3
/// profile directories, a valid id slug as the stem, and a `.toml` suffix.
fn classify_profile_part(name: &PartName) -> Option<ProfilePartKind> {
    if let Some(stem) = name.file_stem_in(MATERIAL_EXTENSION_PART_PREFIX, PROFILE_PART_EXTENSION) {
        return MaterialId::new(stem)
            .ok()
            .map(ProfilePartKind::MaterialExtension);
    }
    if let Some(stem) = name.file_stem_in(MATERIAL_PROFILE_PART_PREFIX, PROFILE_PART_EXTENSION) {
        return MaterialId::new(stem).ok().map(ProfilePartKind::Material);
    }
    if let Some(stem) = name.file_stem_in(PRINTER_PROFILE_PART_PREFIX, PROFILE_PART_EXTENSION) {
        return PrinterId::new(stem).ok().map(ProfilePartKind::Printer);
    }
    None
}
