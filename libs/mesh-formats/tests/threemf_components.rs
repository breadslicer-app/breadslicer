/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end coverage over the vendored consortium components test
//! case (`P_XXX_0910_02.3mf`): one mesh object referenced 5 times
//! through the components of a single build object. The file must
//! load with all 5 shared-geometry occurrences and re-export through
//! every write format: expanded for the flattening formats, sharing
//! preserved for 3MF. Permanent capability pins; a refusal or
//! reference-skipping walk on any of these paths must turn them red.

use std::path::PathBuf;

use sc_3mf::model::ObjectPayload;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_formats::filetypes::WriteFormat;
use sc_mesh_formats::load_mesh_from_path;
use sc_mesh_formats::messages::{LoadMeshWarning, SceneLoadWarning};
use sc_mesh_formats::scene_writer::{FlattenWarning, write_scene};
use sc_mesh_formats::threemf::scene_to_model_document;
use sc_scene::node::{LeafPayload, MeshResourceId, NodeId, NodeKind};
use sc_scene::{CsgPolicy, Scene};

/// Ground truth for `P_XXX_0910_02.3mf`, read off the vendored file:
/// 5 component references to one mesh object of 3664 vertices and
/// 7384 triangles, under one translated build item.
const OCCURRENCES: usize = 5;
const VERTICES_PER_COPY: usize = 3664;
const FACES_PER_COPY: usize = 7384;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/official/3mf-test-suites")
        .join("P_XXX_0910_02.3mf")
}

fn load_fixture() -> Scene {
    let loaded = load_mesh_from_path(&fixture_path(), None, None).expect("fixture loads");
    // The 5 occurrences share one mesh resource, which the loader
    // reports through its shared-vertices notice; nothing else may
    // surface.
    assert_eq!(loaded.warnings.len(), 1, "warnings: {:?}", loaded.warnings);
    match &loaded.warnings[0] {
        LoadMeshWarning::Scene(SceneLoadWarning::SharedVertices { leaves }) => {
            assert_eq!(leaves.len(), OCCURRENCES);
        }
        other => panic!("expected the shared-vertices notice, got {other:?}"),
    }
    loaded.graph
}

fn root_children(scene: &Scene) -> Vec<NodeId> {
    assert_eq!(scene.roots().len(), 1, "expected one scene root");
    match &scene.node(scene.roots()[0]).expect("root lives").kind {
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

#[test]
fn the_fixture_loads_as_five_shared_occurrences() {
    let graph = load_fixture();
    assert_eq!(
        graph
            .expanded_leaf_count(CsgPolicy::Refuse)
            .expect("fixture scene is acyclic"),
        OCCURRENCES
    );

    // Loader shape: the build item's component object becomes one
    // group whose 5 leaves share a single mesh resource.
    let children = root_children(&graph);
    assert_eq!(children.len(), 1);
    let item = graph.node(children[0]).expect("item node lives");
    let leaves = match &item.kind {
        NodeKind::Group { children } => children.clone(),
        other => panic!("expected the component object as a Group, got {other:?}"),
    };
    assert_eq!(leaves.len(), OCCURRENCES);
    let first_rid = leaf_resource(&graph, leaves[0]);
    for &leaf in &leaves {
        assert_eq!(
            leaf_resource(&graph, leaf),
            first_rid,
            "all occurrences share one mesh resource"
        );
    }
    let mesh = graph.mesh_resource(first_rid).expect("resource lives");
    assert_eq!(mesh.vertices().len(), VERTICES_PER_COPY);
    assert_eq!(mesh.faces().len(), FACES_PER_COPY);

    // First occurrence: the item translation composed with the first
    // component translation lands at (33.8, 30.25, 50.1).
    let leaf0 = graph.node(leaves[0]).expect("leaf lives");
    let world = item.local_transform.compose(&leaf0.local_transform);
    let Transform3D::Affine(affine) = world else {
        panic!("expected a translated occurrence");
    };
    let translation = affine.translation();
    let expected = [33.8, 30.25, 50.1];
    for axis in 0..3 {
        assert!(
            (translation[axis] - expected[axis]).abs() < 1e-6,
            "axis {axis}: {} vs {}",
            translation[axis],
            expected[axis]
        );
    }
}

#[test]
fn the_fixture_merges_to_the_full_expanded_geometry() {
    let graph = load_fixture();
    let merged = graph
        .merge_meshes()
        .expect("merge succeeds")
        .expect("geometry present");
    assert_eq!(merged.faces.len(), OCCURRENCES * FACES_PER_COPY);

    // Expanded axis-aligned bounds, computed from the fixture's
    // transforms and vertices; f32 baking allows a small tolerance.
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in merged.vertices() {
        for axis in 0..3 {
            min[axis] = min[axis].min(v[axis]);
            max[axis] = max[axis].max(v[axis]);
        }
    }
    let expected_min = [33.8, 30.25, 50.1];
    let expected_max = [181.46, 177.893, 157.134];
    for axis in 0..3 {
        assert!(
            (min[axis] - expected_min[axis]).abs() < 0.02,
            "min axis {axis}: {} vs {}",
            min[axis],
            expected_min[axis]
        );
        assert!(
            (max[axis] - expected_max[axis]).abs() < 0.02,
            "max axis {axis}: {} vs {}",
            max[axis],
            expected_max[axis]
        );
    }
}

#[test]
fn the_fixture_rewrites_to_threemf_preserving_the_sharing() {
    let graph = load_fixture();

    // Document structure: the geometry serialises once, referenced 5
    // times from one components object, built by one item.
    let document = scene_to_model_document(&graph).expect("document builds");
    let mesh_object_ids: Vec<_> = document
        .objects
        .iter()
        .filter(|o| matches!(o.payload, ObjectPayload::Mesh(_)))
        .map(|o| o.id)
        .collect();
    assert_eq!(mesh_object_ids.len(), 1, "shared geometry serialises once");
    let components = document
        .objects
        .iter()
        .find_map(|o| match &o.payload {
            ObjectPayload::Components(components) => Some(components),
            ObjectPayload::Mesh(_) => None,
        })
        .expect("the shared occurrences become a components object");
    assert_eq!(components.len(), OCCURRENCES);
    for component in components {
        assert_eq!(component.object_id, mesh_object_ids[0]);
    }
    assert_eq!(document.build.len(), 1);

    // The written package loads back to the same expansion.
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("components-rewritten.3mf");
    write_scene(&graph, &target, WriteFormat::ThreeMf).expect("rewrite succeeds");
    let second = load_mesh_from_path(&target, None, None).expect("rewritten file loads");
    assert_eq!(
        second
            .graph
            .expanded_leaf_count(CsgPolicy::Refuse)
            .expect("rewritten scene is acyclic"),
        OCCURRENCES
    );
    let merged = second
        .graph
        .merge_meshes()
        .expect("merge succeeds")
        .expect("geometry present");
    assert_eq!(merged.faces.len(), OCCURRENCES * FACES_PER_COPY);
}

#[test]
fn the_fixture_exports_expanded_to_the_flattening_formats() {
    let graph = load_fixture();
    let dir = tempfile::tempdir().expect("temp dir");

    let stl = dir.path().join("components.stl");
    let warnings = write_scene(&graph, &stl, WriteFormat::StlBin).expect("stl writes");
    assert_eq!(
        warnings,
        vec![FlattenWarning::MultiObjectMerge {
            object_count: OCCURRENCES
        }]
    );
    // Binary STL layout: 80-byte header + 4-byte count + n * 50.
    let bytes = std::fs::metadata(&stl).expect("stl written").len();
    assert_eq!(bytes as usize, 84 + OCCURRENCES * FACES_PER_COPY * 50);

    let obj = dir.path().join("components.obj");
    write_scene(&graph, &obj, WriteFormat::Obj).expect("obj writes");
    let reloaded = load_mesh_from_path(&obj, None, None).expect("obj loads back");
    assert_eq!(reloaded.graph.reachable_leaf_count(), OCCURRENCES);
    let merged = reloaded
        .graph
        .merge_meshes()
        .expect("merge succeeds")
        .expect("geometry present");
    assert_eq!(merged.faces.len(), OCCURRENCES * FACES_PER_COPY);

    let stla = dir.path().join("components.stla");
    write_scene(&graph, &stla, WriteFormat::StlAscii).expect("ascii stl writes");
    let text = std::fs::read_to_string(&stla).expect("ascii stl written");
    assert!(text.starts_with("solid"));
    assert_eq!(
        text.matches("facet normal").count(),
        OCCURRENCES * FACES_PER_COPY
    );
}
