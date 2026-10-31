/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Behavioural tests for the load-FSM helpers in `crate::scene_loader` and for
//! the pure warning formatter `describe_obj_warning`.
//!
//! Tests reach the FSM through `SceneLoader::new_for_test` (which installs a
//! dangling `slint::Weak`, silencing UI pokes) and the `drain_deposit_for_test`
//! accessor (which bypasses `DialogManager::show_error`, the only path on the
//! Slint side that needs a live event loop).

use std::path::PathBuf;

use sc_mesh_core::transform::Transform3D;
use sc_mesh_formats::messages::{
    DuplicateFacesLeaf, LoadObjWarning, SceneLoadWarning, SharedVertexLeaf,
};
use sc_scene::{NodeId, Scene};
use slint::wgpu_30::wgpu;

use crate::error_dialog::DialogManager;
use crate::graphics_status::GraphicsUnavailable;
use crate::mesh_render::{RenderAllocation, RenderError};
use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
use crate::scene_loader::{SceneLoader, describe_obj_warning, describe_scene_warning};

/// Allocates `count` fresh NodeIds out of a throw-away scene so tests don't
/// depend on NodeId having a public raw constructor.
fn fresh_node_ids(count: usize) -> Vec<NodeId> {
    let mut graph = Scene::new();
    (0..count)
        .map(|_| graph.add_group(None, Transform3D::Identity))
        .collect()
}

fn loader() -> SceneLoader {
    SceneLoader::new_for_test(DialogManager::new(&PreferencesBus::new(
        PreferencesSnapshot::for_tests(),
    )))
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
    // Bump the generation; the still-deposited result's stamp now mismatches
    // and drain returns None.
    l.invalidate_and_clear();
    assert!(l.drain_deposit_for_test().is_none());
}

fn refused_output_image() -> RenderError {
    RenderError::DeviceRefused {
        what: RenderAllocation::ColourTargets,
        source: wgpu::Error::Validation {
            source: Box::new(std::io::Error::other("scripted")),
            description: "scripted".to_string(),
        },
    }
}

#[test]
fn note_render_failure_counts_a_run_and_a_success_ends_it() {
    let l = loader();
    l.note_render_failure(&refused_output_image());
    l.note_render_failure(&refused_output_image());
    assert_eq!(l.render_failures_for_test(), 2);
    l.note_render_success();
    assert_eq!(l.render_failures_for_test(), 0);
}

#[test]
fn note_upload_failure_counts_a_run_and_a_success_ends_it() {
    let l = loader();
    l.note_upload_failure(anyhow::anyhow!("first"));
    l.note_upload_failure(anyhow::anyhow!("second"));
    assert_eq!(l.upload_failures_for_test(), 2);
    l.note_upload_success();
    assert_eq!(l.upload_failures_for_test(), 0);
    l.note_upload_failure(anyhow::anyhow!("third"));
    assert_eq!(l.upload_failures_for_test(), 1);
}

#[test]
fn note_device_lost_reports_the_loss_and_shares_the_flag() {
    let l = loader();
    l.note_device_lost();
    assert_eq!(
        l.graphics_unavailable(),
        Some(GraphicsUnavailable::DeviceLost)
    );
    assert!(l.gpu_snapshot().is_none());
    let flag = l.device_loss();
    flag.mark("Unknown: test".to_string());
    assert!(l.device_loss().is_lost(), "clones share one flag");
}

#[test]
fn note_render_failure_ignores_a_lost_device() {
    let l = loader();
    l.note_render_failure(&RenderError::DeviceLost);
    assert_eq!(l.render_failures_for_test(), 0);
}

#[test]
fn reset_render_failures_ends_the_run() {
    let l = loader();
    l.note_render_failure(&refused_output_image());
    l.reset_render_failures();
    assert_eq!(l.render_failures_for_test(), 0);
}

// `describe_obj_warning` exhaustive coverage. The function is a pure formatter
// over every `LoadObjWarning` variant; each test constructs 1 variant and
// asserts the salient tokens land in the output.

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
fn describe_scene_shared_vertices_emits_each_object_name() {
    let ids = fresh_node_ids(2);
    let s = describe_scene_warning(&SceneLoadWarning::SharedVertices {
        leaves: vec![
            SharedVertexLeaf {
                node_id: ids[0],
                object_name: Some("cube".to_string()),
            },
            SharedVertexLeaf {
                node_id: ids[1],
                object_name: Some("sphere".to_string()),
            },
        ],
    });
    assert!(s.contains("'cube'"), "{s}");
    assert!(s.contains("'sphere'"), "{s}");
    assert!(s.contains("share at least one vertex"), "{s}");
}

#[test]
fn describe_scene_duplicate_faces_gives_the_total_and_each_mesh() {
    let ids = fresh_node_ids(2);
    let s = describe_scene_warning(&SceneLoadWarning::DuplicateFacesDropped {
        leaves: vec![
            DuplicateFacesLeaf {
                node_id: ids[0],
                object_name: Some("Werewolf".to_string()),
                faces: 2160,
            },
            DuplicateFacesLeaf {
                node_id: ids[1],
                object_name: None,
                faces: 12,
            },
        ],
    });
    assert_eq!(
        s,
        format!(
            "removed 2172 duplicate face(s), exact copies of other faces: 'Werewolf' (2160), \
             node {} (12)",
            ids[1].get()
        )
    );
}

#[test]
fn describe_scene_shared_vertices_falls_back_to_node_id_when_nameless() {
    let ids = fresh_node_ids(2);
    let nameless = ids[0];
    let s = describe_scene_warning(&SceneLoadWarning::SharedVertices {
        leaves: vec![
            SharedVertexLeaf {
                node_id: nameless,
                object_name: None,
            },
            SharedVertexLeaf {
                node_id: ids[1],
                object_name: Some("cube".to_string()),
            },
        ],
    });
    assert!(s.contains(&format!("node {}", nameless.get())), "{s}");
    assert!(s.contains("'cube'"), "{s}");
}
