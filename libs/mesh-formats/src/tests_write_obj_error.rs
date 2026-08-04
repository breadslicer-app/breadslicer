/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! `Display` / `source()` / `From` coverage for [`WriteObjError`].

use std::error::Error;
use std::io;

use sc_mesh_core::Scene;
use sc_mesh_core::SceneError;
use sc_mesh_core::transform::Transform3D;

use crate::error::WriteObjError;

fn sample_scene_error() -> SceneError {
    let mut scene = Scene::new();
    let id = scene.add_group(None, Transform3D::Identity);
    SceneError::DanglingNodeId { id }
}

#[test]
fn io_variant_display_and_source() {
    let err = WriteObjError::Io(io::Error::other("flush failed"));
    let s = err.to_string();
    assert!(s.contains("OBJ writer I/O error"));
    assert!(s.contains("flush failed"));
    assert!(err.source().is_some());
}

#[test]
fn graph_variant_display_and_source() {
    let err = WriteObjError::Graph(sample_scene_error());
    let s = err.to_string();
    assert!(s.contains("OBJ writer"));
    assert!(s.contains("dangling node id"));
    assert!(err.source().is_some());
}

#[test]
fn instance_unrepresentable_variant_display_and_source() {
    let mut scene = Scene::new();
    let subtree = scene.add_group(None, Transform3D::Identity);
    let rid = scene
        .register_subtree_resource(subtree)
        .expect("unparented group registers");
    let node_id = scene
        .add_instance(None, Transform3D::Identity, rid)
        .expect("registered resource resolves");
    let err = WriteObjError::InstanceUnrepresentable { node_id };
    let s = err.to_string();
    assert!(s.contains("OBJ writer"));
    assert!(s.contains("instance reference"));
    assert!(s.contains(&node_id.get().to_string()));
    assert!(err.source().is_none());
}

#[test]
fn face_index_overflow_variant_display_and_source() {
    let err = WriteObjError::FaceIndexOverflow {
        local: 100,
        offset: u32::MAX - 50,
    };
    let s = err.to_string();
    assert!(s.contains("face vertex index overflow"));
    assert!(s.contains("100"));
    assert!(err.source().is_none());
}

#[test]
fn vertex_count_exceeds_u32_variant_display_and_source() {
    let err = WriteObjError::VertexCountExceedsU32 {
        vertex_count: 5_000_000_000,
    };
    let s = err.to_string();
    assert!(s.contains("5000000000"));
    assert!(s.contains("u32::MAX"));
    assert!(err.source().is_none());
}

#[test]
fn vertex_offset_overflow_variant_display_and_source() {
    let err = WriteObjError::VertexOffsetOverflow {
        offset: u32::MAX - 10,
        added: 100,
    };
    let s = err.to_string();
    assert!(s.contains("offset"));
    assert!(s.contains("100"));
    assert!(err.source().is_none());
}

#[test]
fn from_io_error_routes_to_io_variant() {
    let err: WriteObjError = io::Error::other("flush failed").into();
    assert!(matches!(err, WriteObjError::Io(_)));
}

#[test]
fn from_scene_error_routes_to_graph_variant() {
    let err: WriteObjError = sample_scene_error().into();
    assert!(matches!(err, WriteObjError::Graph(_)));
}
