/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tests for the serialization phase of a 3MF write.

use super::serialization::{ModelSerialization, SerializationProgress};
use super::slice::{
    Slice, SlicePolygon, SliceRef, SliceSegment, SliceStack, SliceStackContent, SliceVertex,
};
use super::types::{
    Component, MeshData, ModelDocument, ObjectPayload, ObjectResource, ResourceId, ResourceIndex,
    TriangleRef, Vertex3,
};
use super::write::write_model;
use crate::error::ModelWriteError;

fn id(value: u32) -> ResourceId {
    ResourceId::new(value).expect("test id is valid")
}

fn index(value: u32) -> ResourceIndex {
    ResourceIndex::new(value).expect("test index is valid")
}

fn object(id: ResourceId, payload: ObjectPayload) -> ObjectResource {
    ObjectResource {
        id,
        object_type: Default::default(),
        name: None,
        partnumber: None,
        property_ref: None,
        slice_stack: None,
        mesh_resolution: Default::default(),
        payload,
    }
}

/// Returns a document with 1 mesh object of `count` vertices and `count`
/// triangles.
fn mesh_document(count: u32) -> ModelDocument {
    let vertices = (0..count)
        .map(|i| {
            let i = f64::from(i);
            Vertex3 {
                x: i * 0.5,
                y: i * 0.25,
                z: i,
            }
        })
        .collect();
    let triangles = (0..count)
        .map(|i| TriangleRef {
            v1: index(i),
            v2: index((i + 1) % count),
            v3: index((i + 2) % count),
        })
        .collect();
    let mesh = MeshData {
        vertices,
        triangles,
    };
    ModelDocument {
        objects: vec![object(id(1), ObjectPayload::Mesh(mesh))],
        ..ModelDocument::default()
    }
}

/// Returns a document with 1 slice stack: a layer with 3 vertices and 1 polygon
/// of 3 segments, then an empty layer.
fn sliced_document() -> ModelDocument {
    let layer = Slice {
        z_top: 0.1,
        vertices: vec![
            SliceVertex { x: 0.0, y: 0.0 },
            SliceVertex { x: 1.0, y: 0.0 },
            SliceVertex { x: 0.0, y: 1.0 },
        ],
        polygons: vec![SlicePolygon {
            start: index(0),
            segments: [1, 2, 0]
                .map(|end| SliceSegment { end: index(end) })
                .to_vec(),
        }],
    };
    let empty = Slice {
        z_top: 0.2,
        vertices: vec![],
        polygons: vec![],
    };
    ModelDocument {
        slice_stacks: vec![SliceStack {
            id: id(1),
            z_bottom: 0.0,
            content: SliceStackContent::Slices(vec![layer, empty]),
        }],
        ..ModelDocument::default()
    }
}

/// Returns a document that `write_model` refuses: its slice stack starts at a
/// NaN height.
fn refused_document() -> ModelDocument {
    ModelDocument {
        slice_stacks: vec![SliceStack {
            id: id(1),
            z_bottom: f64::NAN,
            content: SliceStackContent::Slices(vec![]),
        }],
        ..ModelDocument::default()
    }
}

fn xml_of(document: &ModelDocument) -> Vec<u8> {
    let mut xml = Vec::new();
    write_model(document, &mut xml).expect("the document serializes");
    xml
}

fn write_collecting<const N: usize>(
    documents: [&ModelDocument; N],
) -> (
    Result<[Vec<u8>; N], ModelWriteError>,
    Vec<SerializationProgress>,
) {
    let mut reports = Vec::new();
    let result = ModelSerialization::new(documents).write(|report| reports.push(report));
    (result, reports)
}

mod estimate_tests {
    use super::*;

    #[test]
    fn an_empty_document_is_estimated_at_0() {
        let empty = ModelDocument::default();
        assert_eq!(ModelSerialization::new([&empty]).estimated_total_bytes(), 0);
    }

    #[test]
    fn a_mesh_object_counts_its_vertices_and_triangles() {
        let mesh = mesh_document(4);
        assert_eq!(
            ModelSerialization::new([&mesh]).estimated_total_bytes(),
            448
        );
    }

    #[test]
    fn a_components_object_adds_nothing() {
        let mut document = mesh_document(4);
        let component = Component {
            object_id: id(1),
            transform: None,
        };
        document
            .objects
            .push(object(id(2), ObjectPayload::Components(vec![component])));
        assert_eq!(
            ModelSerialization::new([&document]).estimated_total_bytes(),
            448
        );
    }

    #[test]
    fn a_slice_stack_counts_its_vertices_and_segments() {
        let sliced = sliced_document();
        assert_eq!(
            ModelSerialization::new([&sliced]).estimated_total_bytes(),
            192
        );
    }

    #[test]
    fn a_stack_of_slice_references_adds_nothing() {
        let mut document = sliced_document();
        document.slice_stacks.push(SliceStack {
            id: id(2),
            z_bottom: 0.0,
            content: SliceStackContent::Refs(vec![SliceRef {
                slice_stack: id(1),
                slice_path: "/2D/slices.model".to_string(),
            }]),
        });
        assert_eq!(
            ModelSerialization::new([&document]).estimated_total_bytes(),
            192
        );
    }

    #[test]
    fn the_estimate_of_2_documents_is_their_sum() {
        let (mesh, sliced) = (mesh_document(4), sliced_document());
        assert_eq!(
            ModelSerialization::new([&mesh, &sliced]).estimated_total_bytes(),
            640
        );
    }
}

mod write_tests {
    use super::*;

    #[test]
    fn each_buffer_is_the_xml_of_its_document() {
        let (mesh, sliced) = (mesh_document(4), sliced_document());
        let (result, _) = write_collecting([&mesh, &sliced]);
        assert_eq!(
            result.expect("both documents serialize"),
            [xml_of(&mesh), xml_of(&sliced)]
        );
    }

    #[test]
    fn a_document_above_the_parallel_threshold_keeps_its_xml() {
        // `write_model` formats 8192 elements or more in parallel chunks.
        let mesh = mesh_document(10_000);
        let (result, _) = write_collecting([&mesh]);
        assert_eq!(result.expect("the document serializes"), [xml_of(&mesh)]);
    }

    #[test]
    fn a_small_document_gives_1_report_at_its_length() {
        let mesh = mesh_document(4);
        let (result, reports) = write_collecting([&mesh]);
        let [xml] = result.expect("the document serializes");
        assert_eq!(
            reports,
            vec![SerializationProgress {
                serialized_bytes: xml.len() as u64,
                estimated_total_bytes: 448,
            }]
        );
    }

    #[test]
    fn the_count_carries_across_the_documents() {
        let (mesh, sliced) = (mesh_document(4), sliced_document());
        let (result, reports) = write_collecting([&mesh, &sliced]);
        let [first, second] = result.expect("both documents serialize");
        let report = |serialized_bytes: usize| SerializationProgress {
            serialized_bytes: serialized_bytes as u64,
            estimated_total_bytes: 640,
        };
        assert_eq!(
            reports,
            vec![report(first.len()), report(first.len() + second.len())]
        );
    }

    #[test]
    fn a_large_document_reports_at_steps_of_256_kib_or_more() {
        let mesh = mesh_document(40_000);
        let (result, reports) = write_collecting([&mesh]);
        let [xml] = result.expect("the document serializes");
        assert!(xml.len() > 1024 * 1024, "the XML must exceed 1 MiB");
        assert!(
            reports
                .iter()
                .all(|report| report.estimated_total_bytes == 4_480_000),
            "{reports:?}"
        );
        let counts: Vec<u64> = reports.iter().map(|r| r.serialized_bytes).collect();
        let (last, steps) = counts.split_last().expect("the document reports");
        assert_eq!(*last, xml.len() as u64);
        assert!(!steps.is_empty(), "no report before the end: {counts:?}");
        let mut previous = 0;
        for &count in steps {
            assert!(count - previous >= 256 * 1024, "{counts:?}");
            previous = count;
        }
    }
}

mod failure_tests {
    use super::*;

    #[test]
    fn a_refused_2nd_document_ends_the_reports_with_the_1st() {
        let (mesh, refused) = (mesh_document(4), refused_document());
        let (result, reports) = write_collecting([&mesh, &refused]);
        assert!(
            matches!(
                result,
                Err(ModelWriteError::SliceZBottomNotFinite { stack_id: 1 })
            ),
            "{result:?}"
        );
        assert_eq!(
            reports,
            vec![SerializationProgress {
                serialized_bytes: xml_of(&mesh).len() as u64,
                estimated_total_bytes: 448,
            }]
        );
    }

    #[test]
    fn a_refused_1st_document_gives_no_report() {
        let (refused, mesh) = (refused_document(), mesh_document(4));
        let (result, reports) = write_collecting([&refused, &mesh]);
        assert!(
            matches!(
                result,
                Err(ModelWriteError::SliceZBottomNotFinite { stack_id: 1 })
            ),
            "{result:?}"
        );
        assert!(reports.is_empty(), "{reports:?}");
    }
}
