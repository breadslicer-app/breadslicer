/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Behavioural tests for the wgpu-free helpers in
//! `crate::mesh_loader`. The async worker spawned by `spawn_load`
//! requires a real `wgpu::Device` and stays under the software-wgpu
//! integration vehicle (Phase 1.F).

use std::any::Any;
use std::io::Write;
use std::path::PathBuf;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene_node::LeafPayload;
use sc_mesh_core::transform::Transform3D;

use crate::build_volume::BuildVolume;
use crate::error_dialog::DialogManager;
use crate::mesh_loader::{
    MAX_MESH_FILE_SIZE_BYTES, check_file_size, panic_message, place_on_plate,
    start_load_with_ticket,
};
use crate::scene_loader::SceneLoader;

fn unit_triangle_mesh() -> IndexedMesh {
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

fn scene_with_one_triangle_at_origin() -> Scene {
    let mut s = Scene::new();
    let leaf = s
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(unit_triangle_mesh().into()),
        )
        .expect("add_leaf");
    s.add_root(leaf).expect("add_root");
    s
}

fn loader() -> SceneLoader {
    SceneLoader::new_for_test(DialogManager::new())
}

// --- check_file_size --------------------------------------------------

#[test]
fn check_file_size_returns_ok_for_a_file_under_the_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("small.stl");
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(b"hello").expect("write");
    drop(f);
    assert!(check_file_size(&path).is_ok());
}

#[test]
fn check_file_size_returns_err_for_a_file_over_the_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.stl");
    let f = std::fs::File::create(&path).expect("create");
    // Sparse file: set the length without allocating; metadata().len()
    // still reports the logical size.
    f.set_len(MAX_MESH_FILE_SIZE_BYTES + 1).expect("set_len");
    drop(f);
    let result = check_file_size(&path);
    let err = result.expect_err("over-cap file must fail the check");
    let msg = err.to_string();
    assert!(msg.contains("exceeds"), "msg = {msg}");
    assert!(msg.contains("limit"), "msg = {msg}");
}

#[test]
fn check_file_size_returns_ok_for_a_missing_path_so_the_worker_can_report_the_real_error() {
    let path = PathBuf::from("/this/path/does/not/exist/scene.stl");
    assert!(check_file_size(&path).is_ok());
}

// --- place_on_plate ---------------------------------------------------

#[test]
fn place_on_plate_is_a_no_op_on_an_empty_scene() {
    let mut scene = Scene::new();
    let bv = BuildVolume::new(100.0, 60.0, 80.0);
    place_on_plate(&mut scene, bv).expect("place ok");
    assert!(scene.roots().is_empty());
}

#[test]
fn place_on_plate_translates_a_scene_with_geometry() {
    let mut scene = scene_with_one_triangle_at_origin();
    let bv = BuildVolume::new(100.0, 60.0, 80.0);
    let root_id_before = *scene.roots().first().expect("one root");
    let xform_before = scene
        .node(root_id_before)
        .expect("root present")
        .local_transform;
    assert_eq!(xform_before, Transform3D::Identity);
    place_on_plate(&mut scene, bv).expect("place ok");
    let xform_after = scene
        .node(root_id_before)
        .expect("root still present")
        .local_transform;
    assert_ne!(
        xform_after,
        Transform3D::Identity,
        "root must carry an outer translation"
    );
}

// --- start_load_with_ticket ------------------------------------------

#[test]
fn start_load_with_ticket_deposits_an_err_for_an_oversize_file() {
    let l = loader();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.stl");
    let f = std::fs::File::create(&path).expect("create");
    f.set_len(MAX_MESH_FILE_SIZE_BYTES + 1).expect("set_len");
    drop(f);
    let ticket = l.begin_load().expect("begin");
    start_load_with_ticket(
        ticket,
        path.clone(),
        &l,
        BuildVolume::new(100.0, 60.0, 80.0),
    );
    let drained = l
        .drain_deposit_for_test()
        .expect("size-cap bailout must deposit a result");
    let (drained_path, result) = drained;
    assert_eq!(drained_path, path);
    let Err(err) = result else {
        panic!("oversize must deposit an Err");
    };
    assert!(err.to_string().contains("exceeds"), "{err}");
}

#[test]
fn start_load_with_ticket_deposits_an_err_when_no_gpu_snapshot_is_available() {
    let l = loader();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("small.stl");
    std::fs::File::create(&path).expect("create");
    // gpu_snapshot is `None` because no `set_gpu` has been called.
    let ticket = l.begin_load().expect("begin");
    start_load_with_ticket(
        ticket,
        path.clone(),
        &l,
        BuildVolume::new(100.0, 60.0, 80.0),
    );
    let drained = l
        .drain_deposit_for_test()
        .expect("no-gpu bailout must deposit a result");
    let (drained_path, result) = drained;
    assert_eq!(drained_path, path);
    let Err(err) = result else {
        panic!("no-gpu must deposit an Err");
    };
    assert!(
        err.to_string().contains("Graphics device"),
        "expected 'Graphics device' wording, got: {err}"
    );
}

// --- panic_message ---------------------------------------------------

#[test]
fn panic_message_decodes_a_static_str_payload() {
    let payload: Box<dyn Any + Send> = Box::new("static panic msg");
    let s = panic_message(&payload);
    assert_eq!(s, "static panic msg");
}

#[test]
fn panic_message_decodes_a_string_payload() {
    let payload: Box<dyn Any + Send> = Box::new(String::from("owned panic msg"));
    let s = panic_message(&payload);
    assert_eq!(s, "owned panic msg");
}

#[test]
fn panic_message_falls_back_for_an_unknown_payload_type() {
    let payload: Box<dyn Any + Send> = Box::new(42_u32);
    let s = panic_message(&payload);
    assert!(s.contains("unknown panic payload"));
}

#[test]
fn panic_message_decodes_a_boxed_error_send_sync_payload() {
    let inner: Box<dyn std::error::Error + Send + Sync + 'static> =
        Box::new(std::io::Error::other("send-sync error payload"));
    let payload: Box<dyn Any + Send> = Box::new(inner);
    let s = panic_message(&payload);
    assert!(s.contains("send-sync error payload"));
}

#[test]
fn panic_message_decodes_a_boxed_error_send_payload() {
    let inner: Box<dyn std::error::Error + Send + 'static> =
        Box::new(std::io::Error::other("send-only error payload"));
    let payload: Box<dyn Any + Send> = Box::new(inner);
    let s = panic_message(&payload);
    assert!(s.contains("send-only error payload"));
}
