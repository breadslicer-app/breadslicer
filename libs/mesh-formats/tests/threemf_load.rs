/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Integration coverage for 3MF loading through the public loader
//! entry points: the vendored 3MF Consortium sample packages (see
//! tests/fixtures/official/README.md) and a generated multi-object
//! scene built from the workspace's cube and tunnel-prism fixtures.

use std::io::Cursor;
use std::path::PathBuf;

use sc_3mf::model::{
    BuildItem, Matrix3D, MeshData, MeshResolution, ModelDocument, ObjectPayload, ObjectResource,
    ObjectType, ResourceId, ResourceIndex, TriangleRef, Unit, Vertex3, write_model,
};
use sc_3mf::names::{MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_mesh_formats::filetypes::MeshFileType;
use sc_mesh_formats::messages::{LoadMeshWarning, LoadThreeMfWarning, SceneLoadWarning};
use sc_mesh_formats::{LoadedMesh, load_mesh_from_path, load_mesh_from_reader};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, MeshResourceId, NodeId, NodeKind};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/official/3mf-samples")
        .join(name)
}

fn load_official(name: &str) -> LoadedMesh {
    load_mesh_from_path(&fixture_path(name), None, None)
        .unwrap_or_else(|err| panic!("official sample {name} should load: {err}"))
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

fn single_leaf_mesh(scene: &Scene) -> &IndexedMesh {
    let children = root_children(scene);
    assert_eq!(children.len(), 1);
    scene
        .mesh_resource(leaf_resource(scene, children[0]))
        .expect("resource lives")
}

#[test]
fn box_sample_loads_as_one_cube_leaf() {
    let loaded = load_official("box.3mf");
    assert!(loaded.warnings.is_empty(), "got {:?}", loaded.warnings);
    let mesh = single_leaf_mesh(&loaded.graph);
    assert_eq!(mesh.vertices.len(), 8);
    assert_eq!(mesh.faces.len(), 12);
}

#[test]
fn cylinder_sample_loads_with_known_counts() {
    let loaded = load_official("cylinder.3mf");
    let mesh = single_leaf_mesh(&loaded.graph);
    assert_eq!(mesh.vertices.len(), 46);
    assert_eq!(mesh.faces.len(), 88);
}

#[test]
fn multiple_cylinders_sample_loads_six_leaves_sharing_one_resource() {
    let loaded = load_official("multiple_cylinders.3mf");
    let scene = &loaded.graph;
    let children = root_children(scene);
    assert_eq!(children.len(), 6);
    let resources: Vec<MeshResourceId> = children
        .iter()
        .map(|&child| leaf_resource(scene, child))
        .collect();
    assert!(
        resources.iter().all(|&rid| rid == resources[0]),
        "all 6 placements share one mesh resource",
    );
    assert_eq!(scene.mesh_resources_iter().count(), 1);
    let transformed = children
        .iter()
        .filter(|&&child| {
            !scene
                .node(child)
                .expect("leaf lives")
                .local_transform
                .is_identity()
        })
        .count();
    assert_eq!(transformed, 5, "5 of the 6 placements carry a transform");
    // The file is in millimetres, so the second placement's stored
    // x translation of 21 arrives unscaled.
    match scene.node(children[1]).expect("leaf lives").local_transform {
        Transform3D::Affine(a) => assert_eq!(a.translation()[0], 21.0),
        Transform3D::Identity => panic!("second placement should carry a transform"),
    }
}

#[test]
fn torus_sample_loads_with_known_counts() {
    let loaded = load_official("torus.3mf");
    let mesh = single_leaf_mesh(&loaded.graph);
    assert_eq!(mesh.vertices.len(), 1100);
    assert_eq!(mesh.faces.len(), 2200);
}

#[test]
fn box_sliced_sample_loads_its_fullres_mesh_and_reports_dropped_slices() {
    let loaded = load_official("box_sliced.3mf");
    assert_eq!(
        loaded.warnings,
        vec![LoadMeshWarning::ThreeMf(
            LoadThreeMfWarning::SliceDataIgnored { stack_count: 1 },
        )],
    );
    let mesh = single_leaf_mesh(&loaded.graph);
    assert_eq!(mesh.vertices.len(), 8);
    assert_eq!(mesh.faces.len(), 12);
}

// ---------------------------------------------------------------------
// Generated multi-object scene: the workspace's cube and tunnel-prism
// fixtures written through sc-3mf and read back through the loader.
// ---------------------------------------------------------------------

/// Millimetre translation of each prism placement, in build order:
/// cube centred, Z tunnel to the right, Y tunnel to the left.
const PRISM_PLACEMENTS: [[f64; 3]; 3] = [[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [-5.0, 0.0, 0.0]];

fn to_mesh_data(mesh: &IndexedMesh) -> MeshData {
    MeshData {
        vertices: mesh
            .vertices
            .iter()
            .map(|v| Vertex3 {
                x: f64::from(v[0]),
                y: f64::from(v[1]),
                z: f64::from(v[2]),
            })
            .collect(),
        triangles: mesh
            .faces
            .iter()
            .map(|f| TriangleRef {
                v1: ResourceIndex::new(f.vertices[0].get()).expect("small index"),
                v2: ResourceIndex::new(f.vertices[1].get()).expect("small index"),
                v3: ResourceIndex::new(f.vertices[2].get()).expect("small index"),
            })
            .collect(),
    }
}

fn translation_matrix(t: [f64; 3]) -> Matrix3D {
    Matrix3D([
        1.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, //
        0.0, 0.0, 1.0, //
        t[0], t[1], t[2],
    ])
}

fn prism_scene_document(unit: Unit) -> ModelDocument {
    let meshes = [
        ("Cube", unit_cube_mesh()),
        ("ZTunnel", cuboid_with_z_tunnel()),
        ("YTunnel", cuboid_with_y_tunnel()),
    ];
    let mut document = ModelDocument {
        unit,
        ..ModelDocument::default()
    };
    for (index, (name, mesh)) in meshes.iter().enumerate() {
        let id = ResourceId::new(index as u32 + 1).expect("small id");
        document.objects.push(ObjectResource {
            id,
            object_type: ObjectType::Model,
            name: Some((*name).to_string()),
            partnumber: None,
            property_ref: None,
            slice_stack: None,
            mesh_resolution: MeshResolution::default(),
            payload: ObjectPayload::Mesh(to_mesh_data(mesh)),
        });
        document.build.push(BuildItem {
            object_id: id,
            transform: Some(translation_matrix(PRISM_PLACEMENTS[index])),
            partnumber: None,
        });
    }
    document
}

fn package_cursor(document: &ModelDocument) -> Cursor<Vec<u8>> {
    let mut model_xml = Vec::new();
    write_model(document, &mut model_xml).expect("document serializes");
    let mut package = Package::new();
    let part = PartName::new("/3D/3dmodel.model").expect("valid part name");
    package
        .add_part(part, MODEL_PART_CONTENT_TYPE, model_xml)
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

#[test]
fn prism_trio_round_trips_geometry_names_and_placements() {
    let document = prism_scene_document(Unit::Millimeter);
    let mut cursor = package_cursor(&document);
    let loaded =
        load_mesh_from_reader(MeshFileType::_3mf, &mut cursor).expect("generated package loads");
    // The 2 tunnel prisms share their outer-box corner coordinates,
    // so the loader's scene-level observation fires exactly as it
    // does for OBJ input; no 3MF-specific warnings appear.
    assert_eq!(loaded.warnings.len(), 1, "got {:?}", loaded.warnings);
    match &loaded.warnings[0] {
        LoadMeshWarning::Scene(SceneLoadWarning::SharedVertices { leaves }) => {
            let names: Vec<_> = leaves
                .iter()
                .map(|leaf| leaf.object_name.as_deref())
                .collect();
            assert_eq!(names, vec![Some("ZTunnel"), Some("YTunnel")]);
        }
        other => panic!("expected a shared-vertices scene warning, got {other:?}"),
    }

    let scene = &loaded.graph;
    let children = root_children(scene);
    assert_eq!(children.len(), 3);
    assert_eq!(scene.mesh_resources_iter().count(), 3);

    let expected = [
        ("Cube", unit_cube_mesh()),
        ("ZTunnel", cuboid_with_z_tunnel()),
        ("YTunnel", cuboid_with_y_tunnel()),
    ];
    for (index, &child) in children.iter().enumerate() {
        let mesh = scene
            .mesh_resource(leaf_resource(scene, child))
            .expect("resource lives");
        let (name, source) = &expected[index];
        assert_eq!(
            mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
            Some(*name),
        );
        // f32 -> f64 widening is exact and the writer emits shortest
        // round-trip decimals, so the coordinates come back bitwise
        // identical in millimetre units.
        assert_eq!(mesh.vertices, source.vertices, "vertices of {name}");
        let indices: Vec<_> = mesh.faces.iter().map(|f| f.vertices).collect();
        let source_indices: Vec<_> = source.faces.iter().map(|f| f.vertices).collect();
        assert_eq!(indices, source_indices, "triangles of {name}");

        let transform = scene.node(child).expect("leaf lives").local_transform;
        let placement = PRISM_PLACEMENTS[index];
        if placement == [0.0, 0.0, 0.0] {
            assert!(transform.is_identity());
        } else {
            match transform {
                Transform3D::Affine(a) => assert_eq!(a.translation(), &placement),
                Transform3D::Identity => panic!("expected a translated placement"),
            }
        }
    }
}

#[test]
fn prism_trio_in_centimeters_scales_to_millimetres() {
    let document = prism_scene_document(Unit::Centimeter);
    let mut cursor = package_cursor(&document);
    let loaded =
        load_mesh_from_reader(MeshFileType::_3mf, &mut cursor).expect("generated package loads");
    let scene = &loaded.graph;
    let children = root_children(scene);

    let cube = scene
        .mesh_resource(leaf_resource(scene, children[0]))
        .expect("resource lives");
    let source = unit_cube_mesh();
    for (loaded_vertex, source_vertex) in cube.vertices.iter().zip(&source.vertices) {
        assert_eq!(
            *loaded_vertex,
            Vertex::new([
                source_vertex[0] * 10.0,
                source_vertex[1] * 10.0,
                source_vertex[2] * 10.0,
            ]),
        );
    }
    match scene.node(children[1]).expect("leaf lives").local_transform {
        Transform3D::Affine(a) => assert_eq!(a.translation(), &[50.0, 0.0, 0.0]),
        Transform3D::Identity => panic!("expected a translated placement"),
    }
}
