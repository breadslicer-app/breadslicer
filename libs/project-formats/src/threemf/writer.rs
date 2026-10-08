/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Assembly of 3MF project packages.

use std::io::{Cursor, Write};

use sc_3mf::model::{ModelDocument, ObjectPayload, ResourceId, SliceStackContent, write_model};
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

/// Report an accumulated serialized-byte count no more often than every this
/// many bytes, so a large mesh yields a moving bar without a callback per XML
/// event.
const SERIALIZE_REPORT_STEP: u64 = 256 * 1024;

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

    let serialize_total = estimate_model_bytes(&model.document)
        + slice_document.as_ref().map_or(0, estimate_model_bytes);
    let mut serialized: u64 = 0;

    let model_xml = serialize_ticking(
        &model.document,
        serialize_total,
        &mut serialized,
        &mut progress,
    )?;

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

    if let Some(slice_document) = &slice_document {
        let slice_xml = serialize_ticking(
            slice_document,
            serialize_total,
            &mut serialized,
            &mut progress,
        )?;
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

/// Serializes `document` to XML in memory, reporting the running total of
/// serialized bytes as a [`WriteProgress::Serialize`] against
/// `serialize_total`. `serialized` accumulates across successive calls (the
/// mesh model, then the slice model), so the phase advances once across both
/// rather than restarting.
fn serialize_ticking(
    document: &ModelDocument,
    serialize_total: u64,
    serialized: &mut u64,
    progress: &mut impl FnMut(WriteProgress),
) -> Result<Vec<u8>, WriteThreeMfProjectError> {
    let mut writer = SerializeTicker::new(*serialized, serialize_total, progress);
    write_model(document, &mut writer)?;
    // Force a report at the document boundary, so the serialize phase is
    // visible even when the whole document is smaller than 1 report step.
    writer.report_now();
    *serialized = writer.emitted();
    Ok(writer.into_inner())
}

/// A `Write` sink that serializes into a `Vec` while reporting the running
/// serialized-byte total to a progress callback, throttled to 1 report per
/// [`SERIALIZE_REPORT_STEP`].
struct SerializeTicker<'a, F> {
    inner: Vec<u8>,
    base: u64,
    total: u64,
    last_reported: u64,
    progress: &'a mut F,
}

impl<'a, F: FnMut(WriteProgress)> SerializeTicker<'a, F> {
    fn new(base: u64, total: u64, progress: &'a mut F) -> Self {
        Self {
            inner: Vec::new(),
            base,
            total,
            last_reported: base,
            progress,
        }
    }

    /// Returns the total serialized bytes emitted through this sink, counting
    /// the base carried in from earlier documents.
    fn emitted(&self) -> u64 {
        self.base + self.inner.len() as u64
    }

    /// Emits a report at the current emitted total, regardless of the throttle
    /// step.
    fn report_now(&mut self) {
        let emitted = self.emitted();
        self.last_reported = emitted;
        (self.progress)(WriteProgress::Serialize {
            done: emitted,
            total: self.total,
        });
    }

    fn into_inner(self) -> Vec<u8> {
        self.inner
    }
}

impl<F: FnMut(WriteProgress)> Write for SerializeTicker<'_, F> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.extend_from_slice(buf);
        let emitted = self.base + self.inner.len() as u64;
        if emitted - self.last_reported >= SERIALIZE_REPORT_STEP {
            self.last_reported = emitted;
            (self.progress)(WriteProgress::Serialize {
                done: emitted,
                total: self.total,
            });
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Returns a rough estimate of the XML byte size `write_model` produces for
/// `document`, used only as the serialize phase's denominator. Real output can
/// run over or under; a consumer clamps the fraction.
fn estimate_model_bytes(document: &ModelDocument) -> u64 {
    // Per-element byte guesses for the `<vertex>`, `<triangle>`, slice
    // `<vertex>`, and `<segment>` markup plus their numeric payloads.
    const BYTES_PER_VERTEX: u64 = 64;
    const BYTES_PER_TRIANGLE: u64 = 48;
    const BYTES_PER_SLICE_VERTEX: u64 = 40;
    const BYTES_PER_SLICE_SEGMENT: u64 = 24;

    let mut bytes = 0u64;
    for object in &document.objects {
        if let ObjectPayload::Mesh(mesh) = &object.payload {
            bytes += mesh.vertices.len() as u64 * BYTES_PER_VERTEX;
            bytes += mesh.triangles.len() as u64 * BYTES_PER_TRIANGLE;
        }
    }
    for stack in &document.slice_stacks {
        if let SliceStackContent::Slices(slices) = &stack.content {
            for slice in slices {
                bytes += slice.vertices.len() as u64 * BYTES_PER_SLICE_VERTEX;
                for polygon in &slice.polygons {
                    bytes += polygon.segments.len() as u64 * BYTES_PER_SLICE_SEGMENT;
                }
            }
        }
    }
    bytes
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
