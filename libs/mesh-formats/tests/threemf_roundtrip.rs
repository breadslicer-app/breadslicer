/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Write-then-load round trips for 3MF through the public
//! `write_scene` / `load_mesh_from_path` entry points, using the
//! workspace's cube and tunnel-prism fixtures and a vendored official
//! sample as sources.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use sc_3mf::package::Package;
use sc_mesh_core::scene::node::{LeafPayload, MeshResourceId, NodeId, NodeKind};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, MeshMetadata, Scene};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_mesh_formats::filetypes::WriteFormat;
use sc_mesh_formats::scene_writer::{InstanceFreeScene, write_scene};
use sc_mesh_formats::threemf::{read_threemf, read_threemf_from_package, write_threemf};
use sc_mesh_formats::{LoadedMesh, load_mesh_from_path};

fn named(mesh: IndexedMesh, name: &str) -> IndexedMesh {
    let mut mesh = mesh;
    mesh.set_meta(Some(MeshMetadata {
        name: Some(name.to_string()),
        header_comment: None,
    }));
    mesh
}

/// Loader-shape scene carrying the cube and both tunnel prisms as
/// named leaves with distinct translations.
fn prism_trio_scene() -> Scene {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    let sources = [
        ("Cube", unit_cube_mesh(), [0.0, 0.0, 0.0]),
        ("ZTunnel", cuboid_with_z_tunnel(), [5.0, 0.0, 0.0]),
        ("YTunnel", cuboid_with_y_tunnel(), [-5.0, 0.0, 0.0]),
    ];
    for (name, mesh, translation) in sources {
        let transform = if translation == [0.0, 0.0, 0.0] {
            Transform3D::Identity
        } else {
            Transform3D::Affine(AffineTransform3D::from_translation(translation))
        };
        let leaf = g
            .add_leaf(
                None,
                transform,
                LeafPayload::Mesh(Arc::new(named(mesh, name))),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
    }
    g
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
fn prism_trio_survives_a_write_load_cycle() {
    let source_scene = prism_trio_scene();
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("trio.3mf");

    let source = InstanceFreeScene::try_new(&source_scene).expect("fixture has no instances");
    let warnings = write_scene(source, &target, WriteFormat::ThreeMf).expect("write succeeds");
    assert!(warnings.is_empty(), "3MF write never warns: {warnings:?}");

    let loaded = load_mesh_from_path(&target, None, None).expect("written file loads");
    let scene = &loaded.graph;
    let children = root_children(scene);
    assert_eq!(children.len(), 3);
    assert_eq!(scene.mesh_resources_iter().count(), 3);

    let expected = [
        ("Cube", unit_cube_mesh(), [0.0, 0.0, 0.0]),
        ("ZTunnel", cuboid_with_z_tunnel(), [5.0, 0.0, 0.0]),
        ("YTunnel", cuboid_with_y_tunnel(), [-5.0, 0.0, 0.0]),
    ];
    for (index, &child) in children.iter().enumerate() {
        let (name, source_mesh, translation) = &expected[index];
        let mesh = scene
            .mesh_resource(leaf_resource(scene, child))
            .expect("resource lives");
        assert_eq!(
            mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
            Some(*name),
        );
        // Millimetre coordinates round-trip bitwise: f32 -> f64
        // widening is exact and the XML carries shortest
        // round-trip decimals.
        assert_eq!(mesh.vertices, source_mesh.vertices, "vertices of {name}");
        let indices: Vec<_> = mesh.faces.iter().map(|f| f.vertices).collect();
        let source_indices: Vec<_> = source_mesh.faces.iter().map(|f| f.vertices).collect();
        assert_eq!(indices, source_indices, "triangles of {name}");

        let transform = scene.node(child).expect("leaf lives").local_transform;
        if *translation == [0.0, 0.0, 0.0] {
            assert!(transform.is_identity());
        } else {
            match transform {
                Transform3D::Affine(a) => assert_eq!(a.translation(), translation),
                Transform3D::Identity => panic!("expected a translated placement"),
            }
        }
    }
}

#[test]
fn official_box_sample_survives_a_load_write_load_cycle() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/official")
        .join("box.3mf");
    let first: LoadedMesh = load_mesh_from_path(&source, None, None).expect("sample loads");

    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("box-rewritten.3mf");
    let rewrite = InstanceFreeScene::try_new(&first.graph).expect("sample has no instances");
    write_scene(rewrite, &target, WriteFormat::ThreeMf).expect("rewrite succeeds");

    let second = load_mesh_from_path(&target, None, None).expect("rewritten file loads");
    let children = root_children(&second.graph);
    assert_eq!(children.len(), 1);
    let mesh = second
        .graph
        .mesh_resource(leaf_resource(&second.graph, children[0]))
        .expect("resource lives");
    assert_eq!(mesh.vertices.len(), 8);
    assert_eq!(mesh.faces.len(), 12);

    let first_children = root_children(&first.graph);
    let first_mesh = first
        .graph
        .mesh_resource(leaf_resource(&first.graph, first_children[0]))
        .expect("resource lives");
    assert_eq!(mesh.vertices, first_mesh.vertices);
}

#[test]
fn the_package_level_reader_matches_the_stream_reader() {
    let mut bytes = Vec::new();
    write_threemf(&prism_trio_scene(), &mut bytes).expect("write succeeds");

    let package = Package::read_from(Cursor::new(bytes.clone())).expect("package parses");
    let (package_scene, package_warnings) =
        read_threemf_from_package(&package).expect("package-level read succeeds");
    let (stream_scene, stream_warnings) =
        read_threemf(&mut Cursor::new(bytes)).expect("stream-level read succeeds");

    assert_eq!(package_warnings, stream_warnings);
    let package_children = root_children(&package_scene);
    let stream_children = root_children(&stream_scene);
    assert_eq!(package_children.len(), stream_children.len());
    for (&from_package, &from_stream) in package_children.iter().zip(&stream_children) {
        let package_mesh = package_scene
            .mesh_resource(leaf_resource(&package_scene, from_package))
            .expect("resource lives");
        let stream_mesh = stream_scene
            .mesh_resource(leaf_resource(&stream_scene, from_stream))
            .expect("resource lives");
        assert_eq!(package_mesh.vertices, stream_mesh.vertices);
        assert_eq!(
            package_mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
            stream_mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
        );
    }
}
