/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Cross-format round trips for hostile mesh names, over the cube
//! and tunnel-prism fixtures: 3MF preserves names verbatim (UTF-8 is
//! part of the format), while the line-oriented writers (OBJ, both
//! STL flavours) emit them folded to printable ASCII and their own
//! readers accept the result with identical geometry.

use std::io::Cursor;
use std::sync::Arc;

use sc_mesh_core::scene::node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, MeshMetadata, Scene};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_mesh_formats::obj::{read_obj, write_obj};
use sc_mesh_formats::stl::{read_stla, read_stlb, write_stla, write_stlb};
use sc_mesh_formats::threemf::{read_threemf, write_threemf};

/// The trio's hostile names and their folded forms: a
/// directive-injection attempt, a Han name, and an accented name.
const HOSTILE_TRIO: [(&str, &str); 3] = [
    ("Injected\nv 9 9 9", "Injected v 9 9 9"),
    ("深圳", "ShenZhen"),
    ("Küche", "Kuche"),
];

fn named(mesh: IndexedMesh, name: &str) -> IndexedMesh {
    let mut mesh = mesh;
    mesh.set_meta(Some(MeshMetadata {
        name: Some(name.to_string()),
        header_comment: None,
    }));
    mesh
}

fn fixture_meshes() -> [IndexedMesh; 3] {
    [
        unit_cube_mesh(),
        cuboid_with_z_tunnel(),
        cuboid_with_y_tunnel(),
    ]
}

/// The trio as one scene: cube and both tunnel prisms as leaves
/// under a single group root, each carrying its hostile name.
/// Identity transforms throughout, so baked (OBJ) and native (3MF)
/// geometry compare against the same source vertices.
fn hostile_trio_scene() -> Scene {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    for ((hostile, _), mesh) in HOSTILE_TRIO.iter().zip(fixture_meshes()) {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(named(mesh, hostile))),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
    }
    g
}

fn leaf_ids_in_order(scene: &Scene) -> Vec<NodeId> {
    let mut ids = Vec::new();
    let mut stack: Vec<NodeId> = scene.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        match &scene.node(id).expect("node lives").kind {
            NodeKind::Leaf(_) => ids.push(id),
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for c in children.iter().rev() {
                    stack.push(*c);
                }
            }
            NodeKind::Reference { .. } => {}
        }
    }
    ids
}

fn leaf_name(scene: &Scene, id: NodeId) -> Option<String> {
    scene
        .mesh_at_leaf(id)
        .expect("leaf resolves to a mesh")
        .meta()
        .as_ref()
        .and_then(|m| m.name.clone())
}

/// Each face as its 3 vertex positions, in face order: the geometry
/// comparison that survives the vertex re-indexing STL's soup
/// dedupe performs.
fn face_triples(mesh: &IndexedMesh) -> Vec<[[f32; 3]; 3]> {
    mesh.faces()
        .iter()
        .map(|face| {
            face.vertices
                .map(|vi| mesh.vertices()[usize::from(vi)].coords())
        })
        .collect()
}

#[test]
fn hostile_names_survive_3mf_verbatim() {
    let source = hostile_trio_scene();
    let mut buf: Vec<u8> = Vec::new();
    write_threemf(&source, &mut buf).expect("write_threemf succeeds");

    let (scene, warnings) = read_threemf(&mut Cursor::new(buf)).expect("written 3MF loads");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    let leaves = leaf_ids_in_order(&scene);
    assert_eq!(leaves.len(), 3);
    for ((hostile, _), (leaf, source_mesh)) in
        HOSTILE_TRIO.iter().zip(leaves.iter().zip(fixture_meshes()))
    {
        assert_eq!(leaf_name(&scene, *leaf).as_deref(), Some(*hostile));
        let mesh = scene.mesh_at_leaf(*leaf).expect("leaf resolves to a mesh");
        assert_eq!(face_triples(mesh), face_triples(&source_mesh));
    }
}

#[test]
fn hostile_names_fold_in_the_obj_round_trip() {
    let source = hostile_trio_scene();
    let mut buf: Vec<u8> = Vec::new();
    write_obj(&source, &mut buf).expect("write_obj succeeds");

    let (scene, warnings) = read_obj(&mut Cursor::new(buf)).expect("written OBJ loads");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    let leaves = leaf_ids_in_order(&scene);
    assert_eq!(leaves.len(), 3);
    for ((_, folded), (leaf, source_mesh)) in
        HOSTILE_TRIO.iter().zip(leaves.iter().zip(fixture_meshes()))
    {
        assert_eq!(leaf_name(&scene, *leaf).as_deref(), Some(*folded));
        let mesh = scene.mesh_at_leaf(*leaf).expect("leaf resolves to a mesh");
        assert_eq!(face_triples(mesh), face_triples(&source_mesh));
    }
}

#[test]
fn hostile_names_fold_in_the_ascii_stl_round_trip() {
    for ((hostile, folded), source_mesh) in HOSTILE_TRIO.iter().zip(fixture_meshes()) {
        let mesh = named(source_mesh.clone(), hostile);
        let mut buf: Vec<u8> = Vec::new();
        write_stla(&mesh, &mut buf).expect("write_stla succeeds");

        let scene = read_stla(&mut Cursor::new(buf)).expect("written ASCII STL loads");
        let leaves = leaf_ids_in_order(&scene);
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaf_name(&scene, leaves[0]).as_deref(), Some(*folded));
        let loaded = scene
            .mesh_at_leaf(leaves[0])
            .expect("leaf resolves to a mesh");
        assert_eq!(face_triples(loaded), face_triples(&source_mesh));
    }
}

#[test]
fn hostile_names_fold_in_the_binary_stl_round_trip() {
    for ((hostile, folded), source_mesh) in HOSTILE_TRIO.iter().zip(fixture_meshes()) {
        let mesh = named(source_mesh.clone(), hostile);
        let mut buf: Vec<u8> = Vec::new();
        write_stlb(&mesh, &mut buf).expect("write_stlb succeeds");

        let (scene, warnings) = read_stlb(&mut Cursor::new(buf)).expect("written binary STL loads");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        let leaves = leaf_ids_in_order(&scene);
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaf_name(&scene, leaves[0]).as_deref(), Some(*folded));
        let loaded = scene
            .mesh_at_leaf(leaves[0])
            .expect("leaf resolves to a mesh");
        assert_eq!(face_triples(loaded), face_triples(&source_mesh));
    }
}
