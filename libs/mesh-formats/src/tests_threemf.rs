/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Unit tests for the 3MF adapter, both directions: document-to-scene
//! mapping, transform orientation, unit scaling, warnings, error
//! paths, and the scene-to-document writer. Packages are built in
//! memory through `sc-3mf` so every test goes through the real
//! container path.

use std::io::Cursor;
use std::sync::Arc;

use sc_3mf::error::ModelParseError;
use sc_3mf::model::{Matrix3D, ModelDocument, ObjectPayload, ObjectType, parse_model};
use sc_3mf::names::{MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_core::scene::node::{LeafPayload, MeshResourceId, NodeId, NodeKind, SceneMetadata};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{
    CsgOp, IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Scene, Vertex, bake_leaf_to_world,
};

use crate::error::{LoadMeshError, LoadThreeMfError, WriteThreeMfError};
use crate::filetypes::MeshFileType;
use crate::loader::load_mesh_from_reader;
use crate::messages::{LoadMeshWarning, LoadThreeMfWarning};
use crate::threemf::{MAX_COMPONENT_DEPTH, MAX_EXPANDED_LEAF_COUNT, read_threemf, write_threemf};

/// Tetrahedron with vertices at the origin and 10 units along each
/// axis, following the manifold winding used across the workspace.
const TETRA_OBJECT: &str = r#"<object id="1" name="Tetra"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="10"/></vertices><triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/></triangles></mesh></object>"#;

fn model_xml(unit: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><model unit="{unit}" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">{body}</model>"#,
    )
}

fn build_package(model: &str) -> Cursor<Vec<u8>> {
    let mut package = Package::new();
    let part = PartName::new("/3D/3dmodel.model").expect("valid part name");
    package
        .add_part(part, MODEL_PART_CONTENT_TYPE, model.as_bytes().to_vec())
        .expect("unique part name");
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

fn read(unit: &str, body: &str) -> (Scene, Vec<LoadThreeMfWarning>) {
    let mut cursor = build_package(&model_xml(unit, body));
    read_threemf(&mut cursor).expect("load succeeds")
}

fn read_err(unit: &str, body: &str) -> LoadThreeMfError {
    let mut cursor = build_package(&model_xml(unit, body));
    read_threemf(&mut cursor).expect_err("load fails")
}

fn sliced_model_xml(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:s="http://schemas.microsoft.com/3dmanufacturing/slice/2015/07">{body}</model>"#,
    )
}

fn read_sliced(body: &str) -> (Scene, Vec<LoadThreeMfWarning>) {
    let mut cursor = build_package(&sliced_model_xml(body));
    read_threemf(&mut cursor).expect("load succeeds")
}

/// Asserts the loader's root convention (a single identity Group)
/// and returns its children.
fn root_children(scene: &Scene) -> Vec<NodeId> {
    assert_eq!(scene.roots().len(), 1, "expected one scene root");
    let root = scene.roots()[0];
    let node = scene.node(root).expect("root lives");
    assert!(node.local_transform.is_identity(), "root is identity");
    match &node.kind {
        NodeKind::Group { children } => children.clone(),
        other => panic!("root should be a Group, got {other:?}"),
    }
}

fn leaf_resource(scene: &Scene, id: NodeId) -> MeshResourceId {
    match &scene.node(id).expect("node lives").kind {
        NodeKind::Leaf(LeafPayload::MeshResource(rid)) => *rid,
        other => panic!("expected a resource-backed leaf, got {other:?}"),
    }
}

fn group_children(scene: &Scene, id: NodeId) -> Vec<NodeId> {
    match &scene.node(id).expect("node lives").kind {
        NodeKind::Group { children } => children.clone(),
        other => panic!("expected a Group, got {other:?}"),
    }
}

fn resource_mesh(scene: &Scene, rid: MeshResourceId) -> &IndexedMesh {
    scene.mesh_resource(rid).expect("resource lives")
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    #[test]
    fn single_object_single_item_loads_one_resource_leaf() {
        let body =
            format!(r#"<resources>{TETRA_OBJECT}</resources><build><item objectid="1"/></build>"#);
        let (scene, warnings) = read("millimeter", &body);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        let children = root_children(&scene);
        assert_eq!(children.len(), 1);
        let rid = leaf_resource(&scene, children[0]);
        let mesh = resource_mesh(&scene, rid);
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.faces.len(), 4);
        assert_eq!(mesh.vertices[1], Vertex::new([10.0, 0.0, 0.0]));
        assert_eq!(mesh.vertices[3], Vertex::new([0.0, 0.0, 10.0]));
        assert!(
            scene
                .node(children[0])
                .expect("leaf lives")
                .local_transform
                .is_identity(),
        );
    }

    #[test]
    fn object_name_maps_to_mesh_metadata() {
        let body =
            format!(r#"<resources>{TETRA_OBJECT}</resources><build><item objectid="1"/></build>"#);
        let (scene, _) = read("millimeter", &body);
        let children = root_children(&scene);
        let mesh = resource_mesh(&scene, leaf_resource(&scene, children[0]));
        let meta = mesh.meta().as_ref().expect("mesh has metadata");
        assert_eq!(meta.name.as_deref(), Some("Tetra"));
        assert_eq!(meta.header_comment, None);
    }

    #[test]
    fn title_and_description_map_to_scene_metadata() {
        let body = format!(
            r#"<metadata name="Title">Plate 1</metadata><metadata name="Description">Two prisms</metadata><resources>{TETRA_OBJECT}</resources><build><item objectid="1"/></build>"#,
        );
        let (scene, _) = read("millimeter", &body);
        let meta = scene.meta().as_ref().expect("scene has metadata");
        assert_eq!(meta.name.as_deref(), Some("Plate 1"));
        assert_eq!(meta.comment.as_deref(), Some("Two prisms"));
    }

    #[test]
    fn absent_metadata_leaves_scene_meta_none() {
        let body =
            format!(r#"<resources>{TETRA_OBJECT}</resources><build><item objectid="1"/></build>"#);
        let (scene, _) = read("millimeter", &body);
        assert!(scene.meta().is_none());
    }

    #[test]
    fn shared_object_registers_one_resource_with_two_leaves() {
        let body = format!(
            r#"<resources>{TETRA_OBJECT}</resources><build><item objectid="1"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 40 0 0"/></build>"#,
        );
        let (scene, warnings) = read("millimeter", &body);
        assert!(warnings.is_empty());
        let children = root_children(&scene);
        assert_eq!(children.len(), 2);
        let first = leaf_resource(&scene, children[0]);
        let second = leaf_resource(&scene, children[1]);
        assert_eq!(first, second, "both leaves share one mesh resource");
        assert_eq!(scene.mesh_resources_iter().count(), 1);
    }

    #[test]
    fn empty_mesh_object_loads_as_empty_leaf() {
        let body = r#"<resources><object id="1"><mesh/></object></resources><build><item objectid="1"/></build>"#;
        let (scene, _) = read("millimeter", body);
        let children = root_children(&scene);
        let mesh = resource_mesh(&scene, leaf_resource(&scene, children[0]));
        assert!(mesh.vertices.is_empty());
        assert!(mesh.faces.is_empty());
    }

    #[test]
    fn normals_are_recomputed_from_ccw_winding() {
        let body =
            format!(r#"<resources>{TETRA_OBJECT}</resources><build><item objectid="1"/></build>"#);
        let (scene, _) = read("millimeter", &body);
        let children = root_children(&scene);
        let mesh = resource_mesh(&scene, leaf_resource(&scene, children[0]));
        // Triangle 0 spans (0,0,0), (0,10,0), (10,0,0): the base of
        // the tetrahedron, whose outward normal points down.
        assert_eq!(mesh.faces[0].normal, Normal::new([0.0, 0.0, -1.0]));
    }
}

#[cfg(test)]
mod transform_tests {
    use super::*;

    #[test]
    fn transform_orientation_matches_the_row_vector_convention() {
        // Rotation of 90 degrees about +Z followed by a translation,
        // written in 3MF row-vector form: the first matrix row is the
        // image of the x axis, so e_x maps to +Y. A reader that
        // transposes the wrong way would send e_x to -Y instead.
        let body = format!(
            r#"<resources>{TETRA_OBJECT}</resources><build><item objectid="1" transform="0 1 0 -1 0 0 0 0 1 5 6 7"/></build>"#,
        );
        let (scene, _) = read("millimeter", &body);
        let children = root_children(&scene);
        let leaf = scene.node(children[0]).expect("leaf lives");
        let moved = leaf
            .local_transform
            .apply_to_vertex(Vertex::new([10.0, 0.0, 0.0]));
        assert_eq!(moved, Vertex::new([5.0, 16.0, 7.0]));
    }

    #[test]
    fn explicit_identity_transform_collapses_to_the_identity_variant() {
        let body = format!(
            r#"<resources>{TETRA_OBJECT}</resources><build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 0"/></build>"#,
        );
        let (scene, _) = read("millimeter", &body);
        let children = root_children(&scene);
        let leaf = scene.node(children[0]).expect("leaf lives");
        assert!(leaf.local_transform.is_identity());
    }

    #[test]
    fn unit_scale_applies_to_vertices_and_translations() {
        let body = format!(
            r#"<resources>{TETRA_OBJECT}</resources><build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 1 0 0"/></build>"#,
        );
        let (scene, _) = read("inch", &body);
        let children = root_children(&scene);
        let mesh = resource_mesh(&scene, leaf_resource(&scene, children[0]));
        assert_eq!(
            mesh.vertices[1],
            Vertex::new([(10.0_f64 * 25.4) as f32, 0.0, 0.0]),
        );
        let leaf = scene.node(children[0]).expect("leaf lives");
        match leaf.local_transform {
            Transform3D::Affine(a) => assert_eq!(a.translation(), &[25.4, 0.0, 0.0]),
            Transform3D::Identity => panic!("expected an affine transform"),
        }
    }

    #[test]
    fn components_become_groups_carrying_component_transforms() {
        let body = format!(
            r#"<resources>{TETRA_OBJECT}<object id="2"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 20 0 0"/><component objectid="1"/></components></object></resources><build><item objectid="2" transform="1 0 0 0 1 0 0 0 1 0 0 30"/></build>"#,
        );
        let (scene, warnings) = read("millimeter", &body);
        assert!(warnings.is_empty());
        let children = root_children(&scene);
        assert_eq!(children.len(), 1);
        let group = children[0];
        match scene.node(group).expect("group lives").local_transform {
            Transform3D::Affine(a) => assert_eq!(a.translation(), &[0.0, 0.0, 30.0]),
            Transform3D::Identity => panic!("expected the item transform on the group"),
        }
        let leaves = group_children(&scene, group);
        assert_eq!(leaves.len(), 2);
        assert_eq!(
            leaf_resource(&scene, leaves[0]),
            leaf_resource(&scene, leaves[1]),
        );
        assert_eq!(scene.mesh_resources_iter().count(), 1);
        // First component carries its translation; the second is
        // identity-placed.
        match scene.node(leaves[0]).expect("leaf lives").local_transform {
            Transform3D::Affine(a) => assert_eq!(a.translation(), &[20.0, 0.0, 0.0]),
            Transform3D::Identity => panic!("expected the component transform on the leaf"),
        }
        assert!(
            scene
                .node(leaves[1])
                .expect("leaf lives")
                .local_transform
                .is_identity(),
        );
    }

    #[test]
    fn nested_components_compose_through_the_ancestor_chain() {
        let body = format!(
            r#"<resources>{TETRA_OBJECT}<object id="2"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 3 0 0"/></components></object><object id="3"><components><component objectid="2" transform="1 0 0 0 1 0 0 0 1 0 2 0"/></components></object></resources><build><item objectid="3" transform="1 0 0 0 1 0 0 0 1 0 0 1"/></build>"#,
        );
        let (scene, _) = read("millimeter", &body);
        let children = root_children(&scene);
        let outer = group_children(&scene, children[0]);
        assert_eq!(outer.len(), 1);
        let inner = group_children(&scene, outer[0]);
        assert_eq!(inner.len(), 1);
        let baked = bake_leaf_to_world(&scene, inner[0]).expect("leaf bakes");
        assert_eq!(baked.vertices[1], Vertex::new([13.0, 2.0, 1.0]));
    }
}

#[cfg(test)]
mod warning_tests {
    use super::*;

    #[test]
    fn base_materials_presence_warns_materials_dropped() {
        let body = format!(
            r##"<resources><basematerials id="9"><base name="Green" displaycolor="#21BB4CFF"/></basematerials>{TETRA_OBJECT}</resources><build><item objectid="1"/></build>"##,
        );
        let (_, warnings) = read("millimeter", &body);
        assert_eq!(
            warnings,
            vec![LoadThreeMfWarning::MaterialsDropped { group_count: 1 }],
        );
    }

    #[test]
    fn objects_unreachable_from_the_build_warn_as_skipped() {
        let body = format!(
            r#"<resources>{TETRA_OBJECT}<object id="2" name="Spare"><mesh/></object></resources><build><item objectid="1"/></build>"#,
        );
        let (scene, warnings) = read("millimeter", &body);
        assert_eq!(root_children(&scene).len(), 1);
        assert_eq!(scene.mesh_resources_iter().count(), 1);
        assert_eq!(
            warnings,
            vec![LoadThreeMfWarning::UnbuiltObjectSkipped {
                id: 2,
                name: Some("Spare".to_string()),
            }],
        );
    }

    #[test]
    fn non_model_object_types_load_with_a_flattening_warning() {
        let body = r#"<resources><object id="1" type="support"><mesh/></object></resources><build><item objectid="1"/></build>"#;
        let (scene, warnings) = read("millimeter", body);
        assert_eq!(root_children(&scene).len(), 1);
        assert_eq!(
            warnings,
            vec![LoadThreeMfWarning::NonModelObjectTypeFlattened {
                id: 1,
                object_type: ObjectType::Support,
            }],
        );
    }

    #[test]
    fn non_model_type_warning_is_emitted_once_per_object() {
        let body = r#"<resources><object id="1" type="surface"><mesh/></object></resources><build><item objectid="1"/><item objectid="1"/></build>"#;
        let (_, warnings) = read("millimeter", body);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn empty_build_warns_and_loads_an_empty_scene() {
        let (scene, warnings) = read("millimeter", r#"<resources/><build/>"#);
        assert!(root_children(&scene).is_empty());
        assert_eq!(warnings, vec![LoadThreeMfWarning::EmptyBuild]);
    }

    #[test]
    fn slice_stacks_load_the_mesh_and_warn_slice_data_ignored() {
        let body = format!(
            r#"<resources><s:slicestack id="9" zbottom="0"><s:slice ztop="0.5"/></s:slicestack>{}</resources><build><item objectid="1"/></build>"#,
            TETRA_OBJECT.replace(
                r#"<object id="1" name="Tetra">"#,
                r#"<object id="1" name="Tetra" s:slicestackid="9">"#
            ),
        );
        let (scene, warnings) = read_sliced(&body);
        assert_eq!(root_children(&scene).len(), 1);
        assert_eq!(
            warnings,
            vec![LoadThreeMfWarning::SliceDataIgnored { stack_count: 1 }],
        );
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn garbage_bytes_error_at_the_package_layer() {
        let mut cursor = Cursor::new(b"this is not a zip archive".to_vec());
        let err = read_threemf(&mut cursor).expect_err("garbage fails");
        assert!(matches!(err, LoadThreeMfError::Package(_)));
    }

    #[test]
    fn model_parse_errors_pass_through() {
        let err = read_err("furlong", r#"<resources/><build/>"#);
        assert!(matches!(
            err,
            LoadThreeMfError::Model(ModelParseError::InvalidUnit { .. }),
        ));
    }

    #[test]
    fn start_part_with_a_foreign_content_type_is_refused() {
        let mut package = Package::new();
        let part = PartName::new("/3D/3dmodel.model").expect("valid part name");
        package
            .add_part(part, "text/plain", b"not a model".to_vec())
            .expect("unique part name");
        package.add_root_relationship(Relationship {
            id: "rel-1".to_string(),
            rel_type: START_PART_REL_TYPE.to_string(),
            target: "/3D/3dmodel.model".to_string(),
        });
        let mut cursor = Cursor::new(Vec::new());
        package.write_to(&mut cursor).expect("in-memory write");
        cursor.set_position(0);
        let err = read_threemf(&mut cursor).expect_err("wrong content type fails");
        assert!(matches!(
            err,
            LoadThreeMfError::StartPartNotAModel { content_type } if content_type == "text/plain",
        ));
    }

    #[test]
    fn coordinates_overflowing_f32_after_scaling_are_refused() {
        // 1e37 metres scale to 1e40 mm: finite in f64, infinite once
        // downcast to the mesh's f32 coordinates.
        let body = r#"<resources><object id="1"><mesh><vertices><vertex x="1e37" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build>"#;
        let err = read_err("meter", body);
        assert!(matches!(
            err,
            LoadThreeMfError::NonFiniteVertex { object_id: 1 },
        ));
    }

    #[test]
    fn lowres_stand_in_meshes_are_refused() {
        let body = r#"<resources><object id="1" s:meshresolution="lowres"><mesh/></object></resources><build><item objectid="1"/></build>"#;
        let mut cursor = build_package(&sliced_model_xml(body));
        let err = read_threemf(&mut cursor).expect_err("lowres refused");
        assert!(matches!(
            err,
            LoadThreeMfError::LowresMeshUnsupported { object_id: 1 },
        ));
    }

    #[test]
    fn component_chains_deeper_than_the_cap_are_refused() {
        let mut body = String::from("<resources>");
        body.push_str(TETRA_OBJECT);
        let last = MAX_COMPONENT_DEPTH as u32 + 3;
        for k in 2..=last {
            body.push_str(&format!(
                r#"<object id="{k}"><components><component objectid="{}"/></components></object>"#,
                k - 1,
            ));
        }
        body.push_str(&format!(
            r#"</resources><build><item objectid="{last}"/></build>"#,
        ));
        let err = read_err("millimeter", &body);
        assert!(matches!(
            err,
            LoadThreeMfError::ComponentDepthExceeded { limit } if limit == MAX_COMPONENT_DEPTH,
        ));
    }

    #[test]
    fn component_expansions_beyond_the_leaf_budget_are_refused() {
        // Each level holds 2 components of the previous object, so
        // level n expands to 2^(n-1) leaves; 18 levels overshoot the
        // 100_000 budget.
        let mut body = String::from("<resources>");
        body.push_str(TETRA_OBJECT);
        for k in 2_u32..=18 {
            body.push_str(&format!(
                r#"<object id="{k}"><components><component objectid="{prev}"/><component objectid="{prev}"/></components></object>"#,
                prev = k - 1,
            ));
        }
        body.push_str(r#"</resources><build><item objectid="18"/></build>"#);
        let err = read_err("millimeter", &body);
        assert!(matches!(
            err,
            LoadThreeMfError::LeafBudgetExceeded { limit } if limit == MAX_EXPANDED_LEAF_COUNT,
        ));
    }
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    fn tri_mesh(name: Option<&str>) -> IndexedMesh {
        IndexedMesh {
            meta: name.map(|n| MeshMetadata {
                name: Some(n.to_string()),
                ..Default::default()
            }),
            vertices: vec![
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.577, 0.577, 0.577]),
                [0, 1, 2],
            )],
        }
    }

    /// Writes the scene and parses the resulting package back into a
    /// document, so assertions run on the emitted structure. A
    /// successful parse also proves definition-before-use ordering,
    /// which `parse_model` enforces.
    fn written_document(scene: &Scene) -> ModelDocument {
        let mut bytes = Vec::new();
        write_threemf(scene, &mut bytes).expect("write succeeds");
        let package = Package::read_from(Cursor::new(bytes)).expect("package parses");
        let start = package.start_part().expect("start part resolves");
        parse_model(package.part_data(&start).expect("start part present")).expect("model parses")
    }

    /// Loader-shape scene: anonymous identity Group root with the
    /// given leaf payloads as children.
    fn loader_shape_scene(payloads: Vec<(LeafPayload, Transform3D)>) -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        for (payload, transform) in payloads {
            let leaf = g.add_leaf(None, transform, payload).expect("add leaf");
            g.add_child(root, leaf).expect("attach leaf");
        }
        g
    }

    #[test]
    fn bare_leaf_root_writes_one_object_and_one_item() {
        let scene = Scene::from(tri_mesh(Some("Solo")));
        let document = written_document(&scene);
        assert_eq!(document.objects.len(), 1);
        assert_eq!(document.objects[0].name.as_deref(), Some("Solo"));
        match &document.objects[0].payload {
            ObjectPayload::Mesh(data) => {
                assert_eq!(data.vertices.len(), 3);
                assert_eq!(data.triangles.len(), 1);
                assert_eq!(data.vertices[0].x, 1.0);
            }
            other => panic!("expected a mesh payload, got {other:?}"),
        }
        assert_eq!(document.build.len(), 1);
        assert_eq!(document.build[0].object_id, document.objects[0].id);
        assert!(document.build[0].transform.is_none());
    }

    #[test]
    fn loader_shaped_root_group_collapses_into_build_items() {
        let scene = loader_shape_scene(vec![
            (
                LeafPayload::Mesh(Arc::new(tri_mesh(Some("A")))),
                Transform3D::Identity,
            ),
            (
                LeafPayload::Mesh(Arc::new(tri_mesh(Some("B")))),
                Transform3D::Affine(AffineTransform3D::from_translation([4.0, 0.0, 0.0])),
            ),
        ]);
        let document = written_document(&scene);
        // The wrapping group vanishes: 2 mesh objects, 2 items, no
        // components object.
        assert_eq!(document.objects.len(), 2);
        assert!(
            document
                .objects
                .iter()
                .all(|o| matches!(o.payload, ObjectPayload::Mesh(_))),
        );
        assert_eq!(document.build.len(), 2);
        assert_eq!(
            document.build[1].transform,
            Some(Matrix3D([
                1.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, //
                0.0, 0.0, 1.0, //
                4.0, 0.0, 0.0,
            ])),
        );
    }

    #[test]
    fn named_root_group_stays_a_components_object() {
        let mut g = Scene::new();
        let root = g.add_group(
            Some(SceneMetadata {
                name: Some("Assembly".to_string()),
                comment: None,
            }),
            Transform3D::Identity,
        );
        g.add_root(root).expect("group root");
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh(None))),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");

        let document = written_document(&g);
        assert_eq!(document.build.len(), 1);
        let root_object = document
            .object(document.build[0].object_id)
            .expect("build references the group object");
        assert_eq!(root_object.name.as_deref(), Some("Assembly"));
        match &root_object.payload {
            ObjectPayload::Components(components) => assert_eq!(components.len(), 1),
            other => panic!("expected components, got {other:?}"),
        }
    }

    #[test]
    fn transformed_anonymous_root_group_stays_a_components_object() {
        let mut g = Scene::new();
        let root = g.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 9.0])),
        );
        g.add_root(root).expect("group root");
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh(None))),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");

        let document = written_document(&g);
        assert_eq!(document.build.len(), 1);
        assert_eq!(
            document.build[0].transform,
            Some(Matrix3D([
                1.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, //
                0.0, 0.0, 1.0, //
                0.0, 0.0, 9.0,
            ])),
        );
        let root_object = document
            .object(document.build[0].object_id)
            .expect("build references the group object");
        assert!(matches!(&root_object.payload, ObjectPayload::Components(_)));
    }

    #[test]
    fn shared_mesh_resource_dedupes_into_one_object() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let rid = g.add_mesh_resource(tri_mesh(Some("Shared")));
        for x in [0.0, 12.0] {
            let leaf = g
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.0])),
                    LeafPayload::MeshResource(rid),
                )
                .expect("add leaf");
            g.add_child(root, leaf).expect("attach leaf");
        }

        let document = written_document(&g);
        assert_eq!(document.objects.len(), 1);
        assert_eq!(document.build.len(), 2);
        assert_eq!(document.build[0].object_id, document.build[1].object_id);
    }

    #[test]
    fn rotation_transform_writes_in_row_vector_form() {
        // Exact 90-degree rotation about +Z in our column-vector
        // layout; the written attribute must transpose into the
        // row-vector layout the reader test pins from the other side.
        let rotation = AffineTransform3D::from_linear_and_translation(
            [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [5.0, 6.0, 7.0],
        );
        let scene = loader_shape_scene(vec![(
            LeafPayload::Mesh(Arc::new(tri_mesh(None))),
            Transform3D::Affine(rotation),
        )]);
        let document = written_document(&scene);
        assert_eq!(
            document.build[0].transform,
            Some(Matrix3D([
                0.0, 1.0, 0.0, //
                -1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, //
                5.0, 6.0, 7.0,
            ])),
        );
    }

    #[test]
    fn scene_metadata_writes_title_and_description() {
        let mut scene = Scene::from(tri_mesh(None));
        scene.set_meta(Some(SceneMetadata {
            name: Some("Plate 1".to_string()),
            comment: Some("Two prisms".to_string()),
        }));
        let document = written_document(&scene);
        let title = document
            .metadata
            .iter()
            .find(|m| m.name == "Title")
            .expect("Title present");
        assert_eq!(title.value, "Plate 1");
        let description = document
            .metadata
            .iter()
            .find(|m| m.name == "Description")
            .expect("Description present");
        assert_eq!(description.value, "Two prisms");
    }

    #[test]
    fn nested_groups_emit_children_before_parents() {
        let mut g = Scene::new();
        let outer = g.add_group(
            Some(SceneMetadata {
                name: Some("Outer".to_string()),
                comment: None,
            }),
            Transform3D::Identity,
        );
        g.add_root(outer).expect("outer root");
        let inner = g.add_group(
            Some(SceneMetadata {
                name: Some("Inner".to_string()),
                comment: None,
            }),
            Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        );
        g.add_child(outer, inner).expect("attach inner");
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh(None))),
            )
            .expect("add leaf");
        g.add_child(inner, leaf).expect("attach leaf");

        // `written_document` parsing succeeding is itself the
        // definition-before-use proof; pin the id order too.
        let document = written_document(&g);
        assert_eq!(document.objects.len(), 3);
        assert!(matches!(
            &document.objects[0].payload,
            ObjectPayload::Mesh(_),
        ));
        assert_eq!(document.objects[1].name.as_deref(), Some("Inner"));
        assert_eq!(document.objects[2].name.as_deref(), Some("Outer"));
    }

    #[test]
    fn empty_scene_writes_an_empty_document() {
        let scene = Scene::new();
        let document = written_document(&scene);
        assert!(document.objects.is_empty());
        assert!(document.build.is_empty());

        // Reading the same bytes back surfaces the empty build as the
        // reader's EmptyBuild warning.
        let mut bytes = Vec::new();
        write_threemf(&scene, &mut bytes).expect("write succeeds");
        let (_, warnings) = read_threemf(&mut Cursor::new(bytes)).expect("read succeeds");
        assert_eq!(warnings, vec![LoadThreeMfWarning::EmptyBuild]);
    }

    #[test]
    fn csg_nodes_are_refused() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh(None))),
            )
            .expect("add leaf");
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![leaf])
            .expect("add csg");
        g.add_root(csg).expect("csg root");

        let mut bytes = Vec::new();
        let err = write_threemf(&g, &mut bytes).expect_err("csg refused");
        assert!(matches!(
            err,
            WriteThreeMfError::CsgUnrepresentable { node_id } if node_id == csg,
        ));
    }

    /// Subtree resource holding one triangle leaf under an identity
    /// group; returns the resource id.
    fn triangle_subtree(g: &mut Scene) -> sc_mesh_core::SubtreeResourceId {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh(None))),
            )
            .expect("add subtree leaf");
        let root = g.add_group(None, Transform3D::Identity);
        g.add_child(root, leaf).expect("attach subtree leaf");
        g.register_subtree_resource(root).expect("register subtree")
    }

    fn translation_row(matrix: &Option<Matrix3D>) -> [f64; 3] {
        let m = matrix.as_ref().expect("transform attribute present");
        [m.0[9], m.0[10], m.0[11]]
    }

    #[test]
    fn instances_write_as_build_items_sharing_one_object() {
        // 2 instances of the same subtree: the geometry serialises
        // once, and each occurrence is a build item referencing the
        // shared object under its own transform.
        let mut g = Scene::new();
        let rid = triangle_subtree(&mut g);
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        for x in [10.0, 20.0] {
            let inst = g
                .add_instance(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.0])),
                    rid,
                )
                .expect("add instance");
            g.add_child(root, inst).expect("attach instance");
        }

        let document = written_document(&g);
        assert_eq!(document.objects.len(), 2);
        let mesh_objects = document
            .objects
            .iter()
            .filter(|o| matches!(o.payload, ObjectPayload::Mesh(_)))
            .count();
        assert_eq!(mesh_objects, 1, "shared geometry serialises once");
        let subtree_object = document
            .objects
            .iter()
            .find(|o| matches!(o.payload, ObjectPayload::Components(_)))
            .expect("the subtree becomes a components object");

        assert_eq!(document.build.len(), 2);
        for (item, expected_x) in document.build.iter().zip([10.0, 20.0]) {
            assert_eq!(item.object_id, subtree_object.id);
            assert_eq!(translation_row(&item.transform), [expected_x, 0.0, 0.0]);
        }
    }

    #[test]
    fn nested_instances_write_as_nested_components() {
        // Outer subtree holds 2 instances of the inner subtree; the
        // scene holds 2 instances of the outer. 4 expanded
        // occurrences, but the geometry serialises exactly once and
        // the document stays proportional to the node count.
        let mut g = Scene::new();
        let inner = triangle_subtree(&mut g);
        let outer_root = g.add_group(None, Transform3D::Identity);
        for x in [2.0, 4.0] {
            let inst = g
                .add_instance(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.0])),
                    inner,
                )
                .expect("add inner instance");
            g.add_child(outer_root, inst)
                .expect("attach inner instance");
        }
        let outer = g
            .register_subtree_resource(outer_root)
            .expect("register outer subtree");
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        for y in [0.0, 10.0] {
            let inst = g
                .add_instance(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([0.0, y, 0.0])),
                    outer,
                )
                .expect("add outer instance");
            g.add_child(root, inst).expect("attach outer instance");
        }

        let document = written_document(&g);
        // 1 mesh object, the inner subtree object, the outer subtree
        // object; nothing per occurrence.
        assert_eq!(document.objects.len(), 3);
        let mesh_objects = document
            .objects
            .iter()
            .filter(|o| matches!(o.payload, ObjectPayload::Mesh(_)))
            .count();
        assert_eq!(mesh_objects, 1, "shared geometry serialises once");

        let outer_object = document
            .objects
            .iter()
            .rfind(|o| matches!(o.payload, ObjectPayload::Components(_)))
            .expect("the outer subtree becomes a components object");
        let ObjectPayload::Components(components) = &outer_object.payload else {
            unreachable!("filtered on the components payload");
        };
        assert_eq!(components.len(), 2);
        assert_eq!(translation_row(&components[0].transform), [2.0, 0.0, 0.0]);
        assert_eq!(translation_row(&components[1].transform), [4.0, 0.0, 0.0]);

        assert_eq!(document.build.len(), 2);
        assert_eq!(
            translation_row(&document.build[1].transform),
            [0.0, 10.0, 0.0]
        );
    }

    #[test]
    fn instance_transform_composes_with_the_subtree_root_transform() {
        // A 3MF reference carries a single matrix, so the instance's
        // call-site transform and the subtree root's own transform
        // must compose into it, in call-site-first order.
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(tri_mesh(None))),
            )
            .expect("add subtree leaf");
        let root_group = g.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        );
        g.add_child(root_group, leaf).expect("attach subtree leaf");
        let rid = g
            .register_subtree_resource(root_group)
            .expect("register subtree");
        let inst = g
            .add_instance(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
                rid,
            )
            .expect("add instance");
        g.add_root(inst).expect("instance root");

        let document = written_document(&g);
        assert_eq!(document.build.len(), 1);
        assert_eq!(
            translation_row(&document.build[0].transform),
            [11.0, 0.0, 0.0]
        );
    }

    #[test]
    fn an_instanced_subtree_round_trips_through_the_reader() {
        // The reader expands components into per-occurrence leaves,
        // so the written package loads back as 2 triangles at the 2
        // call sites.
        let mut g = Scene::new();
        let rid = triangle_subtree(&mut g);
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        for x in [10.0, 20.0] {
            let inst = g
                .add_instance(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.0])),
                    rid,
                )
                .expect("add instance");
            g.add_child(root, inst).expect("attach instance");
        }

        let mut bytes = Vec::new();
        write_threemf(&g, &mut bytes).expect("write succeeds");
        let (loaded, warnings) = read_threemf(&mut Cursor::new(bytes)).expect("read succeeds");
        assert!(warnings.is_empty());
        assert_eq!(loaded.reachable_leaf_count(), 2);

        let merged = loaded
            .merge_meshes()
            .expect("merge succeeds")
            .expect("geometry present");
        assert_eq!(merged.faces.len(), 2);
        // tri_mesh's first vertex sits at (1, 0, 0); the occurrences
        // place it at x = 11 and x = 21.
        assert!(
            merged
                .vertices
                .iter()
                .any(|v| v.coords() == [11.0, 0.0, 0.0])
        );
        assert!(
            merged
                .vertices
                .iter()
                .any(|v| v.coords() == [21.0, 0.0, 0.0])
        );
    }
}

#[cfg(test)]
mod loader_dispatch_tests {
    use super::*;

    #[test]
    fn load_mesh_from_reader_dispatches_the_3mf_arm() {
        let body = format!(
            r##"<resources><basematerials id="9"><base name="Red" displaycolor="#FF0000"/></basematerials>{TETRA_OBJECT}</resources><build><item objectid="1"/></build>"##,
        );
        let mut cursor = build_package(&model_xml("millimeter", &body));
        let loaded = load_mesh_from_reader(MeshFileType::_3mf, &mut cursor).expect("load succeeds");
        assert_eq!(root_children(&loaded.graph).len(), 1);
        assert_eq!(
            loaded.warnings,
            vec![LoadMeshWarning::ThreeMf(
                LoadThreeMfWarning::MaterialsDropped { group_count: 1 },
            )],
        );
    }

    #[test]
    fn threemf_errors_surface_as_load_mesh_errors() {
        let mut cursor = Cursor::new(b"junk".to_vec());
        let err = load_mesh_from_reader(MeshFileType::_3mf, &mut cursor).expect_err("junk fails");
        assert!(matches!(
            err,
            LoadMeshError::ThreeMf(LoadThreeMfError::Package(_)),
        ));
    }
}
