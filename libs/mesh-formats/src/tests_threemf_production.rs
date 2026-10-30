/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tests for 3MF packages whose start part references objects in other model
//! parts through the production extension's `p:path`, as Bambu Studio and
//! Orca write every object.

use std::io::Cursor;

use sc_3mf::error::ModelParseError;
use sc_3mf::names::{MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_core::Vertex;
use sc_scene::node::{LeafPayload, NodeKind};
use sc_scene::{Scene, bake_leaf_to_world};

use crate::error::LoadThreeMfError;
use crate::messages::LoadThreeMfWarning;
use crate::threemf::read_threemf;

const PRODUCTION_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";

/// Tetrahedron with vertices at the origin and 10 units along each axis.
fn tetra_object(id: u32) -> String {
    format!(
        r#"<object id="{id}" name="Tetra"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="10"/></vertices><triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/></triangles></mesh></object>"#,
    )
}

fn model_xml(unit: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><model unit="{unit}" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="{PRODUCTION_NS}" requiredextensions="p">{body}</model>"#,
    )
}

/// The start part as Bambu Studio writes it: an assembly whose component is
/// object 1 of `/3D/Objects/object_1.model`, placed by a build item.
fn bambu_start_part() -> String {
    model_xml(
        "millimeter",
        r#"<resources><object id="2" p:UUID="00000001-61cb-4c03-9d28-80fed5dfa1dc" type="model"><components><component p:path="/3D/Objects/object_1.model" objectid="1" p:UUID="00010000-b206-40ff-9872-83e8017abed1" transform="1 0 0 0 1 0 0 0 1 0 0 1"/></components></object></resources><build p:UUID="2c7c17d8-22b5-4d84-8835-1976022ea369"><item objectid="2" p:UUID="00000002-b1ec-4553-aec9-835e5b724bb4" transform="1 0 0 0 1 0 0 0 1 128 128 2.5" printable="1"/></build>"#,
    )
}

/// Returns a package holding the start part `start` and the `parts` (name,
/// content type, XML).
fn package(start: &str, parts: &[(&str, &str, &str)]) -> Cursor<Vec<u8>> {
    let mut package = Package::new();
    package
        .add_part(
            PartName::new("/3D/3dmodel.model").expect("valid part name"),
            MODEL_PART_CONTENT_TYPE,
            start.as_bytes().to_vec(),
        )
        .expect("unique part name");
    for (name, content_type, xml) in parts {
        package
            .add_part(
                PartName::new(name).expect("valid part name"),
                content_type,
                xml.as_bytes().to_vec(),
            )
            .expect("unique part name");
    }
    package.add_root_relationship(Relationship {
        id: "rel-1".to_string(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: "/3D/3dmodel.model".to_string(),
    });
    let mut cursor = Cursor::new(Vec::new());
    package.write_to(&mut cursor).expect("in-memory write");
    cursor.set_position(0);
    cursor
}

fn read(start: &str, parts: &[(&str, &str, &str)]) -> (Scene, Vec<LoadThreeMfWarning>) {
    read_threemf(&mut package(start, parts)).expect("load succeeds")
}

fn read_err(start: &str, parts: &[(&str, &str, &str)]) -> LoadThreeMfError {
    read_threemf(&mut package(start, parts)).expect_err("load fails")
}

/// Returns the world-space meshes of every leaf, in depth-first order.
fn baked_leaves(scene: &Scene) -> Vec<Vec<Vertex>> {
    let mut leaves = Vec::new();
    let mut stack: Vec<_> = scene.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        match &scene.node(id).expect("node lives").kind {
            NodeKind::Group { children } => stack.extend(children.iter().rev()),
            NodeKind::Leaf(LeafPayload::MeshResource(_)) => {
                leaves.push(bake_leaf_to_world(scene, id).expect("leaf bakes").vertices);
            }
            other => panic!("unexpected node {other:?}"),
        }
    }
    leaves
}

#[test]
fn a_component_in_another_part_loads_with_its_placement() {
    let object = model_xml(
        "millimeter",
        &format!("<resources>{}</resources><build/>", tetra_object(1)),
    );
    let (scene, warnings) = read(
        &bambu_start_part(),
        &[(
            "/3D/Objects/object_1.model",
            MODEL_PART_CONTENT_TYPE,
            &object,
        )],
    );
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    let leaves = baked_leaves(&scene);
    assert_eq!(leaves.len(), 1);
    // Build item (128, 128, 2.5) after the component's lift of 1.
    assert_eq!(leaves[0][0], Vertex::new([128.0, 128.0, 3.5]));
    assert_eq!(leaves[0][1], Vertex::new([138.0, 128.0, 3.5]));
}

#[test]
fn a_referenced_part_is_read_in_its_own_unit() {
    // The start part is in centimetres, the object in millimetres: the
    // component's lift of 1 is 10 mm, the tetrahedron's edge stays 10 mm.
    let start = model_xml(
        "centimeter",
        r#"<resources><object id="2"><components><component p:path="/3D/Objects/object_1.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 1"/></components></object></resources><build><item objectid="2"/></build>"#,
    );
    let object = model_xml(
        "millimeter",
        &format!("<resources>{}</resources><build/>", tetra_object(1)),
    );
    let (scene, _) = read(
        &start,
        &[(
            "/3D/Objects/object_1.model",
            MODEL_PART_CONTENT_TYPE,
            &object,
        )],
    );
    let leaves = baked_leaves(&scene);
    assert_eq!(leaves[0][0], Vertex::new([0.0, 0.0, 10.0]));
    assert_eq!(leaves[0][1], Vertex::new([10.0, 0.0, 10.0]));
}

#[test]
fn ids_of_different_parts_do_not_collide() {
    // Object 1 of the start part and object 1 of the referenced part are 2
    // objects; 2 references to the referenced one share 1 mesh resource.
    let start = model_xml(
        "millimeter",
        &format!(
            r#"<resources>{}<object id="2"><components><component p:path="/3D/Objects/object_1.model" objectid="1"/><component p:path="/3D/OBJECTS/object_1.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 50 0 0"/></components></object></resources><build><item objectid="1"/><item objectid="2"/></build>"#,
            tetra_object(1).replace(r#"x="10""#, r#"x="20""#),
        ),
    );
    let object = model_xml(
        "millimeter",
        &format!("<resources>{}</resources><build/>", tetra_object(1)),
    );
    let (scene, warnings) = read(
        &start,
        &[(
            "/3D/Objects/object_1.model",
            MODEL_PART_CONTENT_TYPE,
            &object,
        )],
    );
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    let leaves = baked_leaves(&scene);
    assert_eq!(leaves.len(), 3);
    assert_eq!(leaves[0][1], Vertex::new([20.0, 0.0, 0.0]));
    assert_eq!(leaves[1][1], Vertex::new([10.0, 0.0, 0.0]));
    assert_eq!(leaves[2][1], Vertex::new([60.0, 0.0, 0.0]));
    assert_eq!(scene.mesh_resources_len(), 2);
}

#[test]
fn a_build_item_in_another_part_loads() {
    let start = model_xml(
        "millimeter",
        r#"<resources/><build><item objectid="5" p:path="/3D/Objects/object_5.model" transform="1 0 0 0 1 0 0 0 1 0 7 0"/></build>"#,
    );
    let object = model_xml(
        "millimeter",
        &format!("<resources>{}</resources><build/>", tetra_object(5)),
    );
    let (scene, _) = read(
        &start,
        &[(
            "/3D/Objects/object_5.model",
            MODEL_PART_CONTENT_TYPE,
            &object,
        )],
    );
    let leaves = baked_leaves(&scene);
    assert_eq!(leaves.len(), 1);
    assert_eq!(leaves[0][0], Vertex::new([0.0, 7.0, 0.0]));
}

#[test]
fn a_missing_part_is_refused() {
    let err = read_err(&bambu_start_part(), &[]);
    assert!(
        matches!(&err, LoadThreeMfError::ReferencedPartNotFound { path } if path == "/3D/Objects/object_1.model"),
        "{err:?}",
    );
}

#[test]
fn a_referenced_part_that_is_not_a_model_is_refused() {
    let err = read_err(
        &bambu_start_part(),
        &[("/3D/Objects/object_1.model", "text/plain", "text")],
    );
    assert!(
        matches!(&err, LoadThreeMfError::ReferencedPartNotAModel { content_type, .. } if content_type == "text/plain"),
        "{err:?}",
    );
}

#[test]
fn a_missing_object_is_refused_with_its_part() {
    let object = model_xml(
        "millimeter",
        &format!("<resources>{}</resources><build/>", tetra_object(3)),
    );
    let err = read_err(
        &bambu_start_part(),
        &[(
            "/3D/Objects/object_1.model",
            MODEL_PART_CONTENT_TYPE,
            &object,
        )],
    );
    assert!(
        matches!(&err, LoadThreeMfError::ReferencedModel { path, source: ModelParseError::UnknownObjectReference { id: 1 } } if path == "/3D/Objects/object_1.model"),
        "{err:?}",
    );
}

#[test]
fn a_build_item_of_type_other_in_another_part_is_refused() {
    let start = model_xml(
        "millimeter",
        r#"<resources/><build><item objectid="1" p:path="/3D/Objects/object_1.model"/></build>"#,
    );
    let object = model_xml(
        "millimeter",
        &format!(
            "<resources>{}</resources><build/>",
            tetra_object(1).replace(r#"name="Tetra""#, r#"type="other""#)
        ),
    );
    let err = read_err(
        &start,
        &[(
            "/3D/Objects/object_1.model",
            MODEL_PART_CONTENT_TYPE,
            &object,
        )],
    );
    assert!(
        matches!(
            &err,
            LoadThreeMfError::ReferencedModel {
                source: ModelParseError::BuildItemReferencesOtherType { id: 1 },
                ..
            }
        ),
        "{err:?}",
    );
}

#[test]
fn a_referenced_part_that_references_further_is_refused() {
    let object = model_xml(
        "millimeter",
        r#"<resources><object id="1"><components><component p:path="/3D/Objects/deeper.model" objectid="1"/></components></object></resources><build/>"#,
    );
    let err = read_err(
        &bambu_start_part(),
        &[(
            "/3D/Objects/object_1.model",
            MODEL_PART_CONTENT_TYPE,
            &object,
        )],
    );
    assert!(
        matches!(&err, LoadThreeMfError::NestedProductionPath { path } if path == "/3D/Objects/object_1.model"),
        "{err:?}",
    );
}

#[test]
fn a_referenced_part_that_does_not_parse_names_the_part() {
    let err = read_err(
        &bambu_start_part(),
        &[(
            "/3D/Objects/object_1.model",
            MODEL_PART_CONTENT_TYPE,
            "<model/>",
        )],
    );
    assert!(
        matches!(&err, LoadThreeMfError::ReferencedModel { path, .. } if path == "/3D/Objects/object_1.model"),
        "{err:?}",
    );
    assert!(
        err.to_string().contains("/3D/Objects/object_1.model"),
        "{err}"
    );
}
