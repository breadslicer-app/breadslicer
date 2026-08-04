/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! `Display` / `source()` / `From` coverage for [`WriteSceneError`].

use std::error::Error;
use std::io;
use std::path::PathBuf;

use sc_mesh_core::Scene;
use sc_mesh_core::SceneError;
use sc_mesh_core::transform::Transform3D;

use crate::error::{WriteObjError, WriteSceneError, WriteStlError};

fn sample_scene_error() -> SceneError {
    // Construct a real NodeId through the Scene API so the inner
    // variant is well-formed, then wrap it as DanglingNodeId (no
    // additional invariant required).
    let mut scene = Scene::new();
    let id = scene.add_group(None, Transform3D::Identity);
    SceneError::DanglingNodeId { id }
}

#[test]
fn io_variant_display_and_source() {
    let err = WriteSceneError::Io(io::Error::other("disk full"));
    let s = err.to_string();
    assert!(s.contains("I/O error while writing scene"));
    assert!(s.contains("disk full"));
    assert!(err.source().is_some());
}

#[test]
fn empty_scene_variant_display_and_source() {
    let err = WriteSceneError::EmptyScene;
    let s = err.to_string();
    assert!(s.contains("empty scene") || s.contains("Cannot flatten"));
    assert!(err.source().is_none());
}

#[test]
fn instance_unrepresentable_variant_display_and_source() {
    // A real instance id minted through the Scene API, so the
    // display text names an id that could actually occur.
    let mut scene = Scene::new();
    let subtree = scene.add_group(None, Transform3D::Identity);
    let rid = scene
        .register_subtree_resource(subtree)
        .expect("unparented group registers");
    let node_id = scene
        .add_instance(None, Transform3D::Identity, rid)
        .expect("registered resource resolves");
    let err = WriteSceneError::InstanceUnrepresentable { node_id };
    let s = err.to_string();
    assert!(s.contains("instance reference"));
    assert!(s.contains(&node_id.get().to_string()));
    assert!(err.source().is_none());
}

#[test]
fn path_has_no_file_name_variant_display_and_source() {
    let err = WriteSceneError::PathHasNoFileName(PathBuf::from("/"));
    let s = err.to_string();
    assert!(s.contains("no file name"));
    assert!(err.source().is_none());
}

#[test]
fn create_temp_file_variant_display_and_source() {
    let err = WriteSceneError::CreateTempFile {
        path: PathBuf::from("/tmp/foo.tmp"),
        source: io::Error::other("permission denied"),
    };
    let s = err.to_string();
    assert!(s.contains("/tmp/foo.tmp"));
    assert!(s.contains("permission denied"));
    assert!(err.source().is_some());
}

#[test]
fn rename_variant_display_and_source() {
    let err = WriteSceneError::Rename {
        from: PathBuf::from("/tmp/a.tmp"),
        to: PathBuf::from("/tmp/b.stl"),
        source: io::Error::other("cross-device link"),
    };
    let s = err.to_string();
    assert!(s.contains("/tmp/a.tmp"));
    assert!(s.contains("/tmp/b.stl"));
    assert!(s.contains("cross-device link"));
    assert!(err.source().is_some());
}

#[test]
fn graph_variant_display_and_source() {
    let err = WriteSceneError::Graph(sample_scene_error());
    let s = err.to_string();
    assert!(s.contains("dangling node id"));
    assert!(err.source().is_some());
}

#[test]
fn stl_variant_display_and_source() {
    let err = WriteSceneError::Stl(WriteStlError::FaceCountExceedsU32 {
        count: 5_000_000_000,
    });
    let s = err.to_string();
    assert!(s.contains("5000000000"));
    assert!(err.source().is_some());
}

#[test]
fn obj_variant_display_and_source() {
    let err = WriteSceneError::Obj(WriteObjError::VertexCountExceedsU32 {
        vertex_count: 5_000_000_000,
    });
    let s = err.to_string();
    assert!(s.contains("5000000000"));
    assert!(err.source().is_some());
}

#[test]
fn from_io_error_routes_to_io_variant() {
    let err: WriteSceneError = io::Error::other("x").into();
    assert!(matches!(err, WriteSceneError::Io(_)));
}

#[test]
fn from_scene_error_routes_to_graph_variant() {
    let err: WriteSceneError = sample_scene_error().into();
    assert!(matches!(err, WriteSceneError::Graph(_)));
}

#[test]
fn from_write_stl_error_routes_to_stl_variant() {
    let err: WriteSceneError = WriteStlError::FaceCountExceedsU32 { count: 1 }.into();
    assert!(matches!(err, WriteSceneError::Stl(_)));
}

#[test]
fn from_write_obj_error_routes_to_obj_variant() {
    let err: WriteSceneError = WriteObjError::VertexCountExceedsU32 { vertex_count: 1 }.into();
    assert!(matches!(err, WriteSceneError::Obj(_)));
}
