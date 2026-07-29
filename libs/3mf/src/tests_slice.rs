/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Unit tests for the slice-extension layer: document round trips,
//! parser validation, and the write-time invariants (monotone z,
//! polygon closure, planar transforms on sliced objects).

use crate::error::{ModelParseError, ModelWriteError};
use crate::model::{
    Matrix3D, MeshData, MeshResolution, ModelDocument, ObjectPayload, ObjectResource, ObjectType,
    ResourceId, ResourceIndex, Slice, SlicePolygon, SliceRef, SliceSegment, SliceStack,
    SliceStackContent, SliceVertex, TriangleRef, Vertex3, parse_model, write_model,
};

fn id(value: u32) -> ResourceId {
    ResourceId::new(value).expect("valid id")
}

fn index(value: u32) -> ResourceIndex {
    ResourceIndex::new(value).expect("valid index")
}

fn tetra_mesh() -> MeshData {
    MeshData {
        vertices: vec![
            Vertex3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            Vertex3 {
                x: 10.0,
                y: 0.0,
                z: 0.0,
            },
            Vertex3 {
                x: 0.0,
                y: 10.0,
                z: 0.0,
            },
            Vertex3 {
                x: 0.0,
                y: 0.0,
                z: 10.0,
            },
        ],
        triangles: vec![
            TriangleRef {
                v1: index(0),
                v2: index(2),
                v3: index(1),
            },
            TriangleRef {
                v1: index(0),
                v2: index(1),
                v3: index(3),
            },
            TriangleRef {
                v1: index(0),
                v2: index(3),
                v3: index(2),
            },
            TriangleRef {
                v1: index(1),
                v2: index(2),
                v3: index(3),
            },
        ],
    }
}

/// A closed unit square contour at the given height.
fn square_slice(z_top: f64) -> Slice {
    Slice {
        z_top,
        vertices: vec![
            SliceVertex { x: 0.0, y: 0.0 },
            SliceVertex { x: 10.0, y: 0.0 },
            SliceVertex { x: 10.0, y: 10.0 },
            SliceVertex { x: 0.0, y: 10.0 },
        ],
        polygons: vec![SlicePolygon {
            start: index(0),
            segments: vec![
                SliceSegment { end: index(1) },
                SliceSegment { end: index(2) },
                SliceSegment { end: index(3) },
                SliceSegment { end: index(0) },
            ],
        }],
    }
}

/// Same square, but the chain stops before returning to the start.
fn open_slice(z_top: f64) -> Slice {
    let mut slice = square_slice(z_top);
    slice.polygons[0].segments.pop();
    slice
}

fn empty_slice(z_top: f64) -> Slice {
    Slice {
        z_top,
        vertices: Vec::new(),
        polygons: Vec::new(),
    }
}

fn inline_stack(stack_id: u32, z_bottom: f64, slices: Vec<Slice>) -> SliceStack {
    SliceStack {
        id: id(stack_id),
        z_bottom,
        content: SliceStackContent::Slices(slices),
    }
}

fn sliced_object(object_id: u32, stack_id: u32) -> ObjectResource {
    ObjectResource {
        id: id(object_id),
        object_type: ObjectType::Model,
        name: None,
        partnumber: None,
        property_ref: None,
        slice_stack: Some(id(stack_id)),
        mesh_resolution: MeshResolution::default(),
        payload: ObjectPayload::Mesh(tetra_mesh()),
    }
}

fn document(stacks: Vec<SliceStack>, objects: Vec<ObjectResource>) -> ModelDocument {
    let build = objects
        .iter()
        .map(|o| crate::model::BuildItem {
            object_id: o.id,
            transform: None,
            partnumber: None,
        })
        .collect();
    ModelDocument {
        slice_stacks: stacks,
        objects,
        build,
        ..ModelDocument::default()
    }
}

fn write_string(document: &ModelDocument) -> String {
    let mut bytes = Vec::new();
    write_model(document, &mut bytes).expect("write succeeds");
    String::from_utf8(bytes).expect("valid UTF-8")
}

fn write_err(document: &ModelDocument) -> ModelWriteError {
    let mut bytes = Vec::new();
    write_model(document, &mut bytes).expect_err("write fails")
}

fn roundtrip(document: &ModelDocument) -> ModelDocument {
    parse_model(write_string(document).as_bytes()).expect("written document parses")
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;

    #[test]
    fn inline_slices_round_trip_including_the_void_layer() {
        let source = document(
            vec![inline_stack(
                1,
                0.25,
                vec![square_slice(0.5), square_slice(0.75), empty_slice(1.0)],
            )],
            vec![sliced_object(2, 1)],
        );
        let reparsed = roundtrip(&source);
        assert_eq!(reparsed, source);
    }

    #[test]
    fn slicerefs_round_trip() {
        let source = document(
            vec![SliceStack {
                id: id(1),
                z_bottom: 0.0,
                content: SliceStackContent::Refs(vec![
                    SliceRef {
                        slice_stack: id(1),
                        slice_path: "/2D/slices1.model".to_string(),
                    },
                    SliceRef {
                        slice_stack: id(3),
                        slice_path: "/2D/slices2.model".to_string(),
                    },
                ]),
            }],
            vec![sliced_object(2, 1)],
        );
        let reparsed = roundtrip(&source);
        assert_eq!(reparsed, source);
    }

    #[test]
    fn lowres_marker_round_trips_with_required_extensions() {
        let mut source = document(
            vec![inline_stack(1, 0.0, vec![square_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        source.objects[0].mesh_resolution = MeshResolution::Lowres;
        let xml = write_string(&source);
        assert!(
            xml.contains(r#"requiredextensions="s""#),
            "lowres must make the extension mandatory: {xml}",
        );
        let reparsed = parse_model(xml.as_bytes()).expect("slice-required document parses");
        assert_eq!(reparsed, source);
    }

    #[test]
    fn zbottom_defaults_to_zero_when_absent() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:s="http://schemas.microsoft.com/3dmanufacturing/slice/2015/07"><resources><s:slicestack id="1"><s:slice ztop="0.5"/></s:slicestack></resources><build/></model>"#;
        let parsed = parse_model(xml.as_bytes()).expect("parses");
        assert_eq!(parsed.slice_stacks[0].z_bottom, 0.0);
    }
}

#[cfg(test)]
mod attribute_emission_tests {
    use super::*;

    #[test]
    fn fullres_slice_content_is_only_recommended() {
        let source = document(
            vec![inline_stack(1, 0.0, vec![square_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        let xml = write_string(&source);
        assert!(xml.contains(r#"recommendedextensions="s""#), "{xml}");
        assert!(!xml.contains("requiredextensions"), "{xml}");
        assert!(
            xml.contains(r#"xmlns:s="http://schemas.microsoft.com/3dmanufacturing/slice/2015/07""#),
            "{xml}",
        );
        assert!(xml.contains(r#"s:slicestackid="1""#), "{xml}");
    }

    #[test]
    fn documents_without_slice_content_stay_core_only() {
        let mut source = document(Vec::new(), vec![sliced_object(1, 9)]);
        source.objects[0].slice_stack = None;
        let xml = write_string(&source);
        assert!(!xml.contains("xmlns:s"), "{xml}");
        assert!(!xml.contains("recommendedextensions"), "{xml}");
    }
}

#[cfg(test)]
mod parse_validation_tests {
    use super::*;

    fn slice_model(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:s="http://schemas.microsoft.com/3dmanufacturing/slice/2015/07">{body}</model>"#,
        )
    }

    #[test]
    fn mixing_slices_and_refs_is_rejected() {
        let body = r#"<resources><s:slicestack id="1"><s:slice ztop="0.5"/><s:sliceref slicestackid="1" slicepath="/2D/s.model"/></s:slicestack></resources><build/>"#;
        let err = parse_model(slice_model(body).as_bytes()).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::SliceStackMixesSlicesAndRefs { id: 1 },
        ));
    }

    #[test]
    fn slicestack_ids_share_the_resource_id_space() {
        let body = r#"<resources><s:slicestack id="1"><s:slice ztop="0.5"/></s:slicestack><object id="1"><mesh/></object></resources><build/>"#;
        let err = parse_model(slice_model(body).as_bytes()).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::DuplicateResourceId { id: 1 }
        ));
    }

    #[test]
    fn object_referencing_an_undefined_stack_is_rejected() {
        let body =
            r#"<resources><object id="1" s:slicestackid="9"><mesh/></object></resources><build/>"#;
        let err = parse_model(slice_model(body).as_bytes()).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::UnknownSliceStackReference {
                object_id: 1,
                stack_id: 9,
            },
        ));
    }

    #[test]
    fn unknown_mesh_resolution_is_rejected() {
        let body = r#"<resources><object id="1" s:meshresolution="midres"><mesh/></object></resources><build/>"#;
        let err = parse_model(slice_model(body).as_bytes()).unwrap_err();
        assert!(
            matches!(err, ModelParseError::InvalidMeshResolution { value } if value == "midres"),
        );
    }

    #[test]
    fn polygon_without_segments_is_rejected() {
        let body = r#"<resources><s:slicestack id="1"><s:slice ztop="0.5"><s:vertices><s:vertex x="0" y="0"/></s:vertices><s:polygon startv="0"/></s:slice></s:slicestack></resources><build/>"#;
        let err = parse_model(slice_model(body).as_bytes()).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::PolygonWithoutSegments { stack_id: 1 },
        ));
    }

    #[test]
    fn segment_indices_are_range_checked() {
        let body = r#"<resources><s:slicestack id="1"><s:slice ztop="0.5"><s:vertices><s:vertex x="0" y="0"/><s:vertex x="1" y="0"/></s:vertices><s:polygon startv="0"><s:segment v2="7"/></s:polygon></s:slice></s:slicestack></resources><build/>"#;
        let err = parse_model(slice_model(body).as_bytes()).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::SliceIndexOutOfRange {
                stack_id: 1,
                index: 7,
                vertex_count: 2,
            },
        ));
    }

    #[test]
    fn segment_properties_are_accepted_and_dropped() {
        let body = r#"<resources><s:slicestack id="1"><s:slice ztop="0.5"><s:vertices><s:vertex x="0" y="0"/><s:vertex x="1" y="0"/></s:vertices><s:polygon startv="0"><s:segment v2="1" p1="0" p2="0" pid="4"/><s:segment v2="0"/></s:polygon></s:slice></s:slicestack></resources><build/>"#;
        let parsed = parse_model(slice_model(body).as_bytes()).expect("parses");
        let SliceStackContent::Slices(slices) = &parsed.slice_stacks[0].content else {
            panic!("expected inline slices");
        };
        assert_eq!(
            slices[0].polygons[0].segments,
            vec![
                SliceSegment { end: index(1) },
                SliceSegment { end: index(0) }
            ],
        );
    }
}

#[cfg(test)]
mod write_validation_tests {
    use super::*;

    #[test]
    fn first_slice_must_rise_above_zbottom() {
        let source = document(
            vec![inline_stack(1, 0.5, vec![square_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::SliceZNotIncreasing {
                stack_id: 1,
                index: 0,
            },
        ));
    }

    #[test]
    fn slice_boundaries_must_strictly_increase() {
        let source = document(
            vec![inline_stack(
                1,
                0.0,
                vec![square_slice(0.5), square_slice(0.5)],
            )],
            vec![sliced_object(2, 1)],
        );
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::SliceZNotIncreasing {
                stack_id: 1,
                index: 1,
            },
        ));
    }

    #[test]
    fn open_polygons_are_refused_for_model_objects() {
        let source = document(
            vec![inline_stack(1, 0.0, vec![open_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::OpenPolygonOnModelObject {
                object_id: 2,
                stack_id: 1,
            },
        ));
    }

    #[test]
    fn open_polygons_are_legal_for_support_objects() {
        let mut source = document(
            vec![inline_stack(1, 0.0, vec![open_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        source.objects[0].object_type = ObjectType::Support;
        let reparsed = roundtrip(&source);
        assert_eq!(reparsed, source);
    }

    #[test]
    fn polygon_without_segments_is_refused() {
        let mut slice = square_slice(0.5);
        slice.polygons[0].segments.clear();
        let source = document(vec![inline_stack(1, 0.0, vec![slice])], Vec::new());
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::PolygonWithoutSegments { stack_id: 1 },
        ));
    }

    #[test]
    fn slice_indices_are_range_checked() {
        let mut slice = square_slice(0.5);
        slice.polygons[0].segments[1] = SliceSegment { end: index(11) };
        let source = document(vec![inline_stack(1, 0.0, vec![slice])], Vec::new());
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::SliceIndexOutOfRange {
                stack_id: 1,
                index: 11,
                vertex_count: 4,
            },
        ));
    }

    #[test]
    fn dangling_stack_reference_is_refused() {
        let source = document(Vec::new(), vec![sliced_object(2, 9)]);
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::UnknownSliceStackReference {
                object_id: 2,
                stack_id: 9,
            },
        ));
    }
}

#[cfg(test)]
mod planar_transform_tests {
    use super::*;

    /// Planar: rotation about Z plus an XY translation and a Z lift.
    fn planar() -> Matrix3D {
        Matrix3D([
            0.0, 1.0, 0.0, //
            -1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, //
            5.0, 6.0, 7.0,
        ])
    }

    /// Rotation about X: tips the object over, moving z into y.
    fn non_planar() -> Matrix3D {
        Matrix3D([
            1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, //
            0.0, -1.0, 0.0, //
            0.0, 0.0, 0.0,
        ])
    }

    #[test]
    fn planar_item_transform_on_a_sliced_object_writes() {
        let mut source = document(
            vec![inline_stack(1, 0.0, vec![square_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        source.build[0].transform = Some(planar());
        let reparsed = roundtrip(&source);
        assert_eq!(reparsed, source);
    }

    #[test]
    fn non_planar_item_transform_on_a_sliced_object_is_refused() {
        let mut source = document(
            vec![inline_stack(1, 0.0, vec![square_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        source.build[0].transform = Some(non_planar());
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::NonPlanarSlicedTransform { object_id: 2 },
        ));
    }

    #[test]
    fn non_planar_component_transform_targeting_a_sliced_object_is_refused() {
        let container = ObjectResource {
            id: id(3),
            object_type: ObjectType::Model,
            name: None,
            partnumber: None,
            property_ref: None,
            slice_stack: None,
            mesh_resolution: MeshResolution::default(),
            payload: ObjectPayload::Components(vec![crate::model::Component {
                object_id: id(2),
                transform: Some(non_planar()),
            }]),
        };
        let mut source = document(
            vec![inline_stack(1, 0.0, vec![square_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        source.objects.push(container);
        source.build.push(crate::model::BuildItem {
            object_id: id(3),
            transform: None,
            partnumber: None,
        });
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::NonPlanarSlicedTransform { object_id: 2 },
        ));
    }

    #[test]
    fn non_planar_item_transform_on_a_container_of_sliced_content_is_refused() {
        let container = ObjectResource {
            id: id(3),
            object_type: ObjectType::Model,
            name: None,
            partnumber: None,
            property_ref: None,
            slice_stack: None,
            mesh_resolution: MeshResolution::default(),
            payload: ObjectPayload::Components(vec![crate::model::Component {
                object_id: id(2),
                transform: None,
            }]),
        };
        let mut source = document(
            vec![inline_stack(1, 0.0, vec![square_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        source.objects.push(container);
        source.build.push(crate::model::BuildItem {
            object_id: id(3),
            transform: Some(non_planar()),
            partnumber: None,
        });
        let err = write_err(&source);
        assert!(matches!(
            err,
            ModelWriteError::NonPlanarSlicedTransform { object_id: 3 },
        ));
    }

    #[test]
    fn non_sliced_objects_keep_unrestricted_transforms() {
        let mut source = document(Vec::new(), vec![sliced_object(1, 9)]);
        source.objects[0].slice_stack = None;
        source.build[0].transform = Some(non_planar());
        let reparsed = roundtrip(&source);
        assert_eq!(reparsed, source);
    }

    #[test]
    fn negative_zero_planar_cells_are_normalized_on_emission() {
        let mut matrix = planar();
        matrix.0[2] = -0.0;
        matrix.0[6] = -0.0;
        let mut source = document(
            vec![inline_stack(1, 0.0, vec![square_slice(0.5)])],
            vec![sliced_object(2, 1)],
        );
        source.build[0].transform = Some(matrix);
        let xml = write_string(&source);
        assert!(
            xml.contains(r#"transform="0 1 0 -1 0 0 0 0 1 5 6 7""#),
            "planar cells must serialize as plain 0/1: {xml}",
        );
    }
}

#[cfg(test)]
mod numeric_round_trip_tests {
    use super::*;

    /// Slice vertices, segment indices and ztop are emitted straight to
    /// the sink with ryu/itoa. Values that stress the shortest-decimal
    /// formatter (round ztop, negative zero, a long fractional) must
    /// survive write -> parse bit-exact, for the same fingerprint reason
    /// as the mesh coordinates.
    #[test]
    fn slice_coordinates_round_trip_bit_exact() {
        let vertices = vec![
            SliceVertex { x: 0.0, y: -0.0 },
            SliceVertex { x: 1.0, y: 40.0 },
            SliceVertex {
                x: -12.345_678_9,
                y: 123.456_789_012_345,
            },
        ];
        let source = ModelDocument {
            slice_stacks: vec![SliceStack {
                id: id(1),
                z_bottom: 0.0,
                content: SliceStackContent::Slices(vec![Slice {
                    z_top: 1.0,
                    vertices: vertices.clone(),
                    polygons: vec![SlicePolygon {
                        start: index(0),
                        segments: vec![
                            SliceSegment { end: index(1) },
                            SliceSegment { end: index(2) },
                            SliceSegment { end: index(0) },
                        ],
                    }],
                }]),
            }],
            ..ModelDocument::default()
        };

        let mut bytes = Vec::new();
        write_model(&source, &mut bytes).expect("slice writes");
        let reparsed = parse_model(&bytes).expect("slice parses");
        let SliceStackContent::Slices(slices) = &reparsed.slice_stacks[0].content else {
            panic!("expected inline slices");
        };
        let got = &slices[0];
        assert_eq!(got.z_top.to_bits(), 1.0_f64.to_bits(), "ztop bits differ");
        assert_eq!(got.vertices.len(), vertices.len());
        for (g, w) in got.vertices.iter().zip(&vertices) {
            assert_eq!(g.x.to_bits(), w.x.to_bits(), "x bits differ");
            assert_eq!(g.y.to_bits(), w.y.to_bits(), "y bits differ");
        }
        assert_eq!(got.polygons[0].start.get(), 0);
        let ends: Vec<u32> = got.polygons[0]
            .segments
            .iter()
            .map(|s| s.end.get())
            .collect();
        assert_eq!(ends, vec![1, 2, 0]);
    }

    /// A stack with enough layers to cross the writer's parallel
    /// threshold, so slice fragments are emitted concurrently. Every
    /// layer must round-trip bit-exact, and the strictly increasing
    /// ztop chain must be preserved in order.
    #[test]
    fn many_slices_round_trip_through_the_parallel_path() {
        let count = 100u32;
        let slices: Vec<Slice> = (0..count)
            .map(|i| Slice {
                z_top: (i as f64 + 1.0) * 0.05,
                vertices: vec![
                    SliceVertex { x: 0.0, y: -0.0 },
                    SliceVertex {
                        x: i as f64 * 0.1,
                        y: 10.0,
                    },
                    SliceVertex {
                        x: -(i as f64),
                        y: (i as f64).sqrt(),
                    },
                ],
                polygons: vec![SlicePolygon {
                    start: index(0),
                    segments: vec![
                        SliceSegment { end: index(1) },
                        SliceSegment { end: index(2) },
                        SliceSegment { end: index(0) },
                    ],
                }],
            })
            .collect();
        let source = ModelDocument {
            slice_stacks: vec![SliceStack {
                id: id(1),
                z_bottom: 0.0,
                content: SliceStackContent::Slices(slices.clone()),
            }],
            ..ModelDocument::default()
        };

        let mut bytes = Vec::new();
        write_model(&source, &mut bytes).expect("slices write");
        let reparsed = parse_model(&bytes).expect("slices parse");
        let SliceStackContent::Slices(got) = &reparsed.slice_stacks[0].content else {
            panic!("expected inline slices");
        };
        assert_eq!(got.len(), slices.len());
        for (g, w) in got.iter().zip(&slices) {
            assert_eq!(g.z_top.to_bits(), w.z_top.to_bits());
            for (gv, wv) in g.vertices.iter().zip(&w.vertices) {
                assert_eq!(gv.x.to_bits(), wv.x.to_bits());
                assert_eq!(gv.y.to_bits(), wv.y.to_bits());
            }
        }
    }
}
