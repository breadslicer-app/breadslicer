/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Unit tests for the 3MF reader adapter: document-to-scene mapping,
//! transform orientation, unit scaling, warnings, and error paths.
//! Packages are built in memory through `sc-3mf` so every test goes
//! through the real container path.

use std::io::Cursor;

use sc_3mf::error::ModelParseError;
use sc_3mf::model::ObjectType;
use sc_3mf::names::{MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_core::scene::node::{LeafPayload, MeshResourceId, NodeId, NodeKind};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, Normal, Scene, Vertex, bake_leaf_to_world};

use crate::error::{LoadMeshError, LoadThreeMfError};
use crate::filetypes::MeshFileType;
use crate::loader::load_mesh_from_reader;
use crate::messages::{LoadMeshWarning, LoadThreeMfWarning};
use crate::threemf::{MAX_COMPONENT_DEPTH, MAX_EXPANDED_LEAF_COUNT, read_threemf};

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
