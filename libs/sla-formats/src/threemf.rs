/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! 3MF Slice Extension packages.
//!
//! [`write_scene_slices`] turns a sliced scene into a single 3MF package,
//! conventionally named `*.slices.3mf`: the root model part carries 1 object
//! per sliced leaf (its mesh baked to world space, so build items need no
//! transform) with a slice stack attached through the Slice Extension, and each
//! stack's layer data lives in its own `/2D/` model part reached via a sliceref
//! plus a part-level relationship. Meshes are embedded at full resolution, so
//! consumers without slice support still load the plain geometry.
//!
//! Layer `ztop` values are the slab TOPS recomputed from the
//! [`LayerSchedule`](sc_sl1cer::profile::LayerSchedule) in [`WriterOptions`]:
//! the contour layers' own `z_mm` are the centred sampling planes, which are
//! not what the format records. Layers without polygons become void slices (a
//! bare `ztop`). Contour rings are implicitly closed, while 3MF polygons demand
//! an explicit final segment back to the start vertex; the writer adds it,
//! preserving each ring's winding (counter-clockwise outer boundaries,
//! clockwise holes).

use std::fs;
use std::io::Cursor;
use std::path::Path;

use sc_3mf::model::resource_ids::{ResourceId, ResourceIndex};
use sc_3mf::model::{
    BuildItem, MeshResolution, Metadata, ModelDocument, ObjectPayload, ObjectResource, ObjectType,
    Slice, SlicePolygon, SliceRef, SliceSegment, SliceStack, SliceStackContent, SliceVertex,
    write_model,
};
use sc_3mf::names::{DEFAULT_MODEL_PART_NAME, MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_formats::threemf::mesh_to_model_data;
use sc_scene::Scene;
use sc_scene::bake::bake_leaf_to_world;
use sc_sl1cer::LeafSlices;
use sc_sl1cer::contour::{ClosedRing, ContourLayer};

use crate::error::WriterError;
use crate::format::Format;
use crate::options::WriterOptions;
use crate::write::validate_output_path;

/// Id of the single slice stack inside each `/2D/` part. Every part is its own
/// id space, so the constant never collides.
const SLICE_PART_STACK_ID: usize = 1;

/// Write `slices` as a 3MF Slice Extension package at `output`.
///
/// `slices` is the per-leaf output of [`slice_scene`](sc_sl1cer::slice_scene)
/// for `scene`; the merged stack is not accepted because the format keeps each
/// model's layers separate. Every leaf named by `slices` is baked to world
/// space through `scene`, embedded as a full-resolution mesh object, and paired
/// with its own slice stack; slice coordinates from the engine are world-space
/// millimetres already, so mesh and contours agree without any item transform.
///
/// # Errors
///
/// - [`WriterError::NoModelsToWrite`] when `slices` is empty.
/// - [`WriterError::LayerGridMismatch`] when a stack's layer count differs from
///   the schedule in `options`.
/// - [`WriterError::SceneBakeFailed`] when a leaf cannot be baked (dangling id,
///   non-mesh node, invalid transform chain).
/// - [`WriterError::MalformedPartName`] when a slice part name that the writer
///   built breaks the part-name rules.
/// - Path validation and I/O errors shared with
///   [`write_slice`](crate::write_slice).
pub fn write_scene_slices(
    scene: &Scene,
    slices: &[LeafSlices],
    output: &Path,
    options: &WriterOptions,
) -> Result<(), WriterError> {
    if slices.is_empty() {
        return Err(WriterError::NoModelsToWrite);
    }
    let slab_tops: Vec<f64> = options.layer_schedule().iter_slab_tops().collect();
    for leaf_slices in slices {
        let stack_layers = leaf_slices.stack().layers().len();
        if stack_layers != slab_tops.len() {
            return Err(WriterError::LayerGridMismatch {
                leaf: leaf_slices.leaf(),
                schedule_layers: slab_tops.len(),
                stack_layers,
            });
        }
    }
    validate_output_path(output, Format::ThreeMf)?;

    let mut package = Package::new();
    let root_part = PartName::from(DEFAULT_MODEL_PART_NAME);
    let mut root_document = ModelDocument::default();
    copy_scene_metadata(scene, &mut root_document);

    for (index, leaf_slices) in slices.iter().enumerate() {
        let ordinal = index + 1;
        let slice_part_name = format!("/2D/slices-{ordinal}.model");

        let stack_id = resource_id(2 * ordinal - 1);
        let object_id = resource_id(2 * ordinal);
        root_document.slice_stacks.push(SliceStack {
            id: stack_id,
            z_bottom: 0.0,
            content: SliceStackContent::Refs(vec![SliceRef {
                slice_stack: resource_id(SLICE_PART_STACK_ID),
                slice_path: slice_part_name.clone(),
            }]),
        });

        let baked =
            bake_leaf_to_world(scene, leaf_slices.leaf()).map_err(WriterError::SceneBakeFailed)?;
        root_document.objects.push(ObjectResource {
            id: object_id,
            object_type: ObjectType::Model,
            name: leaf_slices.leaf_name().map(str::to_string),
            partnumber: None,
            property_ref: None,
            slice_stack: Some(stack_id),
            mesh_resolution: MeshResolution::default(),
            payload: ObjectPayload::Mesh(mesh_to_model_data(&baked)),
        });
        root_document.build.push(BuildItem {
            object_id,
            transform: None,
            partnumber: None,
        });

        let slice_document = slice_part_document(leaf_slices.stack().layers(), &slab_tops);
        let mut slice_xml = Vec::new();
        write_model(&slice_document, &mut slice_xml)?;
        let part = checked_part_name(&slice_part_name)?;
        package.add_part(part, MODEL_PART_CONTENT_TYPE, slice_xml)?;
        package.add_part_relationship(
            &root_part,
            Relationship {
                id: format!("rel-{ordinal}"),
                rel_type: START_PART_REL_TYPE.to_string(),
                target: slice_part_name,
            },
        )?;
    }

    let mut root_xml = Vec::new();
    write_model(&root_document, &mut root_xml)?;
    package.add_part(root_part, MODEL_PART_CONTENT_TYPE, root_xml)?;
    package.add_root_relationship(Relationship {
        id: "rel-1".to_string(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: DEFAULT_MODEL_PART_NAME.as_str().to_string(),
    });

    // The zip layer needs a seekable sink, so serialize into memory first and
    // persist in 1 write.
    let mut buffer = Cursor::new(Vec::new());
    package.write_to(&mut buffer)?;
    fs::write(output, buffer.get_ref())?;
    Ok(())
}

/// Returns `name`, a part name that the writer built, as a [`PartName`], or
/// [`WriterError::MalformedPartName`] when it breaks the part-name rules.
fn checked_part_name(name: &str) -> Result<PartName, WriterError> {
    PartName::new(name).map_err(|source| WriterError::MalformedPartName {
        name: name.to_string(),
        source,
    })
}

/// A `/2D/` slice part: a core model document holding exactly 1 inline-slices
/// stack and an empty build, the single level of sliceref indirection the Slice
/// Extension allows.
fn slice_part_document(layers: &[ContourLayer], slab_tops: &[f64]) -> ModelDocument {
    let slices = layers
        .iter()
        .zip(slab_tops.iter())
        .map(|(layer, &z_top)| layer_to_slice(layer, z_top))
        .collect();
    let mut document = ModelDocument::default();
    document.slice_stacks.push(SliceStack {
        id: resource_id(SLICE_PART_STACK_ID),
        z_bottom: 0.0,
        content: SliceStackContent::Slices(slices),
    });
    document
}

/// A contour layer as a 3MF slice at the slab top `z_top`. A layer without
/// polygons becomes a void slice.
fn layer_to_slice(layer: &ContourLayer, z_top: f64) -> Slice {
    let mut vertices = Vec::new();
    let mut polygons = Vec::new();
    for polygon in layer.polygons() {
        push_ring(polygon.outer(), &mut vertices, &mut polygons);
        for hole in polygon.holes() {
            push_ring(hole, &mut vertices, &mut polygons);
        }
    }
    Slice {
        z_top,
        vertices,
        polygons,
    }
}

/// Appends an implicitly-closed contour ring as an explicitly closed 3MF
/// polygon: the ring's points join the slice vertex list and the segment chain
/// ends with the closing edge back to the start vertex. Winding is preserved
/// as-is.
fn push_ring(ring: &ClosedRing, vertices: &mut Vec<SliceVertex>, polygons: &mut Vec<SlicePolygon>) {
    let base = vertices.len();
    vertices.extend(ring.vertices().iter().map(|point| SliceVertex {
        x: point.x,
        y: point.y,
    }));
    let segments = (1..ring.len())
        .chain([0])
        .map(|offset| SliceSegment {
            end: resource_index(base + offset),
        })
        .collect();
    polygons.push(SlicePolygon {
        start: resource_index(base),
        segments,
    });
}

fn copy_scene_metadata(scene: &Scene, document: &mut ModelDocument) {
    let Some(meta) = scene.meta() else {
        return;
    };
    if let Some(name) = &meta.name {
        document.metadata.push(Metadata {
            name: "Title".to_string(),
            value: name.clone(),
            preserve: false,
            value_type: None,
        });
    }
    if let Some(comment) = &meta.comment {
        document.metadata.push(Metadata {
            name: "Description".to_string(),
            value: comment.clone(),
            preserve: false,
            value_type: None,
        });
    }
}

fn resource_id(raw: usize) -> ResourceId {
    let raw = u32::try_from(raw).expect("scene leaf counts stay far below 2^31");
    ResourceId::new(raw).expect("ids derived from leaf ordinals start at 1")
}

fn resource_index(raw: usize) -> ResourceIndex {
    let raw = u32::try_from(raw).expect("slice vertex counts stay far below 2^31");
    ResourceIndex::new(raw).expect("indices below 2^31 are always valid")
}

#[cfg(test)]
mod checked_part_name_tests {
    use super::*;
    use sc_3mf::error::MalformedPartNameError;

    #[test]
    fn a_malformed_name_gives_the_name_and_the_broken_rule() {
        let result = checked_part_name("/2D//slices-1.model");
        assert!(
            matches!(
                &result,
                Err(WriterError::MalformedPartName { name, source })
                    if name == "/2D//slices-1.model"
                        && *source == MalformedPartNameError::EmptySegment
            ),
            "{result:?}"
        );
    }

    #[test]
    fn a_valid_name_gives_the_part_name() {
        let name = checked_part_name("/2D/slices-1.model").expect("the name is valid");
        assert_eq!(name.as_str(), "/2D/slices-1.model");
    }
}
