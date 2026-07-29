/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Unit tests for the model document parser and serializer.

use crate::error::ModelParseError;
use crate::model::{
    BuildItem, Matrix3D, MeshData, MeshResolution, ModelDocument, ObjectPayload, ObjectResource,
    ObjectType, ResourceId, ResourceIndex, TriangleRef, Unit, Vertex3, parse_model, write_model,
};
use crate::names::{PRODUCTION_NS, SLICE_NS};

/// The core-spec appendix metadata example (ASCII-adjusted): vendor
/// metadata with preserve flags, a base-materials group, a mesh
/// object with default properties and a metadata group, a components
/// object, and a transformed build item carrying its own metadata
/// group.
const SPEC_METADATA_EXAMPLE: &str = include_str!("../tests/fixtures/spec/metadata_example.model");

fn id(value: u32) -> ResourceId {
    ResourceId::new(value).expect("valid id")
}

fn parse(xml: &str) -> Result<ModelDocument, ModelParseError> {
    parse_model(xml.as_bytes())
}

fn minimal_model(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">{body}</model>"#,
    )
}

/// A tetrahedron object plus a build item, as reusable body content.
const TETRA_BODY: &str = r#"<resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="10"/></vertices><triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/></triangles></mesh></object></resources><build><item objectid="1"/></build>"#;

#[cfg(test)]
mod spec_example_tests {
    use super::*;

    #[test]
    fn parses_the_spec_metadata_example() {
        let document = parse(SPEC_METADATA_EXAMPLE).expect("spec example parses");
        assert_eq!(document.unit, Unit::Millimeter);

        assert_eq!(document.metadata.len(), 9);
        assert_eq!(document.metadata[0].name, "Copyright");
        assert_eq!(document.metadata[0].value, "(c) Microsoft Corporation 2013");
        let vendor = &document.metadata[8];
        assert_eq!(vendor.name, "vendor1:CustomMetadata1");
        assert!(vendor.preserve);

        assert_eq!(document.base_materials.len(), 1);
        assert_eq!(document.base_materials[0].id, id(1));
        assert_eq!(document.base_materials[0].bases.len(), 1);
        assert_eq!(document.base_materials[0].bases[0].name, "Green");
        assert_eq!(
            document.base_materials[0].bases[0].display_color,
            "#21BB4CFF",
        );

        assert_eq!(document.objects.len(), 2);
        let cube = &document.objects[0];
        assert_eq!(cube.id, id(2));
        assert_eq!(cube.object_type, ObjectType::Model);
        let property = cube.property_ref.expect("pid/pindex captured");
        assert_eq!(property.group, id(1));
        assert_eq!(property.index.get(), 0);
        let ObjectPayload::Mesh(mesh) = &cube.payload else {
            panic!("object 2 carries a mesh");
        };
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.triangles.len(), 12);
        assert_eq!(mesh.vertices[1].x, 39.998);
        assert_eq!(mesh.triangles[0].v3.get(), 2);

        let composed = &document.objects[1];
        assert_eq!(composed.id, id(3));
        let ObjectPayload::Components(components) = &composed.payload else {
            panic!("object 3 carries components");
        };
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].object_id, id(2));

        assert_eq!(document.build.len(), 1);
        let item = &document.build[0];
        assert_eq!(item.object_id, id(3));
        let transform = item.transform.expect("item carries a transform");
        assert_eq!(transform.0[9], -19.999);
        assert_eq!(transform.0[10], -62.998);
    }

    #[test]
    fn spec_example_round_trips_through_the_writer() {
        let document = parse(SPEC_METADATA_EXAMPLE).expect("spec example parses");
        let mut bytes = Vec::new();
        write_model(&document, &mut bytes).expect("document writes");
        let reparsed = parse_model(&bytes).expect("written document parses");
        assert_eq!(reparsed, document);
    }
}

#[cfg(test)]
mod conformance_tests {
    use super::*;

    #[test]
    fn dtd_content_is_rejected() {
        let xml = format!(
            "<?xml version=\"1.0\"?><!DOCTYPE model [<!ENTITY x \"y\">]>{}",
            minimal_model(TETRA_BODY)
                .trim_start_matches("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
        );
        let err = parse(&xml).unwrap_err();
        assert!(matches!(err, ModelParseError::DtdForbidden), "err: {err:?}");
    }

    #[test]
    fn unsupported_required_extension_is_refused_by_namespace() {
        let xml = format!(
            r#"<model unit="millimeter" requiredextensions="p" xmlns:p="{PRODUCTION_NS}" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources/><build/></model>"#,
        );
        let err = parse(&xml).unwrap_err();
        assert!(
            matches!(err, ModelParseError::UnsupportedRequiredExtension { namespace } if namespace == PRODUCTION_NS),
        );
    }

    #[test]
    fn required_slice_extension_is_accepted() {
        // The slice namespace is in the parser's supported set, so
        // requiring it no longer refuses the document.
        let xml = format!(
            r#"<model unit="millimeter" requiredextensions="s" xmlns:s="{SLICE_NS}" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources/><build/></model>"#,
        );
        parse(&xml).expect("slice-required document parses");
    }

    #[test]
    fn undeclared_required_extension_prefix_is_refused() {
        let xml = r#"<model requiredextensions="q" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources/><build/></model>"#;
        let err = parse(xml).unwrap_err();
        assert!(
            matches!(err, ModelParseError::UnknownRequiredExtensionPrefix { prefix } if prefix == "q"),
        );
    }

    #[test]
    fn production_path_attributes_are_refused() {
        let xml = r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"><resources><object id="1" p:path="/3D/other.model"><components/></object></resources><build/></model>"#;
        let err = parse(xml).unwrap_err();
        assert!(
            matches!(err, ModelParseError::ProductionPathUnsupported),
            "err: {err:?}",
        );
    }

    #[test]
    fn unknown_optional_namespace_elements_are_ignored() {
        let body = TETRA_BODY.replace(
            "</triangles></mesh>",
            r#"</triangles><t:trianglesets xmlns:t="http://schemas.microsoft.com/3dmanufacturing/trianglesets/2021/07"><t:triangleset name="all" identifier="a"><t:refrange startindex="0" endindex="3"/></t:triangleset></t:trianglesets></mesh>"#,
        );
        let document = parse(&minimal_model(&body)).expect("trianglesets are ignorable");
        let ObjectPayload::Mesh(mesh) = &document.objects[0].payload else {
            panic!("mesh payload expected");
        };
        assert_eq!(mesh.triangles.len(), 4);
    }

    #[test]
    fn non_finite_vertex_coordinates_are_rejected() {
        // The spec's ST_Number is a decimal pattern; "NaN" and "inf"
        // are not expressible in it even though Rust's f64 parser
        // accepts them.
        for bad in ["NaN", "inf", "-inf", "infinity"] {
            let body = format!(
                r#"<resources><object id="1"><mesh><vertices><vertex x="{bad}" y="0" z="0"/></vertices><triangles/></mesh></object></resources><build/>"#,
            );
            let err = parse(&minimal_model(&body)).unwrap_err();
            assert!(
                matches!(err, ModelParseError::InvalidNumber { ref value, .. } if value == bad),
                "expected InvalidNumber for vertex x={bad}",
            );
        }
    }

    #[test]
    fn non_finite_transform_values_are_rejected() {
        let body = r#"<resources><object id="1"><components/></object></resources><build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 NaN 0 0"/></build>"#;
        let err = parse(&minimal_model(body)).unwrap_err();
        assert!(matches!(err, ModelParseError::InvalidMatrix { .. }));
    }

    #[test]
    fn missing_core_namespace_is_an_unexpected_root() {
        let err = parse(r#"<model unit="millimeter"><resources/><build/></model>"#).unwrap_err();
        assert!(
            matches!(err, ModelParseError::UnexpectedRootElement { found } if found == "model"),
        );
    }

    #[test]
    fn empty_document_reports_missing_model() {
        let err = parse(r#"<?xml version="1.0"?><!-- nothing here -->"#).unwrap_err();
        assert!(matches!(err, ModelParseError::MissingModelElement));
    }
}

#[cfg(test)]
mod reference_and_id_tests {
    use super::*;

    #[test]
    fn duplicate_resource_ids_are_rejected() {
        let body = r#"<resources><basematerials id="7"/><object id="7"><components/></object></resources><build/>"#;
        let err = parse(&minimal_model(body)).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::DuplicateResourceId { id: 7 }
        ));
    }

    #[test]
    fn forward_component_references_are_rejected() {
        let body = r#"<resources><object id="1"><components><component objectid="2"/></components></object><object id="2"><components/></object></resources><build/>"#;
        let err = parse(&minimal_model(body)).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::UnknownObjectReference { id: 2 }
        ));
    }

    #[test]
    fn build_item_referencing_an_undefined_object_is_rejected() {
        let body = r#"<resources/><build><item objectid="9"/></build>"#;
        let err = parse(&minimal_model(body)).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::UnknownObjectReference { id: 9 }
        ));
    }

    #[test]
    fn build_item_referencing_an_other_type_object_is_rejected() {
        let body = r#"<resources><object id="1" type="other"><components/></object></resources><build><item objectid="1"/></build>"#;
        let err = parse(&minimal_model(body)).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::BuildItemReferencesOtherType { id: 1 },
        ));
    }

    #[test]
    fn triangle_indices_are_range_checked() {
        let body = r#"<resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="3"/></triangles></mesh></object></resources><build/>"#;
        let err = parse(&minimal_model(body)).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::TriangleIndexOutOfRange {
                object_id: 1,
                index: 3,
                vertex_count: 3,
            },
        ));
    }

    #[test]
    fn object_without_geometry_is_rejected() {
        let body = r#"<resources><object id="1"/></resources><build/>"#;
        let err = parse(&minimal_model(body)).unwrap_err();
        assert!(matches!(
            err,
            ModelParseError::ObjectWithoutGeometry { id: 1 }
        ));
    }

    #[test]
    fn invalid_unit_is_rejected() {
        let xml = r#"<model unit="furlong" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources/><build/></model>"#;
        let err = parse(xml).unwrap_err();
        assert!(matches!(err, ModelParseError::InvalidUnit { value } if value == "furlong"));
    }

    #[test]
    fn invalid_transform_is_rejected() {
        let body = r#"<resources><object id="1"><components/></object></resources><build><item objectid="1" transform="1 0 0"/></build>"#;
        let err = parse(&minimal_model(body)).unwrap_err();
        assert!(matches!(err, ModelParseError::InvalidMatrix { .. }));
    }
}

#[cfg(test)]
mod text_and_unit_tests {
    use super::*;

    #[test]
    fn metadata_entities_are_resolved() {
        let body = r#"<resources/><build/>"#;
        let xml = minimal_model(body).replace(
            "<resources/>",
            r#"<metadata name="Title">A &amp; B &#65;</metadata><resources/>"#,
        );
        let document = parse(&xml).expect("entities resolve");
        assert_eq!(document.metadata[0].value, "A & B A");
    }

    #[test]
    fn undeclared_entities_are_rejected() {
        let xml = minimal_model(r#"<resources/><build/>"#).replace(
            "<resources/>",
            r#"<metadata name="Title">&unknown;</metadata><resources/>"#,
        );
        let err = parse(&xml).unwrap_err();
        assert!(matches!(err, ModelParseError::UndeclaredEntity { name } if name == "unknown"),);
    }

    #[test]
    fn non_millimeter_units_parse() {
        let xml = minimal_model(TETRA_BODY).replace("unit=\"millimeter\"", "unit=\"inch\"");
        let document = parse(&xml).expect("inch unit parses");
        assert_eq!(document.unit, Unit::Inch);
        assert_eq!(document.unit.scale_to_mm(), 25.4);
    }

    #[test]
    fn missing_unit_defaults_to_millimeter() {
        let xml = minimal_model(TETRA_BODY).replace(" unit=\"millimeter\"", "");
        let document = parse(&xml).expect("unitless model parses");
        assert_eq!(document.unit, Unit::Millimeter);
    }

    #[test]
    fn collapsed_whitespace_in_transforms_is_tolerated() {
        let body = r#"<resources><object id="1"><components/></object></resources><build><item objectid="1" transform="  1 0 0   0 1 0
 0 0 1 5 6 7 "/></build>"#;
        let document = parse(&minimal_model(body)).expect("whitespace tolerated");
        assert_eq!(
            document.build[0].transform,
            Some(Matrix3D([
                1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 5.0, 6.0, 7.0,
            ])),
        );
    }

    #[test]
    fn writer_escapes_special_characters_in_names_and_metadata() {
        let mut document = parse(&minimal_model(TETRA_BODY)).expect("tetra parses");
        document.objects[0].name = Some("a<b & \"c\"".to_string());
        document.metadata.push(crate::model::Metadata {
            name: "Description".to_string(),
            value: "5 < 7 & 9 > 3".to_string(),
            preserve: false,
            value_type: None,
        });
        let mut bytes = Vec::new();
        write_model(&document, &mut bytes).expect("document writes");
        let reparsed = parse_model(&bytes).expect("escaped document parses");
        assert_eq!(reparsed, document);
    }
}

#[cfg(test)]
mod numeric_round_trip_tests {
    use super::*;

    fn ridx(value: u32) -> ResourceIndex {
        ResourceIndex::new(value).expect("valid index")
    }

    /// The mesh writer emits vertex and triangle elements straight to
    /// the sink with ryu/itoa. Coordinates that stress the
    /// shortest-decimal formatter (round integers, negative zero, a long
    /// fractional, f32-origin values) must survive write -> parse with
    /// their exact f64 bits: the slice-input fingerprint hashes those
    /// bits, so a formatter that did not round-trip exactly would
    /// silently stale a project's slices on reopen.
    #[test]
    fn mesh_coordinates_round_trip_bit_exact() {
        let vertices = vec![
            Vertex3 {
                x: 0.0,
                y: -0.0,
                z: 1.0,
            },
            Vertex3 {
                x: -1.0,
                y: 100.0,
                z: 39.998,
            },
            Vertex3 {
                x: 123.456_789_012_345,
                y: 0.1_f32 as f64,
                z: -2.5_f32 as f64,
            },
        ];
        let triangles = vec![TriangleRef {
            v1: ridx(0),
            v2: ridx(1),
            v3: ridx(2),
        }];
        let document = ModelDocument {
            objects: vec![ObjectResource {
                id: id(1),
                object_type: ObjectType::Model,
                name: None,
                partnumber: None,
                property_ref: None,
                slice_stack: None,
                mesh_resolution: MeshResolution::Fullres,
                payload: ObjectPayload::Mesh(MeshData {
                    vertices: vertices.clone(),
                    triangles: triangles.clone(),
                }),
            }],
            build: vec![BuildItem {
                object_id: id(1),
                transform: None,
                partnumber: None,
            }],
            ..ModelDocument::default()
        };

        let mut bytes = Vec::new();
        write_model(&document, &mut bytes).expect("mesh writes");
        let reparsed = parse_model(&bytes).expect("mesh parses");
        let ObjectPayload::Mesh(mesh) = &reparsed.objects[0].payload else {
            panic!("expected a mesh payload");
        };
        assert_eq!(mesh.vertices.len(), vertices.len());
        for (got, want) in mesh.vertices.iter().zip(&vertices) {
            assert_eq!(got.x.to_bits(), want.x.to_bits(), "x bits differ");
            assert_eq!(got.y.to_bits(), want.y.to_bits(), "y bits differ");
            assert_eq!(got.z.to_bits(), want.z.to_bits(), "z bits differ");
        }
        for (got, want) in mesh.triangles.iter().zip(&triangles) {
            assert_eq!(got.v1.get(), want.v1.get());
            assert_eq!(got.v2.get(), want.v2.get());
            assert_eq!(got.v3.get(), want.v3.get());
        }
    }

    /// A mesh large enough to cross the writer's parallel threshold, so
    /// the concurrent chunk path runs. Every vertex bit must still
    /// survive write -> parse, including the negative zeros and messy
    /// fractionals sprinkled through it.
    #[test]
    fn large_mesh_round_trips_through_the_parallel_path() {
        let n = 10_000u32;
        let vertices: Vec<Vertex3> = (0..n)
            .map(|i| Vertex3 {
                x: (i as f64) * 0.001 - 5.0,
                y: -(i as f64) * 0.01,
                z: if i % 7 == 0 { -0.0 } else { (i as f64).sqrt() },
            })
            .collect();
        let triangles: Vec<TriangleRef> = (0..n)
            .map(|i| TriangleRef {
                v1: ridx(i % n),
                v2: ridx((i + 1) % n),
                v3: ridx((i + 2) % n),
            })
            .collect();
        let document = ModelDocument {
            objects: vec![ObjectResource {
                id: id(1),
                object_type: ObjectType::Model,
                name: None,
                partnumber: None,
                property_ref: None,
                slice_stack: None,
                mesh_resolution: MeshResolution::Fullres,
                payload: ObjectPayload::Mesh(MeshData {
                    vertices: vertices.clone(),
                    triangles: triangles.clone(),
                }),
            }],
            build: vec![BuildItem {
                object_id: id(1),
                transform: None,
                partnumber: None,
            }],
            ..ModelDocument::default()
        };

        let mut bytes = Vec::new();
        write_model(&document, &mut bytes).expect("mesh writes");
        let reparsed = parse_model(&bytes).expect("mesh parses");
        let ObjectPayload::Mesh(mesh) = &reparsed.objects[0].payload else {
            panic!("expected a mesh payload");
        };
        assert_eq!(mesh.vertices.len(), vertices.len());
        for (got, want) in mesh.vertices.iter().zip(&vertices) {
            assert_eq!(got.x.to_bits(), want.x.to_bits());
            assert_eq!(got.y.to_bits(), want.y.to_bits());
            assert_eq!(got.z.to_bits(), want.z.to_bits());
        }
        assert_eq!(mesh.triangles.len(), triangles.len());
    }
}
