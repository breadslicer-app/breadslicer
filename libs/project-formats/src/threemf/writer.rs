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
use sc_mesh_core::IndexedMesh;
use sc_mesh_formats::threemf::{
    SceneModel, add_declared_facet_normals_parts, scene_to_model_document,
};
use sc_scene::Scene;

use crate::error::WriteThreeMfProjectError;
use crate::payload::{EmbedProfiles, EmbeddedProfile, ProjectPayload};
use crate::slices::PlateSlices;
use crate::threemf::names::{
    MATERIAL_EXTENSION_PART_PREFIX, MATERIAL_PROFILE_PART_PREFIX, PRINTER_PROFILE_PART_PREFIX,
    PROFILE_PART_CONTENT_TYPE, PROJECT_PART_CONTENT_TYPE, PROJECT_PART_NAME, PROJECT_REL_TYPE,
    SLICE_PART_NAME, SLICE_REL_TYPE,
};
use crate::threemf::payload_part::serialize_payload;
use crate::threemf::slice_mapping::slice_stack_from_contours;
use crate::write::progress::WriteProgress;

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
    let (scene_model, slice_document) = build_documents(scene, slices)?;
    let (model_xml, slice_xml) = serialize_documents(
        &scene_model.document,
        slice_document.as_ref(),
        &mut progress,
    )?;
    let package = assemble_package(
        model_xml,
        &scene_model.mesh_objects,
        slice_xml,
        payload,
        embed,
        slice_fingerprint,
    )?;
    compress_into(&package, writer, progress)
}

/// Builds the scene document and, when `slices` is `Some`, the standalone slice
/// document.
fn build_documents<'s>(
    scene: &'s Scene,
    slices: Option<&PlateSlices>,
) -> Result<(SceneModel<'s>, Option<ModelDocument>), WriteThreeMfProjectError> {
    let scene_model = scene_to_model_document(scene)?;
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
    Ok((scene_model, slice_document))
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

fn assemble_package(
    model_xml: Vec<u8>,
    mesh_objects: &[(ResourceId, &IndexedMesh)],
    slice_xml: Option<Vec<u8>>,
    payload: &ProjectPayload,
    embed: EmbedProfiles,
    slice_fingerprint: Option<&str>,
) -> Result<Package, WriteThreeMfProjectError> {
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
    add_declared_facet_normals_parts(&mut package, mesh_objects)?;

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
    Ok(package)
}

fn compress_into<W: Write>(
    package: &Package,
    writer: &mut W,
    mut progress: impl FnMut(WriteProgress),
) -> Result<(), WriteThreeMfProjectError> {
    let mut buffer = Cursor::new(Vec::new());
    package.write_to_with_progress(&mut buffer, |done, total| {
        progress(WriteProgress::Compress { done, total })
    })?;
    writer.write_all(buffer.get_ref())?;
    Ok(())
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

#[cfg(test)]
mod build_documents_tests {
    use super::*;
    use crate::threemf::slice_mapping::SliceMappingError;
    use crate::threemf::test_fixtures::{cube_scene, plate_slices};

    #[test]
    fn without_slices_there_is_no_slice_document() {
        let scene = cube_scene();
        let (scene_model, slice_document) =
            build_documents(&scene, None).expect("the cube converts");
        assert_eq!(scene_model.mesh_objects.len(), 1);
        assert_eq!(slice_document, None);
    }

    #[test]
    fn the_slice_document_holds_only_the_slice_stack() {
        let scene = cube_scene();
        let (_, slice_document) =
            build_documents(&scene, Some(&plate_slices())).expect("the slices map");
        let document = slice_document.expect("slices give a slice document");
        assert_eq!(document.slice_stacks.len(), 1);
        assert_eq!(document.slice_stacks[0].id, ResourceId::MIN);
        assert!(document.objects.is_empty());
        assert!(document.build.is_empty());
    }

    #[test]
    fn a_slice_mapping_failure_is_returned() {
        let scene = cube_scene();
        let mut plate = plate_slices();
        plate.slab_tops.pop();
        let result = build_documents(&scene, Some(&plate));
        assert!(
            matches!(
                result,
                Err(WriteThreeMfProjectError::SliceMapping(
                    SliceMappingError::LayerCountMismatch { layers: 2, tops: 1 }
                ))
            ),
            "{result:?}"
        );
    }
}

#[cfg(test)]
mod serialize_documents_tests {
    use sc_3mf::model::{SliceStack, SliceStackContent, write_model};

    use super::*;
    use crate::threemf::test_fixtures::{cube_scene, plate_slices};

    fn xml_of(document: &ModelDocument) -> Vec<u8> {
        let mut xml = Vec::new();
        write_model(document, &mut xml).expect("the document serializes");
        xml
    }

    #[test]
    fn each_document_gets_its_xml() {
        let scene = cube_scene();
        let (scene_model, slice_document) =
            build_documents(&scene, Some(&plate_slices())).expect("the documents build");
        let slice_document = slice_document.expect("slices give a slice document");
        let (scene_xml, slice_xml) = serialize_documents(
            &scene_model.document,
            Some(&slice_document),
            &mut |_: WriteProgress| {},
        )
        .expect("both documents serialize");
        assert_eq!(scene_xml, xml_of(&scene_model.document));
        assert_eq!(slice_xml, Some(xml_of(&slice_document)));
    }

    #[test]
    fn without_a_slice_document_there_is_no_slice_xml() {
        let scene = ModelDocument::default();
        let (scene_xml, slice_xml) = serialize_documents(&scene, None, &mut |_: WriteProgress| {})
            .expect("the document serializes");
        assert_eq!(scene_xml, xml_of(&scene));
        assert_eq!(slice_xml, None);
    }

    #[test]
    fn a_refused_slice_document_returns_its_error() {
        let refused = ModelDocument {
            slice_stacks: vec![SliceStack {
                id: ResourceId::MIN,
                z_bottom: f64::NAN,
                content: SliceStackContent::Slices(vec![]),
            }],
            ..ModelDocument::default()
        };
        let result = serialize_documents(
            &ModelDocument::default(),
            Some(&refused),
            &mut |_: WriteProgress| {},
        );
        assert!(
            matches!(
                result,
                Err(ModelWriteError::SliceZBottomNotFinite { stack_id: 1 })
            ),
            "{result:?}"
        );
    }
}

#[cfg(test)]
mod assemble_package_tests {
    use super::*;
    use crate::threemf::test_fixtures::{full_payload, part};

    fn assemble(slice_xml: Option<Vec<u8>>, embed: EmbedProfiles) -> Package {
        assemble_package(
            b"model".to_vec(),
            &[],
            slice_xml,
            &full_payload(),
            embed,
            None,
        )
        .expect("the package assembles")
    }

    #[test]
    fn the_model_and_slice_parts_hold_the_given_bytes() {
        let package = assemble(Some(b"slices".to_vec()), EmbedProfiles::Yes);
        assert_eq!(
            package.part_data(&part(DEFAULT_MODEL_PART_NAME)),
            Some(&b"model"[..])
        );
        assert_eq!(
            package.part_data(&part(SLICE_PART_NAME)),
            Some(&b"slices"[..])
        );
    }

    #[test]
    fn the_root_relationships_follow_the_parts_in_order() {
        let package = assemble(Some(b"slices".to_vec()), EmbedProfiles::Yes);
        let relationships: Vec<(&str, &str, &str)> = package
            .root_relationships()
            .iter()
            .map(|r| (r.id.as_str(), r.rel_type.as_str(), r.target.as_str()))
            .collect();
        assert_eq!(
            relationships,
            vec![
                ("rel-1", START_PART_REL_TYPE, DEFAULT_MODEL_PART_NAME),
                ("rel-2", PROJECT_REL_TYPE, PROJECT_PART_NAME),
                ("rel-3", MUST_PRESERVE_REL_TYPE, PROJECT_PART_NAME),
                ("rel-4", SLICE_REL_TYPE, SLICE_PART_NAME),
                ("rel-5", MUST_PRESERVE_REL_TYPE, SLICE_PART_NAME),
                (
                    "rel-6",
                    MUST_PRESERVE_REL_TYPE,
                    "/Metadata/profiles/printers/test-printer.toml"
                ),
                (
                    "rel-7",
                    MUST_PRESERVE_REL_TYPE,
                    "/Metadata/profiles/materials/test-resin.toml"
                ),
                (
                    "rel-8",
                    MUST_PRESERVE_REL_TYPE,
                    "/Metadata/profiles/materials/extensions/test-resin.toml"
                ),
            ]
        );
    }

    #[test]
    fn without_slices_or_profiles_only_the_model_and_project_parts_remain() {
        let package = assemble(None, EmbedProfiles::No);
        let names: Vec<&str> = package.part_names().map(PartName::as_str).collect();
        assert_eq!(names, vec![DEFAULT_MODEL_PART_NAME, PROJECT_PART_NAME]);
        assert_eq!(package.root_relationships().len(), 3);
    }
}

#[cfg(test)]
mod compress_into_tests {
    use super::*;
    use crate::threemf::test_fixtures::part;

    fn small_package() -> Package {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME),
                MODEL_PART_CONTENT_TYPE,
                b"<model/>".to_vec(),
            )
            .expect("the part name is unique");
        package
    }

    struct RefusingWriter;

    impl Write for RefusingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("refused"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_writer_receives_the_compressed_package() {
        let package = small_package();
        let mut expected = Cursor::new(Vec::new());
        package
            .write_to(&mut expected)
            .expect("the package compresses");
        let mut written = Vec::new();
        compress_into(&package, &mut written, |_| {}).expect("the package compresses");
        assert_eq!(written, expected.into_inner());
    }

    #[test]
    fn a_refusing_writer_gives_an_io_error() {
        let result = compress_into(&small_package(), &mut RefusingWriter, |_| {});
        assert!(
            matches!(result, Err(WriteThreeMfProjectError::Io(_))),
            "{result:?}"
        );
    }
}
