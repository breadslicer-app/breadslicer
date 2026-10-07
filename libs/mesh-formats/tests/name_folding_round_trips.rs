/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Cross-format round trips for hostile mesh names, over the cube and
//! tunnel-prism fixtures: 3MF keeps the full Unicode charset but cleans names
//! of structural junk (control characters, noncharacters, whitespace runs),
//! while the line-oriented writers (OBJ, both STL flavours) emit them folded
//! all the way to printable ASCII. Every writer's own reader accepts the result
//! with identical geometry.

use std::io::Cursor;

use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, MeshMetadata};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_mesh_formats::obj::{read_obj, write_obj};
use sc_mesh_formats::stl::{read_stla, read_stlb, write_stla, write_stlb};
use sc_mesh_formats::threemf::{read_threemf, write_threemf};
use sc_scene::Scene;
use sc_scene::node::{NodeId, NodeKind};
use sc_scene_fixtures::leaves_under_group_root;

/// The trio's hostile names with the cleaned (3MF) and folded (line-format)
/// forms: a directive-injection attempt, a Han name, and an accented name.
const HOSTILE_TRIO: [(&str, &str, &str); 3] = [
    ("Injected\nv 9 9 9", "Injected v 9 9 9", "Injected v 9 9 9"),
    ("深圳", "深圳", "ShenZhen"),
    ("Küche", "Küche", "Kuche"),
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

/// Builds the trio as 1 scene: the cube and both tunnel prisms as leaves under
/// 1 group root, each with its hostile name. Every transform is the identity,
/// so the baked (OBJ) and native (3MF) geometry compare against the same source
/// vertices.
fn hostile_trio_scene() -> Scene {
    leaves_under_group_root(
        HOSTILE_TRIO
            .iter()
            .zip(fixture_meshes())
            .map(|((hostile, _, _), mesh)| (named(mesh, hostile), Transform3D::Identity)),
    )
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

/// Returns each face as its 3 vertex positions, in face order: a comparison of
/// geometry that survives the vertex re-indexing of the STL dedupe.
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
fn hostile_names_keep_their_charset_but_lose_structural_junk_in_3mf() {
    // 3MF names stay Unicode, with control characters and whitespace runs
    // cleaned as everywhere else: the newline in the injection name collapses,
    // and the Han and accented names pass as they are.
    let source = hostile_trio_scene();
    let mut buf: Vec<u8> = Vec::new();
    write_threemf(&source, &mut buf).expect("write_threemf succeeds");

    let (scene, warnings) = read_threemf(&mut Cursor::new(buf)).expect("written 3MF loads");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    let leaves = leaf_ids_in_order(&scene);
    assert_eq!(leaves.len(), 3);
    for ((_, cleaned, _), (leaf, source_mesh)) in
        HOSTILE_TRIO.iter().zip(leaves.iter().zip(fixture_meshes()))
    {
        assert_eq!(leaf_name(&scene, *leaf).as_deref(), Some(*cleaned));
        let mesh = scene.mesh_at_leaf(*leaf).expect("leaf resolves to a mesh");
        assert_eq!(face_triples(mesh), face_triples(&source_mesh));
    }
}

/// Checks names that the cleaning reduces further than the trio's: each is
/// written through 3MF and read back as its cleaned form, or with no name when
/// nothing is left.
#[test]
fn control_and_noncharacter_names_are_cleaned_in_3mf() {
    let cases: [(&str, Option<&str>); 5] = [
        ("a\u{7}b", Some("ab")),
        ("a\0b", Some("ab")),
        ("a\u{FFFF}b", Some("ab")),
        ("  x ", Some("x")),
        ("\u{7}\u{FDD0}", None),
    ];
    for (hostile, expected) in cases {
        let g = Scene::from(named(unit_cube_mesh(), hostile));

        let mut buf: Vec<u8> = Vec::new();
        write_threemf(&g, &mut buf).expect("write_threemf succeeds");
        let (scene, warnings) = read_threemf(&mut Cursor::new(buf)).expect("written 3MF loads");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        let leaves = leaf_ids_in_order(&scene);
        assert_eq!(leaves.len(), 1);
        assert_eq!(
            leaf_name(&scene, leaves[0]).as_deref(),
            expected,
            "cleaned form of {hostile:?}",
        );
    }
}

#[test]
fn emitted_model_xml_carries_no_xml_illegal_bytes() {
    // XML 1.0 forbids most C0 controls outright, escaped or not; a conformant
    // parser refuses the whole document over 1 such byte. The scene name (Title
    // metadata) is covered too.
    let mut source = hostile_trio_scene();
    source.set_meta(Some(sc_scene::SceneMetadata {
        name: Some("Ti\u{7}tle\u{FFFF}".to_string()),
        comment: None,
    }));
    let mut buf: Vec<u8> = Vec::new();
    write_threemf(&source, &mut buf).expect("write_threemf succeeds");

    let package = sc_3mf::package::Package::read_from(Cursor::new(buf.clone()))
        .expect("written package parses");
    let model_name = package
        .part_names()
        .find(|name| name.as_str().ends_with(".model"))
        .expect("package holds the model part")
        .clone();
    let illegal = package
        .part_data(&model_name)
        .expect("model part has data")
        .iter()
        .filter(|&&b| b < 0x20 && b != 0x09 && b != 0x0A && b != 0x0D)
        .count();
    assert_eq!(illegal, 0, "model part must stay well-formed XML");

    let (scene, _) = read_threemf(&mut Cursor::new(buf)).expect("written 3MF loads");
    assert_eq!(
        scene.meta().as_ref().and_then(|m| m.name.as_deref()),
        Some("Title"),
    );
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
    for ((_, _, folded), (leaf, source_mesh)) in
        HOSTILE_TRIO.iter().zip(leaves.iter().zip(fixture_meshes()))
    {
        assert_eq!(leaf_name(&scene, *leaf).as_deref(), Some(*folded));
        let mesh = scene.mesh_at_leaf(*leaf).expect("leaf resolves to a mesh");
        assert_eq!(face_triples(mesh), face_triples(&source_mesh));
    }
}

#[test]
fn hostile_names_fold_in_the_ascii_stl_round_trip() {
    for ((hostile, _, folded), source_mesh) in HOSTILE_TRIO.iter().zip(fixture_meshes()) {
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
    for ((hostile, _, folded), source_mesh) in HOSTILE_TRIO.iter().zip(fixture_meshes()) {
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
