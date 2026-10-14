/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Assembly of 3MF project packages.

use std::io::{Cursor, Write};

use sc_3mf::error::ModelWriteError;
use sc_3mf::model::{ModelDocument, ModelSerialization, ResourceId, SerializationProgress};
use sc_3mf::names::{
    DEFAULT_MODEL_PART_NAME, MODEL_PART_CONTENT_TYPE, MUST_PRESERVE_REL_TYPE, START_PART_REL_TYPE,
};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_formats::threemf::{add_declared_facet_normals_parts, scene_to_model_document};
use sc_scene::Scene;

use crate::error::WriteThreeMfProjectError;
use crate::payload::{EmbedProfiles, EmbeddedProfile, ProjectPayload};
use crate::slices::PlateSlices;
use crate::threemf::payload_part::serialize_payload;
use crate::threemf::slice_mapping::slice_stack_from_contours;
use crate::threemf::{
    MATERIAL_EXTENSION_PART_PREFIX, MATERIAL_PROFILE_PART_PREFIX, PRINTER_PROFILE_PART_PREFIX,
    PROFILE_PART_CONTENT_TYPE, PROJECT_PART_CONTENT_TYPE, PROJECT_PART_NAME, PROJECT_REL_TYPE,
    SLICE_PART_NAME, SLICE_REL_TYPE,
};
use crate::write::WriteProgress;

/// Writes `scene` and `payload` as a 3MF project package: the parts a plain 3MF
/// export would carry, plus the project part and, with [`EmbedProfiles::Yes`],
/// 1 part per embedded profile document. When `slices` is `Some`, the merged
/// slice result is written to its own model part, standalone from the scene,
/// and `slice_fingerprint`, when present, is recorded in the project part as
/// that slice data's opaque validity token. The zip layer needs a seekable
/// sink, so the package is serialized into memory first and then copied to
/// `writer`.
pub fn write_threemf_project<W>(
    scene: &Scene,
    payload: &ProjectPayload,
    embed: EmbedProfiles,
    slices: Option<&PlateSlices>,
    slice_fingerprint: Option<&str>,
    writer: &mut W,
) -> Result<(), WriteThreeMfProjectError>
where
    W: Write,
{
    write_threemf_project_with_progress(
        scene,
        payload,
        embed,
        slices,
        slice_fingerprint,
        writer,
        |_| {},
    )
}

/// Writes as [`write_threemf_project`] does, and reports serialize and compress
/// progress through `progress`.
pub fn write_threemf_project_with_progress<W>(
    scene: &Scene,
    payload: &ProjectPayload,
    embed: EmbedProfiles,
    slices: Option<&PlateSlices>,
    slice_fingerprint: Option<&str>,
    writer: &mut W,
    mut progress: impl FnMut(WriteProgress),
) -> Result<(), WriteThreeMfProjectError>
where
    W: Write,
{
    let model = scene_to_model_document(scene)?;

    // Build the standalone slice document up front, if present, so its size
    // feeds the serialize estimate before any XML is written.
    let slice_document = match slices {
        Some(plate) => {
            // The sole resource in the standalone slice document.
            const SLICE_STACK_ID: ResourceId = ResourceId::MIN;
            let stack =
                slice_stack_from_contours(&plate.contours, &plate.slab_tops, SLICE_STACK_ID)?;
            Some(ModelDocument {
                slice_stacks: vec![stack],
                ..ModelDocument::default()
            })
        }
        None => None,
    };

    let (model_xml, slice_xml) =
        serialize_documents(&model.document, slice_document.as_ref(), &mut progress)?;

    let mut package = Package::new();
    let model_part = known_part_name(DEFAULT_MODEL_PART_NAME);
    package.add_part(model_part, MODEL_PART_CONTENT_TYPE, model_xml)?;

    let project_part = known_part_name(PROJECT_PART_NAME);
    let payload_toml = serialize_payload(payload, slice_fingerprint)?;
    package.add_part(
        project_part,
        PROJECT_PART_CONTENT_TYPE,
        payload_toml.into_bytes(),
    )?;

    let mut rel_ids = RelationshipIds::default();
    package.add_root_relationship(Relationship {
        id: rel_ids.next(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: DEFAULT_MODEL_PART_NAME.to_string(),
    });
    package.add_root_relationship(Relationship {
        id: rel_ids.next(),
        rel_type: PROJECT_REL_TYPE.to_string(),
        target: PROJECT_PART_NAME.to_string(),
    });
    package.add_root_relationship(Relationship {
        id: rel_ids.next(),
        rel_type: MUST_PRESERVE_REL_TYPE.to_string(),
        target: PROJECT_PART_NAME.to_string(),
    });
    add_declared_facet_normals_parts(&mut package, &model.mesh_objects)?;

    if let Some(slice_xml) = slice_xml {
        package.add_part(
            known_part_name(SLICE_PART_NAME),
            MODEL_PART_CONTENT_TYPE,
            slice_xml,
        )?;
        package.add_root_relationship(Relationship {
            id: rel_ids.next(),
            rel_type: SLICE_REL_TYPE.to_string(),
            target: SLICE_PART_NAME.to_string(),
        });
        package.add_root_relationship(Relationship {
            id: rel_ids.next(),
            rel_type: MUST_PRESERVE_REL_TYPE.to_string(),
            target: SLICE_PART_NAME.to_string(),
        });
    }

    if embed == EmbedProfiles::Yes {
        for profile in &payload.embedded {
            let (name, bytes) = profile_part(profile);
            package.add_part(name.clone(), PROFILE_PART_CONTENT_TYPE, bytes.to_vec())?;
            package.add_root_relationship(Relationship {
                id: rel_ids.next(),
                rel_type: MUST_PRESERVE_REL_TYPE.to_string(),
                target: name.as_str().to_string(),
            });
        }
    }

    let mut buffer = Cursor::new(Vec::new());
    package.write_to_with_progress(&mut buffer, |done, total| {
        progress(WriteProgress::Compress { done, total })
    })?;
    writer.write_all(buffer.get_ref())?;
    Ok(())
}

/// Serializes the scene document and, when present, the slice document in 1
/// phase, and returns their XML in that order.
fn serialize_documents(
    scene: &ModelDocument,
    slices: Option<&ModelDocument>,
    progress: &mut impl FnMut(WriteProgress),
) -> Result<(Vec<u8>, Option<Vec<u8>>), ModelWriteError> {
    let mut report = |serialization: SerializationProgress| {
        progress(WriteProgress::Serialize {
            done: serialization.serialized_bytes,
            total: serialization.estimated_total_bytes,
        })
    };
    match slices {
        None => {
            let [scene_xml] = ModelSerialization::new([scene]).write(&mut report)?;
            Ok((scene_xml, None))
        }
        Some(slices) => {
            let [scene_xml, slice_xml] =
                ModelSerialization::new([scene, slices]).write(&mut report)?;
            Ok((scene_xml, Some(slice_xml)))
        }
    }
}

/// Sequential `rel-<n>` relationship ids for the package root.
#[derive(Default)]
struct RelationshipIds {
    issued: u32,
}

impl RelationshipIds {
    fn next(&mut self) -> String {
        self.issued += 1;
        format!("rel-{}", self.issued)
    }
}

/// Builds a package part name from a string that is statically known to be
/// well-formed: the module's part-name constants, and profile part names
/// assembled from already-validated id slugs. A malformed name here is a
/// programming error, not a runtime input, so it is not a write failure mode.
fn known_part_name(name: &str) -> PartName {
    PartName::new(name).expect("part name built from a known-good source is well-formed")
}

/// Returns the part name and bytes of an embedded profile document. The id slug
/// grammar (lowercase alphanumerics and hyphens) keeps every generated part
/// name well-formed.
fn profile_part(profile: &EmbeddedProfile) -> (PartName, &[u8]) {
    let (raw_name, bytes) = match profile {
        EmbeddedProfile::Printer { id, bytes } => (
            format!("{PRINTER_PROFILE_PART_PREFIX}{}.toml", id.as_str()),
            bytes,
        ),
        EmbeddedProfile::Material { id, bytes } => (
            format!("{MATERIAL_PROFILE_PART_PREFIX}{}.toml", id.as_str()),
            bytes,
        ),
        EmbeddedProfile::MaterialExtension { id, bytes } => (
            format!("{MATERIAL_EXTENSION_PART_PREFIX}{}.toml", id.as_str()),
            bytes,
        ),
    };
    let name = known_part_name(&raw_name);
    (name, bytes.as_slice())
}
