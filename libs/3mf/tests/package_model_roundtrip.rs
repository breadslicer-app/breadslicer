/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Full-container integration test: build a document
//! programmatically, serialize it, wrap it in an OPC package, write
//! the package to bytes, read everything back, and compare.

use std::io::Cursor;

use sc_3mf::model::{
    BuildItem, Component, Matrix3D, MeshData, Metadata, ModelDocument, ObjectPayload,
    ObjectResource, ObjectType, ResourceId, ResourceIndex, TriangleRef, Unit, Vertex3, parse_model,
    write_model,
};
use sc_3mf::names::{DEFAULT_MODEL_PART_NAME, MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};

fn id(value: u32) -> ResourceId {
    ResourceId::new(value).expect("valid id")
}

fn index(value: u32) -> ResourceIndex {
    ResourceIndex::new(value).expect("valid index")
}

fn tetrahedron_mesh() -> MeshData {
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

fn sample_document() -> ModelDocument {
    ModelDocument {
        unit: Unit::Millimeter,
        metadata: vec![Metadata {
            name: "Title".to_string(),
            value: "Round-trip fixture".to_string(),
            preserve: false,
            value_type: None,
        }],
        base_materials: Vec::new(),
        objects: vec![
            ObjectResource {
                id: id(1),
                object_type: ObjectType::Model,
                name: Some("tetra".to_string()),
                partnumber: None,
                property_ref: None,
                payload: ObjectPayload::Mesh(tetrahedron_mesh()),
            },
            ObjectResource {
                id: id(2),
                object_type: ObjectType::Model,
                name: Some("pair-of-tetras".to_string()),
                partnumber: None,
                property_ref: None,
                payload: ObjectPayload::Components(vec![
                    Component {
                        object_id: id(1),
                        transform: None,
                    },
                    Component {
                        object_id: id(1),
                        transform: Some(Matrix3D([
                            0.0, 1.0, 0.0, //
                            -1.0, 0.0, 0.0, //
                            0.0, 0.0, 1.0, //
                            15.0, 0.0, 0.0,
                        ])),
                    },
                ]),
            },
        ],
        build: vec![BuildItem {
            object_id: id(2),
            transform: Some(Matrix3D([
                1.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, //
                0.0, 0.0, 1.0, //
                -5.0, 2.5, 0.0,
            ])),
            partnumber: Some("rt-001".to_string()),
        }],
    }
}

#[test]
fn document_survives_the_full_package_round_trip() {
    let document = sample_document();

    let mut model_xml = Vec::new();
    write_model(&document, &mut model_xml).expect("document writes");

    let mut package = Package::new();
    let model_part = PartName::new(DEFAULT_MODEL_PART_NAME).expect("valid part name");
    package
        .add_part(model_part.clone(), MODEL_PART_CONTENT_TYPE, model_xml)
        .expect("unique part name");
    package.add_root_relationship(Relationship {
        id: "rel-1".to_string(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: DEFAULT_MODEL_PART_NAME.to_string(),
    });

    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).expect("package writes");

    let restored = Package::read_from(Cursor::new(bytes.into_inner())).expect("package reads");
    let start = restored.start_part().expect("start part resolves");
    assert_eq!(start, model_part);
    assert_eq!(
        restored.part_content_type(&start),
        Some(MODEL_PART_CONTENT_TYPE),
    );

    let data = restored.part_data(&start).expect("model part data");
    let reparsed = parse_model(data).expect("model parses");
    assert_eq!(reparsed, document);
}

#[test]
fn rotation_heavy_transforms_survive_the_round_trip_bitwise() {
    // A transform with values on every row, including negatives and
    // values needing many decimal digits, to pin the matrix
    // serialization format.
    let matrix = Matrix3D([
        std::f64::consts::FRAC_1_SQRT_2,
        -std::f64::consts::FRAC_1_SQRT_2,
        0.0,
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
        0.0,
        0.0,
        0.0,
        1.0,
        -19.999,
        0.125,
        3.0e-7,
    ]);
    let mut document = sample_document();
    document.build[0].transform = Some(matrix);

    let mut xml = Vec::new();
    write_model(&document, &mut xml).expect("document writes");
    let reparsed = parse_model(&xml).expect("model parses");
    let restored = reparsed.build[0].transform.expect("transform kept");
    assert_eq!(restored.0, matrix.0, "bitwise transform round trip");
}
