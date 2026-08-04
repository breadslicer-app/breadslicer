/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Contract coverage for [`InstanceFreeScene`]: scenes using
//! `Instance` nodes are refused once, at witness mint, before any
//! writer or flatten walk runs; instance-free scenes pass through
//! every format unchanged.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, LeafPayload, NodeId, Normal, Scene, Vertex};

use crate::error::WriteSceneError;
use crate::filetypes::WriteFormat;
use crate::scene_writer::{FlattenWarning, InstanceFreeScene, flatten_graph_to_mesh, write_scene};

fn unit_triangle() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ],
        faces: vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 2],
        )],
    }
}

fn add_plain_leaf(scene: &mut Scene, parent: NodeId, x: f64) {
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.0])),
            LeafPayload::Mesh(Arc::new(unit_triangle())),
        )
        .expect("leaf construction succeeds");
    scene.add_child(parent, leaf).expect("attach succeeds");
}

/// Root group holding `plain_leaves` triangle leaves plus one
/// `Instance` of a registered single-leaf subtree. Returns the
/// instance's id for refusal assertions.
fn scene_with_instance(plain_leaves: usize) -> (Scene, NodeId) {
    let mut scene = Scene::new();
    let res_leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_triangle())),
        )
        .expect("subtree leaf construction succeeds");
    let res_root = scene.add_group(None, Transform3D::Identity);
    scene
        .add_child(res_root, res_leaf)
        .expect("attach succeeds");
    let rid = scene
        .register_subtree_resource(res_root)
        .expect("unparented group registers");

    let root = scene.add_group(None, Transform3D::Identity);
    for i in 0..plain_leaves {
        add_plain_leaf(&mut scene, root, 2.0 * i as f64);
    }
    let inst = scene
        .add_instance(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
            rid,
        )
        .expect("registered resource resolves");
    scene.add_child(root, inst).expect("attach succeeds");
    scene.add_root(root).expect("root registers");
    (scene, inst)
}

fn instance_free_scene(plain_leaves: usize) -> Scene {
    let mut scene = Scene::new();
    let root = scene.add_group(None, Transform3D::Identity);
    for i in 0..plain_leaves {
        add_plain_leaf(&mut scene, root, 2.0 * i as f64);
    }
    scene.add_root(root).expect("root registers");
    scene
}

#[test]
fn try_new_refuses_instance_scenes_naming_the_node() {
    // 0 / 1 / 2 plain leaves next to the instance: before the
    // witness these shapes took 3 different silent paths through
    // the flatten dispatch; now all 3 die at the same gate.
    for plain in [0usize, 1, 2] {
        let (scene, inst) = scene_with_instance(plain);
        match InstanceFreeScene::try_new(&scene) {
            Err(WriteSceneError::InstanceUnrepresentable { node_id }) => {
                assert_eq!(node_id, inst, "refusal names the instance node");
            }
            Err(other) => panic!("wrong error for {plain} plain leaves: {other:?}"),
            Ok(_) => panic!("witness minted for a scene with an instance ({plain} plain leaves)"),
        }
    }
}

#[test]
fn try_new_admits_instance_free_scenes() {
    let scene = instance_free_scene(2);
    assert!(InstanceFreeScene::try_new(&scene).is_ok());
}

#[test]
fn try_new_admits_a_registered_subtree_without_instances() {
    // A subtree resource with no referencing instance contributes no
    // geometry to any walk (expanding or skipping), so the written
    // file matches what every consumer sees and the witness admits
    // the scene.
    let mut scene = instance_free_scene(1);
    let orphan_subtree = scene.add_group(None, Transform3D::Identity);
    scene
        .register_subtree_resource(orphan_subtree)
        .expect("unparented group registers");
    assert!(InstanceFreeScene::try_new(&scene).is_ok());
}

#[test]
fn flatten_still_merges_witnessed_scenes() {
    let scene = instance_free_scene(2);
    let witness = InstanceFreeScene::try_new(&scene).expect("no instances");
    let flattened = flatten_graph_to_mesh(witness).expect("flatten succeeds");
    assert_eq!(flattened.mesh.faces.len(), 2);
    assert_eq!(
        flattened.warnings,
        vec![FlattenWarning::MultiObjectMerge { object_count: 2 }],
    );
}

#[test]
fn every_format_writes_a_witnessed_instance_free_scene() {
    let scene = instance_free_scene(2);
    let witness = InstanceFreeScene::try_new(&scene).expect("no instances");
    let dir = tempfile::tempdir().expect("temp dir");
    for (format, name) in [
        (WriteFormat::Obj, "scene.obj"),
        (WriteFormat::StlBin, "scene-bin.stl"),
        (WriteFormat::StlAscii, "scene-ascii.stl"),
        (WriteFormat::ThreeMf, "scene.3mf"),
    ] {
        let target = dir.path().join(name);
        write_scene(witness, &target, format)
            .unwrap_or_else(|e| panic!("{name} write failed: {e}"));
        let written = std::fs::metadata(&target)
            .unwrap_or_else(|e| panic!("{name} missing after write: {e}"));
        assert!(written.len() > 0, "{name} is empty");
    }
}
