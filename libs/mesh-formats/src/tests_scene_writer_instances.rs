/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Capability pins for scene writing over instance-bearing scenes:
//! [`flatten_graph_to_mesh`] and [`write_scene`] expand instances,
//! so every occurrence of an instanced subtree reaches the written
//! file under its composed call-site transform. These pins are
//! permanent: a refusal, witness, or instance-skipping walk on these
//! paths must turn them red, never ship green.
//!
//! The 3MF branch is the one exception, pinned separately below as
//! an interim typed error until the writer maps instances to native
//! 3MF components; that pin exists only to keep the gap loud, and
//! it is replaced by an export-correctly pin, never by silence.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{
    CsgPolicy, IndexedMesh, IndexedTriangle, LeafPayload, MAX_EXPANDED_LEAF_COUNT, MeshMetadata,
    NodeId, Normal, Scene, SubtreeResourceId, Vertex,
};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};

use crate::error::{WriteObjError, WriteSceneError, WriteThreeMfError};
use crate::filetypes::WriteFormat;
use crate::scene_writer::{FlattenWarning, flatten_graph_to_mesh, write_scene};
use crate::stl::read_stlb;

fn unit_triangle(name: Option<&str>) -> IndexedMesh {
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

/// Registers a subtree resource holding one triangle leaf under an
/// identity group and returns its id.
fn register_triangle_subtree(scene: &mut Scene, name: Option<&str>) -> SubtreeResourceId {
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_triangle(name))),
        )
        .expect("add subtree leaf");
    let root = scene.add_group(None, Transform3D::Identity);
    scene.add_child(root, leaf).expect("attach subtree leaf");
    scene
        .register_subtree_resource(root)
        .expect("unparented group registers")
}

fn add_instance_at(scene: &mut Scene, parent: NodeId, rid: SubtreeResourceId, t: [f64; 3]) {
    let inst = scene
        .add_instance(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(t)),
            rid,
        )
        .expect("registered resource resolves");
    scene.add_child(parent, inst).expect("attach instance");
}

/// Root group holding `plain_leaves` triangle leaves (at x = 0, 2,
/// 4, ...) plus one instance of a registered single-triangle subtree
/// at x = 10.
fn scene_with_instance(plain_leaves: usize) -> Scene {
    let mut scene = Scene::new();
    let rid = register_triangle_subtree(&mut scene, None);
    let root = scene.add_group(None, Transform3D::Identity);
    for i in 0..plain_leaves {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([
                    2.0 * i as f64,
                    0.0,
                    0.0,
                ])),
                LeafPayload::Mesh(Arc::new(unit_triangle(None))),
            )
            .expect("leaf construction succeeds");
        scene.add_child(root, leaf).expect("attach succeeds");
    }
    add_instance_at(&mut scene, root, rid, [10.0, 0.0, 0.0]);
    scene.add_root(root).expect("root registers");
    scene
}

fn has_vertex(mesh: &IndexedMesh, coords: [f32; 3]) -> bool {
    mesh.vertices().iter().any(|v| v.coords() == coords)
}

// --- flatten dispatch shapes -----------------------------------------
//
// The 3 shapes below used to take 3 different silent paths through
// the dispatch (a false empty-scene error, a silent drop of the
// instanced geometry, and a correct merge). Each now pins its exact
// correct output.

#[test]
fn flatten_expands_a_lone_instance() {
    let scene = scene_with_instance(0);
    let flattened = flatten_graph_to_mesh(&scene).expect("flatten succeeds");
    assert_eq!(flattened.mesh.faces.len(), 1);
    assert!(has_vertex(&flattened.mesh, [11.0, 0.0, 0.0]));
    assert!(
        flattened.warnings.is_empty(),
        "a single occurrence merges nothing"
    );
}

#[test]
fn flatten_merges_one_plain_leaf_with_the_instance() {
    let scene = scene_with_instance(1);
    let flattened = flatten_graph_to_mesh(&scene).expect("flatten succeeds");
    assert_eq!(flattened.mesh.faces.len(), 2);
    assert!(has_vertex(&flattened.mesh, [1.0, 0.0, 0.0]));
    assert!(has_vertex(&flattened.mesh, [11.0, 0.0, 0.0]));
    assert_eq!(
        flattened.warnings,
        vec![FlattenWarning::MultiObjectMerge { object_count: 2 }],
    );
}

#[test]
fn flatten_merges_two_plain_leaves_with_the_instance() {
    let scene = scene_with_instance(2);
    let flattened = flatten_graph_to_mesh(&scene).expect("flatten succeeds");
    assert_eq!(flattened.mesh.faces.len(), 3);
    assert!(has_vertex(&flattened.mesh, [1.0, 0.0, 0.0]));
    assert!(has_vertex(&flattened.mesh, [3.0, 0.0, 0.0]));
    assert!(has_vertex(&flattened.mesh, [11.0, 0.0, 0.0]));
    assert_eq!(
        flattened.warnings,
        vec![FlattenWarning::MultiObjectMerge { object_count: 3 }],
    );
}

#[test]
fn flatten_preserves_the_name_of_a_single_instanced_leaf() {
    // A single occurrence keeps its source mesh's name even when it
    // sits behind an instance, so the ASCII STL writer still gets
    // its `solid <name>` header.
    let mut scene = Scene::new();
    let rid = register_triangle_subtree(&mut scene, Some("Solo"));
    let root = scene.add_group(None, Transform3D::Identity);
    add_instance_at(&mut scene, root, rid, [5.0, 0.0, 0.0]);
    scene.add_root(root).expect("root registers");

    let flattened = flatten_graph_to_mesh(&scene).expect("flatten succeeds");
    assert_eq!(
        flattened
            .mesh
            .meta()
            .as_ref()
            .and_then(|m| m.name.as_deref()),
        Some("Solo")
    );
    assert!(has_vertex(&flattened.mesh, [6.0, 0.0, 0.0]));
    assert!(flattened.warnings.is_empty());
}

#[test]
fn flatten_ignores_an_unreferenced_subtree_resource() {
    // A registered subtree with no referencing instance contributes
    // no occurrence, matching what every other consumer of the
    // scene sees.
    let mut scene = Scene::new();
    let root = scene.add_group(None, Transform3D::Identity);
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_triangle(None))),
        )
        .expect("leaf construction succeeds");
    scene.add_child(root, leaf).expect("attach succeeds");
    scene.add_root(root).expect("root registers");
    let orphan = scene.add_group(None, Transform3D::Identity);
    scene
        .register_subtree_resource(orphan)
        .expect("unparented group registers");

    let flattened = flatten_graph_to_mesh(&scene).expect("flatten succeeds");
    assert_eq!(flattened.mesh.faces.len(), 1);
    assert!(flattened.warnings.is_empty());
}

// --- write_scene over instance scenes --------------------------------

#[test]
fn obj_and_stl_write_an_instance_scene_with_full_geometry() {
    let scene = scene_with_instance(2);
    let dir = tempfile::tempdir().expect("temp dir");

    let obj_target = dir.path().join("scene.obj");
    write_scene(&scene, &obj_target, WriteFormat::Obj).expect("obj write succeeds");
    let obj_text = std::fs::read_to_string(&obj_target).expect("obj target written");
    assert_eq!(obj_text.lines().filter(|l| l.starts_with("f ")).count(), 3);
    assert!(
        obj_text.contains("v 11 0 0"),
        "instanced occurrence missing from OBJ output"
    );

    let stl_target = dir.path().join("scene.stl");
    let warnings = write_scene(&scene, &stl_target, WriteFormat::StlBin).expect("stl write");
    assert_eq!(
        warnings,
        vec![FlattenWarning::MultiObjectMerge { object_count: 3 }],
    );
    let mut file = std::fs::File::open(&stl_target).expect("stl target written");
    let merged = read_stlb(&mut file).expect("written STL parses");
    let leaf = merged.first_leaf_id().expect("merged scene has a leaf");
    let mesh = merged.mesh_at_leaf(leaf).expect("leaf resolves");
    assert_eq!(mesh.faces().len(), 3);
    assert!(has_vertex(mesh, [11.0, 0.0, 0.0]));

    let stla_target = dir.path().join("scene.stla");
    write_scene(&scene, &stla_target, WriteFormat::StlAscii).expect("ascii stl write");
    let stla_text = std::fs::read_to_string(&stla_target).expect("ascii target written");
    assert!(stla_text.starts_with("solid"));
    assert_eq!(stla_text.matches("facet normal").count(), 3);
}

#[test]
fn stl_writes_an_instanced_prism_trio_in_full() {
    let meshes = [
        unit_cube_mesh(),
        cuboid_with_z_tunnel(),
        cuboid_with_y_tunnel(),
    ];
    let trio_faces: usize = meshes.iter().map(|m| m.faces().len()).sum();

    let mut scene = Scene::new();
    let sub_root = scene.add_group(None, Transform3D::Identity);
    for (i, mesh) in meshes.iter().enumerate() {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([
                    i as f64 * 30.0,
                    0.0,
                    0.0,
                ])),
                LeafPayload::Mesh(Arc::new(mesh.clone())),
            )
            .expect("add trio leaf");
        scene.add_child(sub_root, leaf).expect("attach trio leaf");
    }
    let rid = scene
        .register_subtree_resource(sub_root)
        .expect("unparented group registers");
    let root = scene.add_group(None, Transform3D::Identity);
    add_instance_at(&mut scene, root, rid, [0.0, 0.0, 0.0]);
    add_instance_at(&mut scene, root, rid, [0.0, 100.0, 0.0]);
    scene.add_root(root).expect("root registers");

    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("trio.stl");
    let warnings = write_scene(&scene, &target, WriteFormat::StlBin).expect("write succeeds");
    assert_eq!(
        warnings,
        vec![FlattenWarning::MultiObjectMerge { object_count: 6 }],
    );
    let mut file = std::fs::File::open(&target).expect("target written");
    let merged = read_stlb(&mut file).expect("written STL parses");
    let leaf = merged.first_leaf_id().expect("merged scene has a leaf");
    let mesh = merged.mesh_at_leaf(leaf).expect("leaf resolves");
    assert_eq!(mesh.faces().len(), 2 * trio_faces);

    // Both occurrences of the cube's first vertex: verbatim and
    // shifted 100mm along +Y.
    let c = unit_cube_mesh().vertices()[0];
    let shifted = AffineTransform3D::from_translation([0.0, 100.0, 0.0]).apply_to_vertex(c);
    assert!(has_vertex(mesh, c.coords()), "first occurrence missing");
    assert!(
        has_vertex(mesh, shifted.coords()),
        "second occurrence missing"
    );
}

#[test]
fn threemf_write_of_an_instance_scene_fails_loud_for_now() {
    // Interim pin, not a target state: the 3MF writer does not map
    // instances to native components yet, and until it does the gap
    // must stay a typed error, never silent geometry loss. This pin
    // is replaced by an export-correctly pin when the component
    // mapping lands; it must never be replaced by silence.
    let scene = scene_with_instance(2);
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("scene.3mf");
    match write_scene(&scene, &target, WriteFormat::ThreeMf) {
        Err(WriteSceneError::ThreeMf(WriteThreeMfError::InstanceUnrepresentable { .. })) => {}
        Err(other) => panic!("wrong error: {other:?}"),
        Ok(_) => panic!("3MF cannot represent this scene yet; writing it must fail loud"),
    }
    assert!(
        !target.exists(),
        "a failed write must not leave a target file"
    );
}

#[test]
fn every_format_writes_an_instance_free_scene() {
    let mut scene = Scene::new();
    let root = scene.add_group(None, Transform3D::Identity);
    for i in 0..2 {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([
                    2.0 * i as f64,
                    0.0,
                    0.0,
                ])),
                LeafPayload::Mesh(Arc::new(unit_triangle(None))),
            )
            .expect("leaf construction succeeds");
        scene.add_child(root, leaf).expect("attach succeeds");
    }
    scene.add_root(root).expect("root registers");

    let dir = tempfile::tempdir().expect("temp dir");
    for (format, name) in [
        (WriteFormat::Obj, "scene.obj"),
        (WriteFormat::StlBin, "scene-bin.stl"),
        (WriteFormat::StlAscii, "scene-ascii.stl"),
        (WriteFormat::ThreeMf, "scene.3mf"),
    ] {
        let target = dir.path().join(name);
        write_scene(&scene, &target, format).unwrap_or_else(|e| panic!("{name} write failed: {e}"));
        let written = std::fs::metadata(&target)
            .unwrap_or_else(|e| panic!("{name} missing after write: {e}"));
        assert!(written.len() > 0, "{name} is empty");
    }
}

// --- expansion budget through the public write entries ---------------

/// Registers a chain of subtree resources whose top resource expands
/// to 10^levels triangle occurrences, using O(levels) nodes: each
/// level is a group of 10 instances of the previous one.
fn register_power_of_ten_chain(g: &mut Scene, levels: u32) -> SubtreeResourceId {
    let mut rid = register_triangle_subtree(g, None);
    for _ in 0..levels {
        let group = g.add_group(None, Transform3D::Identity);
        for _ in 0..10 {
            let inst = g
                .add_instance(None, Transform3D::Identity, rid)
                .expect("registered resource resolves");
            g.add_child(group, inst).expect("attach chain instance");
        }
        rid = g
            .register_subtree_resource(group)
            .expect("unparented group registers");
    }
    rid
}

fn at_cap_scene() -> Scene {
    let mut g = Scene::new();
    let rid = register_power_of_ten_chain(&mut g, 5);
    let root = g.add_group(None, Transform3D::Identity);
    add_instance_at(&mut g, root, rid, [0.0, 0.0, 0.0]);
    g.add_root(root).expect("root registers");
    g
}

fn over_cap_scene() -> Scene {
    let mut g = at_cap_scene();
    let root = g.roots()[0];
    let extra = g
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_triangle(None))),
        )
        .expect("add extra leaf");
    g.add_child(root, extra).expect("attach extra leaf");
    g
}

#[test]
fn flatten_accepts_an_at_cap_expansion() {
    // The budget is inclusive, and the count assertion pins the
    // cap's value through this path so a lowered cap cannot ship
    // unnoticed.
    let scene = at_cap_scene();
    assert_eq!(
        scene
            .expanded_leaf_count(CsgPolicy::Refuse)
            .expect("chain is acyclic"),
        MAX_EXPANDED_LEAF_COUNT
    );
    let flattened = flatten_graph_to_mesh(&scene).expect("at-cap scene flattens");
    assert_eq!(flattened.mesh.faces.len(), MAX_EXPANDED_LEAF_COUNT);
}

#[test]
fn flatten_refuses_an_over_cap_expansion() {
    let scene = over_cap_scene();
    match flatten_graph_to_mesh(&scene) {
        Err(WriteSceneError::LeafBudgetExceeded { limit }) => {
            assert_eq!(limit, MAX_EXPANDED_LEAF_COUNT);
        }
        Err(other) => panic!("wrong error: {other:?}"),
        Ok(_) => panic!("over-cap scene flattened instead of being refused"),
    }
}

#[test]
fn write_scene_surfaces_the_stl_budget_error_and_writes_nothing() {
    let scene = over_cap_scene();
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("bomb.stl");
    match write_scene(&scene, &target, WriteFormat::StlBin) {
        Err(WriteSceneError::LeafBudgetExceeded { limit }) => {
            assert_eq!(limit, MAX_EXPANDED_LEAF_COUNT);
        }
        Err(other) => panic!("wrong error: {other:?}"),
        Ok(_) => panic!("over-cap scene serialised instead of being refused"),
    }
    assert!(!target.exists(), "a refused write must not leave a file");
}

#[test]
fn write_scene_surfaces_the_obj_budget_error() {
    let scene = over_cap_scene();
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("bomb.obj");
    match write_scene(&scene, &target, WriteFormat::Obj) {
        Err(WriteSceneError::Obj(WriteObjError::LeafBudgetExceeded { limit })) => {
            assert_eq!(limit, MAX_EXPANDED_LEAF_COUNT);
        }
        Err(other) => panic!("wrong error: {other:?}"),
        Ok(_) => panic!("over-cap scene serialised instead of being refused"),
    }
    assert!(!target.exists(), "a refused write must not leave a file");
}
