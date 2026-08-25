/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tests for reading runs of mesh leaf elements from raw bytes.
//!
//! Two properties matter throughout: the elements a run accepts must
//! match what the event parser produces for the same bytes, and a run
//! must stop exactly before the first thing it cannot read, so the
//! parser still sees that element and still reports it.

use proptest::prelude::*;

use super::element_run::{slice_segment_run, slice_vertex_run, triangle_run, vertex_run};
use super::slice::{Slice, SlicePolygon, SliceSegment, SliceStack, SliceStackContent, SliceVertex};
use super::types::{
    BuildItem, MeshData, ModelDocument, ObjectPayload, ObjectResource, ResourceId, ResourceIndex,
    TriangleRef, Vertex3,
};
use super::write::write_model;
use crate::model::parse_model;

/// Bit patterns of a vertex, so a signed zero cannot pass as a zero.
fn vertex_bits(vertex: &Vertex3) -> [u64; 3] {
    [vertex.x.to_bits(), vertex.y.to_bits(), vertex.z.to_bits()]
}

fn indices(triangle: &TriangleRef) -> [u32; 3] {
    [triangle.v1.get(), triangle.v2.get(), triangle.v3.get()]
}

fn index(value: u32) -> ResourceIndex {
    ResourceIndex::new(value).expect("index in range")
}

/// The bytes following `open`, as a run's input starts: right after
/// the opening tag of the element list.
fn content_after<'a>(xml: &'a str, open: &str) -> &'a [u8] {
    let start = xml.find(open).expect("opening tag present") + open.len();
    &xml.as_bytes()[start..]
}

#[cfg(test)]
mod vertex_run_tests {
    use super::*;

    #[test]
    fn reads_a_plain_run() {
        let input = br#"<vertex x="0" y="1.5" z="-2"/><vertex x="3" y="4" z="5"/></vertices>"#;
        let run = vertex_run(input, b"vertex");
        assert_eq!(run.items.len(), 2);
        assert_eq!(
            vertex_bits(&run.items[0]),
            vertex_bits(&Vertex3 {
                x: 0.0,
                y: 1.5,
                z: -2.0
            })
        );
        assert_eq!(run.consumed, input.len() - b"</vertices>".len());
    }

    #[test]
    fn reads_the_indented_spelling_producers_emit() {
        let input = b"\n  <vertex x=\"0\" y=\"0\" z=\"0\" />\n  <vertex x=\"1\" y=\"0\" z=\"0\" />\n</vertices>";
        let run = vertex_run(input, b"vertex");
        assert_eq!(run.items.len(), 2);
        // The run ends at the last element, leaving the whitespace and
        // the end tag to the parser.
        assert_eq!(&input[run.consumed..], b"\n</vertices>");
    }

    #[test]
    fn accepts_every_attribute_order() {
        let orders = [
            r#"<vertex x="1" y="2" z="3"/>"#,
            r#"<vertex x="1" z="3" y="2"/>"#,
            r#"<vertex y="2" x="1" z="3"/>"#,
            r#"<vertex y="2" z="3" x="1"/>"#,
            r#"<vertex z="3" x="1" y="2"/>"#,
            r#"<vertex z="3" y="2" x="1"/>"#,
        ];
        for order in orders {
            let run = vertex_run(order.as_bytes(), b"vertex");
            assert_eq!(run.items.len(), 1, "{order}");
            assert_eq!(
                vertex_bits(&run.items[0]),
                vertex_bits(&Vertex3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0
                }),
                "{order}",
            );
        }
    }

    #[test]
    fn accepts_the_whitespace_characters_xml_allows() {
        let input = b"<vertex\tx=\"1\"\ny=\"2\"\r\nz=\"3\"\t/>";
        let run = vertex_run(input, b"vertex");
        assert_eq!(run.items.len(), 1);
        assert_eq!(run.consumed, input.len());
    }

    #[test]
    fn reads_a_prefixed_element_name() {
        // A conformant producer may bind the core namespace to a
        // prefix; the caller resolves the name and passes its bytes.
        let input = br#"<c:vertex x="1" y="2" z="3"/></c:vertices>"#;
        let run = vertex_run(input, b"c:vertex");
        assert_eq!(run.items.len(), 1);
        let plain = vertex_run(input, b"vertex");
        assert_eq!(plain.items.len(), 0);
        assert_eq!(plain.consumed, 0);
    }

    #[test]
    fn accepts_the_number_spellings_the_parser_accepts() {
        let input = br#"<vertex x="+1" y=".5" z="1e2"/><vertex x=" 7 " y="-0" z="0007"/>"#;
        let run = vertex_run(input, b"vertex");
        assert_eq!(run.items.len(), 2);
        assert_eq!(run.items[0].x, 1.0);
        assert_eq!(run.items[0].y, 0.5);
        assert_eq!(run.items[0].z, 100.0);
        assert_eq!(run.items[1].x, 7.0);
        assert_eq!(run.items[1].y.to_bits(), (-0.0_f64).to_bits());
    }

    #[test]
    fn reads_nothing_from_an_empty_or_blank_input() {
        for input in [b"".as_slice(), b"   \n\t ".as_slice(), b"</vertices>"] {
            let run = vertex_run(input, b"vertex");
            assert!(run.items.is_empty());
            assert_eq!(run.consumed, 0);
        }
    }
}

/// One stop condition per case: the run keeps the elements before the
/// offending one and ends right after the last of them.
#[cfg(test)]
mod stop_condition_tests {
    use super::*;

    const GOOD: &str = r#"<vertex x="1" y="2" z="3"/>"#;

    fn assert_stops_before(offending: &str) {
        let input = format!("{GOOD}{offending}");
        let run = vertex_run(input.as_bytes(), b"vertex");
        assert_eq!(run.items.len(), 1, "accepted {offending}");
        assert_eq!(run.consumed, GOOD.len(), "wrong resume point: {offending}");
    }

    #[test]
    fn stops_at_a_different_element_name() {
        assert_stops_before(r#"<vertexx x="1" y="2" z="3"/>"#);
        assert_stops_before(r#"<triangle v1="0" v2="1" v3="2"/>"#);
    }

    #[test]
    fn stops_at_an_attribute_outside_the_schema() {
        assert_stops_before(r#"<vertex x="1" y="2" z="3" w="4"/>"#);
    }

    #[test]
    fn stops_at_a_prefixed_attribute() {
        // Extension attributes carry a prefix, and the parser may act
        // on them: the production extension's path is refused there.
        assert_stops_before(r#"<vertex x="1" y="2" z="3" p:path="/3D/other.model"/>"#);
    }

    #[test]
    fn stops_at_a_namespace_declaration() {
        // A default-namespace rebinding would put the element outside
        // the core namespace for the parser.
        assert_stops_before(r#"<vertex xmlns="urn:other" x="1" y="2" z="3"/>"#);
        assert_stops_before(r#"<vertex xmlns:p="urn:other" x="1" y="2" z="3"/>"#);
    }

    #[test]
    fn stops_at_a_duplicate_attribute() {
        // The event parser reports duplicates as an error.
        assert_stops_before(r#"<vertex x="1" x="1" y="2" z="3"/>"#);
    }

    #[test]
    fn stops_at_a_missing_attribute() {
        assert_stops_before(r#"<vertex x="1" y="2"/>"#);
        assert_stops_before(r#"<vertex/>"#);
    }

    #[test]
    fn stops_at_a_value_needing_unescaping() {
        assert_stops_before(r#"<vertex x="&#48;" y="2" z="3"/>"#);
        assert_stops_before(r#"<vertex x="1" y="2" z="3&"/>"#);
    }

    #[test]
    fn stops_at_a_value_outside_ascii() {
        assert_stops_before("<vertex x=\"\u{00B9}\" y=\"2\" z=\"3\"/>");
    }

    #[test]
    fn stops_at_a_quoting_or_spacing_variant() {
        assert_stops_before(r#"<vertex x='1' y='2' z='3'/>"#);
        assert_stops_before(r#"<vertex x = "1" y="2" z="3"/>"#);
        assert_stops_before(r#"<vertex x="1"y="2" z="3"/>"#);
    }

    #[test]
    fn stops_at_an_element_with_a_closing_tag() {
        assert_stops_before(r#"<vertex x="1" y="2" z="3"></vertex>"#);
    }

    #[test]
    fn stops_at_a_comment() {
        assert_stops_before("<!-- a comment --><vertex x=\"1\" y=\"2\" z=\"3\"/>");
    }

    #[test]
    fn stops_at_a_number_the_parser_would_reject() {
        assert_stops_before(r#"<vertex x="none" y="2" z="3"/>"#);
        assert_stops_before(r#"<vertex x="NaN" y="2" z="3"/>"#);
        assert_stops_before(r#"<vertex x="inf" y="2" z="3"/>"#);
        assert_stops_before(r#"<vertex x="1 2" y="2" z="3"/>"#);
    }

    #[test]
    fn stops_at_a_truncated_element() {
        assert_stops_before(r#"<vertex x="1" y="2" z="3""#);
        assert_stops_before(r#"<vertex x="1" y="2" z="#);
        assert_stops_before("<");
    }
}

#[cfg(test)]
mod triangle_run_tests {
    use super::*;

    #[test]
    fn reads_a_plain_run() {
        let input = br#"<triangle v1="0" v2="1" v3="2"/><triangle v1="2" v2="1" v3="0"/>"#;
        let run = triangle_run(input, b"triangle", 3);
        assert_eq!(run.items.len(), 2);
        assert_eq!(indices(&run.items[0]), [0, 1, 2]);
        assert_eq!(indices(&run.items[1]), [2, 1, 0]);
        assert_eq!(run.consumed, input.len());
    }

    #[test]
    fn accepts_the_property_attributes_the_schema_defines() {
        // The materials extension assigns properties through these
        // core attributes; this crate carries no model for them.
        let input = br#"<triangle v1="0" v2="1" v3="2" pid="1" p1="0"/><triangle v1="0" v2="1" v3="2" pid="8" p1="0" p2="1" p3="2"/>"#;
        let run = triangle_run(input, b"triangle", 3);
        assert_eq!(run.items.len(), 2);
        assert_eq!(indices(&run.items[1]), [0, 1, 2]);
        assert_eq!(run.consumed, input.len());
    }

    #[test]
    fn stops_at_an_index_outside_the_mesh() {
        let good = r#"<triangle v1="0" v2="1" v3="2"/>"#;
        let input = format!("{good}<triangle v1=\"0\" v2=\"1\" v3=\"3\"/>");
        let run = triangle_run(input.as_bytes(), b"triangle", 3);
        assert_eq!(run.items.len(), 1);
        assert_eq!(run.consumed, good.len());
    }

    #[test]
    fn stops_at_an_index_outside_the_spec_range() {
        let input = br#"<triangle v1="0" v2="1" v3="2147483648"/>"#;
        let run = triangle_run(input, b"triangle", 2147483649);
        assert!(run.items.is_empty());
        assert_eq!(run.consumed, 0);
    }

    #[test]
    fn reads_nothing_when_the_mesh_has_no_vertices() {
        let input = br#"<triangle v1="0" v2="1" v3="2"/>"#;
        let run = triangle_run(input, b"triangle", 0);
        assert!(run.items.is_empty());
        assert_eq!(run.consumed, 0);
    }
}

/// A run long enough to be split across threads must give the same
/// answer as a short one, including where it stops.
#[cfg(test)]
mod chunking_tests {
    use super::*;

    const COUNT: usize = 5_000;

    fn long_vertex_input(bad_at: Option<usize>) -> (String, usize) {
        let mut text = String::new();
        let mut good_len = 0;
        for i in 0..COUNT {
            if Some(i) == bad_at {
                text.push_str(r#"<vertex x="oops" y="0" z="0"/>"#);
                return (text, good_len);
            }
            text.push_str(&format!("<vertex x=\"{i}\" y=\"0.5\" z=\"-1\"/>"));
            good_len = text.len();
        }
        (text, good_len)
    }

    #[test]
    fn a_long_run_is_split_and_stitched_in_order() {
        let (text, good_len) = long_vertex_input(None);
        assert!(text.len() > 64 * 1024, "input exercises the split");
        let run = vertex_run(text.as_bytes(), b"vertex");
        assert_eq!(run.items.len(), COUNT);
        assert_eq!(run.consumed, good_len);
        for (i, vertex) in run.items.iter().enumerate() {
            assert_eq!(vertex.x, i as f64);
        }
    }

    #[test]
    fn a_long_run_stops_at_the_first_unreadable_element() {
        for bad_at in [1, COUNT / 3, COUNT / 2, COUNT - 1] {
            let (text, good_len) = long_vertex_input(Some(bad_at));
            let run = vertex_run(text.as_bytes(), b"vertex");
            assert_eq!(run.items.len(), bad_at, "bad element at {bad_at}");
            assert_eq!(run.consumed, good_len, "bad element at {bad_at}");
        }
    }

    #[test]
    fn a_long_run_ends_at_the_element_list_close() {
        let (text, good_len) = long_vertex_input(None);
        let closed = format!("{text}\n  </vertices>\n</mesh>");
        let run = vertex_run(closed.as_bytes(), b"vertex");
        assert_eq!(run.items.len(), COUNT);
        assert_eq!(run.consumed, good_len);
    }
}

/// The elements a run reads must equal the ones the event parser
/// produces from the same document.
#[cfg(test)]
mod parser_equality_tests {
    use super::*;

    fn document_with(mesh: MeshData) -> ModelDocument {
        let id = ResourceId::MIN;
        ModelDocument {
            objects: vec![ObjectResource {
                id,
                object_type: Default::default(),
                name: None,
                partnumber: None,
                property_ref: None,
                slice_stack: None,
                mesh_resolution: Default::default(),
                payload: ObjectPayload::Mesh(mesh),
            }],
            build: vec![BuildItem {
                object_id: id,
                transform: None,
                partnumber: None,
            }],
            ..ModelDocument::default()
        }
    }

    fn written(document: &ModelDocument) -> String {
        let mut bytes = Vec::new();
        write_model(document, &mut bytes).expect("write");
        String::from_utf8(bytes).expect("utf8")
    }

    fn parsed_mesh(xml: &str) -> MeshData {
        let document = parse_model(xml.as_bytes()).expect("parse");
        match &document.objects[0].payload {
            ObjectPayload::Mesh(mesh) => mesh.clone(),
            ObjectPayload::Components(_) => panic!("object carries a mesh"),
        }
    }

    fn assert_matches_the_parser(mesh: MeshData) {
        let xml = written(&document_with(mesh));
        let expected = parsed_mesh(&xml);

        let vertices = vertex_run(content_after(&xml, "<vertices>"), b"vertex");
        assert_eq!(vertices.items.len(), expected.vertices.len());
        for (got, want) in vertices.items.iter().zip(&expected.vertices) {
            assert_eq!(vertex_bits(got), vertex_bits(want));
        }

        let triangles = triangle_run(
            content_after(&xml, "<triangles>"),
            b"triangle",
            expected.vertices.len(),
        );
        assert_eq!(triangles.items.len(), expected.triangles.len());
        for (got, want) in triangles.items.iter().zip(&expected.triangles) {
            assert_eq!(indices(got), indices(want));
        }
    }

    #[test]
    fn matches_the_parser_on_a_tetrahedron() {
        assert_matches_the_parser(MeshData {
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
        });
    }

    #[test]
    fn matches_the_parser_on_awkward_coordinates() {
        assert_matches_the_parser(MeshData {
            vertices: vec![
                Vertex3 {
                    x: -0.0,
                    y: f64::MIN_POSITIVE,
                    z: 1e300,
                },
                Vertex3 {
                    x: 1e-300,
                    y: -1.7976931348623157e308,
                    z: 0.1 + 0.2,
                },
                Vertex3 {
                    x: 123456789.12345679,
                    y: -0.000030517578125,
                    z: 4.9e-324,
                },
            ],
            triangles: vec![TriangleRef {
                v1: index(0),
                v2: index(1),
                v3: index(2),
            }],
        });
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn matches_the_parser_on_generated_meshes(
            coordinates in prop::collection::vec(
                prop::num::f64::ANY.prop_filter("finite", |value| value.is_finite()),
                9..90,
            ),
        ) {
            let vertices: Vec<Vertex3> = coordinates
                .as_chunks::<3>()
                .0
                .iter()
                .map(|chunk| Vertex3 {
                    x: chunk[0],
                    y: chunk[1],
                    z: chunk[2],
                })
                .collect();
            let count = vertices.len() as u32;
            let triangles: Vec<TriangleRef> = (0..count)
                .map(|i| TriangleRef {
                    v1: index(i),
                    v2: index((i + 1) % count),
                    v3: index((i + 2) % count),
                })
                .collect();
            assert_matches_the_parser(MeshData {
                vertices,
                triangles,
            });
        }
    }
}

/// The parser must reach the same document whether or not the runs
/// fire, and must carry on correctly from where a run left it.
#[cfg(test)]
mod parser_route_tests {
    use super::*;
    use crate::error::ModelParseError;

    /// The same document with every attribute in single quotes: legal
    /// XML that no run reads, so the parse goes element by element.
    fn without_runs(xml: &str) -> String {
        xml.replace('"', "'")
    }

    fn model(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">{body}</model>"#,
        )
    }

    const TWO_OBJECTS: &str = r#"<metadata name="Title">two</metadata><resources><object id="1" name="first"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/></vertices><triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/></triangles></mesh></object><object id="2" name="second"><mesh><vertices>
        <vertex x="5" y="0" z="0" />
        <vertex x="6" y="0" z="0" />
        <vertex x="5" y="1" z="0" />
      </vertices><triangles>
        <triangle v1="0" v2="1" v3="2" pid="7" p1="0" />
      </triangles></mesh></object></resources><build><item objectid="1"/><item objectid="2" partnumber="p-2"/></build>"#;

    #[test]
    fn both_spellings_of_a_document_parse_alike() {
        let xml = model(TWO_OBJECTS);
        let accelerated = parse_model(xml.as_bytes()).expect("parse");
        let plain = parse_model(without_runs(&xml).as_bytes()).expect("parse");
        assert_eq!(accelerated, plain);
    }

    #[test]
    fn the_parse_continues_correctly_after_a_run() {
        // Everything after the runs is where a mis-advanced cursor
        // would show up first.
        let document = parse_model(model(TWO_OBJECTS).as_bytes()).expect("parse");
        assert_eq!(document.metadata.len(), 1);
        assert_eq!(document.objects.len(), 2);
        assert_eq!(document.objects[1].name.as_deref(), Some("second"));
        let ObjectPayload::Mesh(second) = &document.objects[1].payload else {
            panic!("object 2 carries a mesh");
        };
        assert_eq!(second.vertices.len(), 3);
        assert_eq!(second.triangles.len(), 1);
        assert_eq!(document.build.len(), 2);
        assert_eq!(document.build[1].partnumber.as_deref(), Some("p-2"));
    }

    #[test]
    fn a_prefixed_core_namespace_parses_alike() {
        let xml = model(TWO_OBJECTS);
        let plain = parse_model(xml.as_bytes()).expect("parse");
        let prefixed = xml
            .replace("xmlns=", "xmlns:c=")
            .replace('<', "<c:")
            .replace("<c:?xml", "<?xml")
            .replace("<c:/", "</c:");
        let document = parse_model(prefixed.as_bytes()).expect("parse prefixed");
        assert_eq!(document, plain);
    }

    #[test]
    fn a_document_the_runs_cannot_read_reports_the_same_error() {
        let body = TWO_OBJECTS.replace(
            r#"<vertex x="1" y="0" z="0"/>"#,
            r#"<vertex x="oops" y="0" z="0"/>"#,
        );
        let xml = model(&body);
        let accelerated = parse_model(xml.as_bytes()).expect_err("invalid number");
        let plain = parse_model(without_runs(&xml).as_bytes()).expect_err("invalid number");
        let ModelParseError::InvalidNumber {
            element,
            attribute,
            value,
        } = &accelerated
        else {
            panic!("expected an invalid number, got {accelerated:?}");
        };
        assert_eq!(
            (*element, *attribute, value.as_str()),
            ("vertex", "x", "oops")
        );
        assert_eq!(format!("{accelerated:?}"), format!("{plain:?}"));
    }

    #[test]
    fn an_index_beyond_the_vertices_reports_the_same_error() {
        let body = TWO_OBJECTS.replace(
            r#"<triangle v1="1" v2="2" v3="3"/>"#,
            r#"<triangle v1="1" v2="2" v3="4"/>"#,
        );
        let xml = model(&body);
        let accelerated = parse_model(xml.as_bytes()).expect_err("index out of range");
        let plain = parse_model(without_runs(&xml).as_bytes()).expect_err("index out of range");
        assert!(matches!(
            accelerated,
            ModelParseError::TriangleIndexOutOfRange {
                object_id: 1,
                index: 4,
                vertex_count: 4,
            },
        ));
        assert_eq!(format!("{accelerated:?}"), format!("{plain:?}"));
    }
}

/// The slice extension's own leaf runs: 2D vertices and the segment
/// chains of a polygon.
#[cfg(test)]
mod slice_run_tests {
    use super::*;
    use crate::model::ResourceId;

    #[test]
    fn reads_a_run_of_two_dimensional_vertices() {
        let input =
            br#"<s:vertex x="0.000" y="0.000" /><s:vertex x="10.103" y="0.000" /></s:vertices>"#;
        let run = slice_vertex_run(input, b"s:vertex");
        assert_eq!(run.items.len(), 2);
        assert_eq!(run.items[1].x, 10.103);
        assert_eq!(run.consumed, input.len() - b"</s:vertices>".len());
    }

    #[test]
    fn stops_at_a_mesh_vertex_spelling() {
        // A 3D vertex carries z, which the 2D element does not define.
        let good = r#"<s:vertex x="1" y="2"/>"#;
        let input = format!("{good}<s:vertex x=\"1\" y=\"2\" z=\"3\"/>");
        let run = slice_vertex_run(input.as_bytes(), b"s:vertex");
        assert_eq!(run.items.len(), 1);
        assert_eq!(run.consumed, good.len());
    }

    #[test]
    fn reads_a_segment_chain() {
        let input = br#"<s:segment v2="1"/><s:segment v2="2"/><s:segment v2="0"/></s:polygon>"#;
        let run = slice_segment_run(input, b"s:segment", 3);
        assert_eq!(run.items.len(), 3);
        assert_eq!(run.items[2].end.get(), 0);
        assert_eq!(run.consumed, input.len() - b"</s:polygon>".len());
    }

    #[test]
    fn accepts_the_segment_properties_the_extension_defines() {
        let input = br#"<s:segment v2="1" p1="0" p2="0" pid="4"/>"#;
        let run = slice_segment_run(input, b"s:segment", 2);
        assert_eq!(run.items.len(), 1);
        assert_eq!(run.consumed, input.len());
    }

    #[test]
    fn stops_at_a_segment_beyond_the_slice_vertices() {
        let good = r#"<s:segment v2="1"/>"#;
        let input = format!("{good}<s:segment v2=\"2\"/>");
        let run = slice_segment_run(input.as_bytes(), b"s:segment", 2);
        assert_eq!(run.items.len(), 1);
        assert_eq!(run.consumed, good.len());
    }

    fn square_slice(z_top: f64) -> Slice {
        let corners = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        Slice {
            z_top,
            vertices: corners
                .iter()
                .map(|(x, y)| SliceVertex { x: *x, y: *y })
                .collect(),
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

    #[test]
    fn a_sliced_document_parses_alike_with_and_without_the_runs() {
        let stack_id = ResourceId::MIN;
        let object_id = ResourceId::new(2).expect("id");
        let document = ModelDocument {
            slice_stacks: vec![SliceStack {
                id: stack_id,
                z_bottom: 0.0,
                content: SliceStackContent::Slices(
                    (1..=8)
                        .map(|layer| square_slice(layer as f64 * 0.05))
                        .collect(),
                ),
            }],
            objects: vec![ObjectResource {
                id: object_id,
                object_type: Default::default(),
                name: None,
                partnumber: None,
                property_ref: None,
                slice_stack: Some(stack_id),
                mesh_resolution: Default::default(),
                payload: ObjectPayload::Mesh(MeshData::default()),
            }],
            build: vec![BuildItem {
                object_id,
                transform: None,
                partnumber: None,
            }],
            ..ModelDocument::default()
        };
        let mut bytes = Vec::new();
        write_model(&document, &mut bytes).expect("write");
        let xml = String::from_utf8(bytes).expect("utf8");

        let accelerated = parse_model(xml.as_bytes()).expect("parse");
        let plain = parse_model(xml.replace('"', "'").as_bytes()).expect("parse");
        assert_eq!(accelerated, plain);
        assert_eq!(accelerated.slice_stacks, document.slice_stacks);
    }
}
