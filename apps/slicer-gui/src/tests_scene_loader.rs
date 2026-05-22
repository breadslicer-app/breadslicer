/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Behavioural tests for the load-FSM helpers in
//! `crate::scene_loader` and for the pure warning formatter
//! `describe_obj_warning`.
//!
//! Tests reach the FSM through `SceneLoader::new_for_test` (which
//! installs a dangling `slint::Weak`, silencing UI pokes) and the
//! `drain_deposit_for_test` accessor (which bypasses
//! `DialogManager::show_error`, the only path on the Slint side
//! that needs a live event loop).

use std::path::PathBuf;

use sc_mesh_formats::messages::LoadObjWarning;

use crate::error_dialog::DialogManager;
use crate::scene_loader::{SceneLoader, describe_obj_warning};

fn loader() -> SceneLoader {
    SceneLoader::new_for_test(DialogManager::new())
}

#[test]
fn begin_load_returns_some_when_no_load_is_in_flight() {
    let l = loader();
    assert!(l.begin_load().is_some());
}

#[test]
fn begin_load_returns_none_when_another_load_is_in_flight() {
    let l = loader();
    let _ticket = l.begin_load().expect("first begin should succeed");
    assert!(
        l.begin_load().is_none(),
        "second begin must be gated by the first ticket's inflight bump"
    );
}

#[test]
fn dropping_a_ticket_releases_the_gate() {
    let l = loader();
    {
        let _ticket = l.begin_load().expect("first begin");
    }
    assert!(
        l.begin_load().is_some(),
        "second begin must succeed once the first ticket dropped"
    );
}

#[test]
fn drain_deposit_returns_none_when_no_deposit_present() {
    let l = loader();
    assert!(l.drain_deposit_for_test().is_none());
}

#[test]
fn drain_deposit_returns_some_for_matching_stamp() {
    let l = loader();
    let ticket = l.begin_load().expect("first begin");
    ticket.deposit(
        PathBuf::from("/tmp/scene.stl"),
        Err(anyhow::anyhow!("scripted worker failure")),
    );
    let drained = l.drain_deposit_for_test();
    let (path, result) = drained.expect("matching deposit drains");
    assert_eq!(path, PathBuf::from("/tmp/scene.stl"));
    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("scripted worker failure")
    );
}

#[test]
fn drain_deposit_drops_a_stale_deposit_after_invalidate_and_clear() {
    let l = loader();
    let ticket = l.begin_load().expect("first begin");
    ticket.deposit(
        PathBuf::from("/tmp/scene.stl"),
        Err(anyhow::anyhow!("worker raced an invalidate")),
    );
    // Bump the generation; the still-deposited result's stamp now
    // mismatches and drain returns None.
    l.invalidate_and_clear();
    assert!(l.drain_deposit_for_test().is_none());
}

// `describe_obj_warning` exhaustive coverage. The function is a
// pure formatter over every `LoadObjWarning` variant; each test
// constructs one variant and asserts the salient tokens land in
// the output.

#[test]
fn describe_no_vertices() {
    let s = describe_obj_warning(&LoadObjWarning::NoVertices);
    assert!(s.contains("no vertices"));
}

#[test]
fn describe_no_faces() {
    let s = describe_obj_warning(&LoadObjWarning::NoFaces);
    assert!(s.contains("no faces"));
}

#[test]
fn describe_skipped_texture_coordinates() {
    let s = describe_obj_warning(&LoadObjWarning::SkippedTextureCoordinates { line: 11 });
    assert!(s.contains("line 11"));
    assert!(s.contains("texture coordinates"));
}

#[test]
fn describe_skipped_vertex_normal() {
    let s = describe_obj_warning(&LoadObjWarning::SkippedVertexNormal { line: 13 });
    assert!(s.contains("line 13"));
    assert!(s.contains("vertex normal"));
}

#[test]
fn describe_skipped_group() {
    let s = describe_obj_warning(&LoadObjWarning::SkippedGroup {
        line: 17,
        name: "wheels".to_string(),
    });
    assert!(s.contains("line 17"));
    assert!(s.contains("wheels"));
}

#[test]
fn describe_skipped_smoothing_group() {
    let s = describe_obj_warning(&LoadObjWarning::SkippedSmoothingGroup { line: 19 });
    assert!(s.contains("line 19"));
    assert!(s.contains("smoothing"));
}

#[test]
fn describe_skipped_material_reference() {
    let s = describe_obj_warning(&LoadObjWarning::SkippedMaterialReference {
        line: 23,
        material: "shiny".to_string(),
    });
    assert!(s.contains("line 23"));
    assert!(s.contains("shiny"));
}

#[test]
fn describe_skipped_material_library() {
    let s = describe_obj_warning(&LoadObjWarning::SkippedMaterialLibrary {
        line: 29,
        path: "/tmp/lib.mtl".to_string(),
    });
    assert!(s.contains("line 29"));
    assert!(s.contains("/tmp/lib.mtl"));
}

#[test]
fn describe_skipped_comment() {
    let s = describe_obj_warning(&LoadObjWarning::SkippedComment { line: 31 });
    assert!(s.contains("line 31"));
    assert!(s.contains("comment"));
}

#[test]
fn describe_skipped_unknown_directive() {
    let s = describe_obj_warning(&LoadObjWarning::SkippedUnknownDirective {
        line: 37,
        directive: "vp".to_string(),
    });
    assert!(s.contains("line 37"));
    assert!(s.contains("vp"));
}

#[test]
fn describe_shared_vertices_named_object_branch() {
    let s = describe_obj_warning(&LoadObjWarning::SharedVertices {
        object_name: Some("cube".to_string()),
        shared_indices: vec![3, 5],
    });
    assert!(s.contains("cube"));
    assert!(s.contains("[3, 5]"));
}

#[test]
fn describe_shared_vertices_unnamed_object_branch() {
    let s = describe_obj_warning(&LoadObjWarning::SharedVertices {
        object_name: None,
        shared_indices: vec![7, 9],
    });
    assert!(s.contains("unnamed"));
    assert!(s.contains("[7, 9]"));
}
