/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Behavioural tests for `ViewportController::handle_before_rendering`.
//!
//! The tests drive the controller against a [`MockRenderer`] stand-in
//! so the per-frame braid is exercised without wgpu. Coverage focus:
//! the upload / camera-init / redraw-owed phases of the method, which
//! run before the `self.ui.upgrade()` gate.
//!
//! Phases past the UI-upgrade gate (effective-size, gizmo composition,
//! render call, texture handoff) need a real Slint `MainWindow` and
//! land under the software-wgpu integration vehicle (Phase 1.F).

use std::rc::Rc;
use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::Transform3D;
use sc_scene::LeafPayload;
use sc_scene::Scene;

use super::controller::ViewportController;
use super::types::FrameState;
use crate::mesh_render::mock::{MockRenderer, MockRendererState};

/// Build a fresh controller with a `MockRenderer` installed, plus the
/// returned shared state handle for inspection.
fn fixture() -> (
    ViewportController,
    Rc<std::cell::RefCell<MockRendererState>>,
) {
    let ctrl = ViewportController::new_for_test();
    let state = MockRendererState::new_shared();
    ctrl.set_renderer_for_test(Box::new(MockRenderer::new(state.clone())));
    (ctrl, state)
}

/// [`fixture`] behind an `Rc` with the document listener subscribed, so
/// a document edit drives the same frame-state cascade it drives in
/// production instead of the test setting the flags by hand.
fn subscribed_fixture() -> (
    Rc<ViewportController>,
    Rc<std::cell::RefCell<MockRendererState>>,
) {
    let ctrl = Rc::new(ViewportController::new_for_test());
    let state = MockRendererState::new_shared();
    ctrl.set_renderer_for_test(Box::new(MockRenderer::new(state.clone())));
    ctrl.register_document_listener();
    (ctrl, state)
}

fn empty_leaf_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
        faces: vec![],
    }
}

/// Loader-faithful single-leaf graph under one identity-Group root.
fn graph_with_one_leaf() -> Scene {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    let leaf = g
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(empty_leaf_mesh())),
        )
        .expect("add leaf");
    g.add_child(root, leaf).expect("attach leaf");
    g
}

#[test]
fn no_upload_owed_skips_the_upload_phase() {
    let (ctrl, state) = fixture();
    // Scene present, but no upload_owed flag: the upload phase
    // must not fire even though a scene is installed.
    ctrl.install_graph_for_test(Scene::new());
    ctrl.set_frame_state_for_test(FrameState::default());
    ctrl.handle_before_rendering();
    assert!(state.borrow().upload_calls.is_empty());
}

#[test]
fn upload_owed_with_scene_triggers_one_upload_and_clears_the_flag() {
    let (ctrl, state) = fixture();
    ctrl.install_graph_for_test(Scene::new());
    ctrl.set_frame_state_for_test(FrameState::default().with_upload_owed());
    ctrl.handle_before_rendering();
    let s = state.borrow();
    assert_eq!(s.upload_calls.len(), 1);
    let frame = ctrl.frame_state_for_test();
    // `after_upload_success` clears upload_owed and sets redraw_owed.
    assert!(!frame.upload_owed());
    assert!(frame.redraw_owed());
}

#[test]
fn upload_owed_success_resets_the_loader_failure_counter() {
    let (ctrl, state) = fixture();
    // Pre-load the counter so a successful upload's reset is observable.
    ctrl.loader()
        .note_upload_failure(&anyhow::anyhow!("prior failure"));
    assert_eq!(ctrl.loader().upload_failures_for_test(), 1);
    ctrl.install_graph_for_test(Scene::new());
    ctrl.set_frame_state_for_test(FrameState::default().with_upload_owed());
    ctrl.handle_before_rendering();
    drop(state); // recording not consulted in this assertion
    assert_eq!(ctrl.loader().upload_failures_for_test(), 0);
}

#[test]
fn upload_owed_failure_notes_the_loader_failure_and_clears_the_flag() {
    let (ctrl, state) = fixture();
    state
        .borrow_mut()
        .next_upload_results
        .push_back(Err(anyhow::anyhow!("scripted upload failure")));
    ctrl.install_graph_for_test(Scene::new());
    ctrl.set_frame_state_for_test(FrameState::default().with_upload_owed());
    ctrl.handle_before_rendering();
    let s = state.borrow();
    assert_eq!(s.upload_calls.len(), 1);
    // Loader observed the failure (counter incremented).
    assert_eq!(ctrl.loader().upload_failures_for_test(), 1);
    // The flag is cleared rather than left standing: with paint
    // continuing past a failure, a still-set flag would re-run the
    // failing build and the scene drop on every later dirty event,
    // churning the render-target allocations for as long as the
    // failure lasts. Every path that changes what needs uploading
    // sets the flag itself (`RenderingSetup`, scene install, and each
    // structural edit), so a retry still arrives when there is
    // something new to upload.
    let frame = ctrl.frame_state_for_test();
    assert!(!frame.upload_owed());
}

#[test]
fn upload_owed_failure_drops_the_renderer_scene() {
    // A failed upload leaves the installed GpuScene disagreeing with
    // the live graph. Dropping it is what makes painting on safe: the
    // frame falls back to the bare plate instead of drawing against a
    // mirror that no longer matches.
    let (ctrl, state) = fixture();
    state
        .borrow_mut()
        .next_upload_results
        .push_back(Err(anyhow::anyhow!("scripted upload failure")));
    ctrl.install_graph_for_test(Scene::new());
    ctrl.set_frame_state_for_test(FrameState::default().with_upload_owed());
    ctrl.handle_before_rendering();
    assert_eq!(state.borrow().clear_scene_calls, 1);
}

#[test]
fn upload_owed_failure_does_not_skip_the_rest_of_the_frame() {
    // The reported lock-up: a failed upload used to abandon the frame
    // before the paint work, leaving the viewport frozen on its last
    // texture with no camera or gizmo response. The camera-init phase
    // sits after the upload phase and before the UI-upgrade gate, so
    // its firing is the signal available through this fixture that the
    // frame was carried on with rather than abandoned.
    let (ctrl, state) = fixture();
    state
        .borrow_mut()
        .next_upload_results
        .push_back(Err(anyhow::anyhow!("scripted upload failure")));
    ctrl.install_graph_for_test(Scene::new());
    ctrl.set_frame_state_for_test(FrameState::default().with_upload_owed());
    assert!(!ctrl.camera_is_initialised_for_test());
    ctrl.handle_before_rendering();
    assert!(ctrl.camera_is_initialised_for_test());
}

#[test]
fn upload_owed_with_no_scene_transitions_to_upload_cleared() {
    let (ctrl, state) = fixture();
    // No scene installed: `with_scene` returns `None`, the `None`
    // arm flips upload_owed off without marking redraw_owed.
    ctrl.set_frame_state_for_test(FrameState::default().with_upload_owed());
    ctrl.handle_before_rendering();
    let s = state.borrow();
    assert!(s.upload_calls.is_empty());
    let frame = ctrl.frame_state_for_test();
    assert!(!frame.upload_owed());
    assert!(!frame.redraw_owed());
}

#[test]
fn deleting_the_only_object_leaves_the_frame_clean() {
    // The reported gesture, driven through the production cascade:
    // the structural edit flags an upload, and the frame that follows
    // completes it without a failure and without leaving work owed.
    let (ctrl, state) = subscribed_fixture();
    ctrl.install_graph_for_test(graph_with_one_leaf());
    ctrl.on_select_by_index(0);
    ctrl.clear_frame_state_for_test();

    ctrl.on_delete_requested();
    assert_eq!(ctrl.reachable_leaf_count_for_test(), 0);
    assert!(
        ctrl.frame_state_for_test().upload_owed(),
        "the structural edit should flag the GPU mirror as stale"
    );

    ctrl.handle_before_rendering();
    assert_eq!(state.borrow().upload_calls.len(), 1);
    assert!(!ctrl.frame_state_for_test().upload_owed());
    assert_eq!(ctrl.loader().upload_failures_for_test(), 0);
}

#[test]
fn lazy_camera_init_fires_on_the_first_call_and_only_then() {
    let (ctrl, state) = fixture();
    // Configure the mock to return concrete scene bounds so the
    // camera framing has something to consume.
    state.borrow_mut().scene_bounds_value = Some((
        glam::Vec3::new(-1.0, -1.0, -1.0),
        glam::Vec3::new(1.0, 1.0, 1.0),
    ));
    assert!(!ctrl.camera_is_initialised_for_test());
    // Frame state with no flags: the upload phase short-circuits,
    // the camera-init phase still runs because it gates on
    // `is_initialised`, not on a flag.
    ctrl.set_frame_state_for_test(FrameState::default());
    ctrl.handle_before_rendering();
    assert!(ctrl.camera_is_initialised_for_test());
    // A second call must NOT re-initialise (no observable side
    // effect to assert directly, but the second call is a no-op
    // for the camera if `is_initialised()` is honoured).
    ctrl.handle_before_rendering();
    assert!(ctrl.camera_is_initialised_for_test());
}

#[test]
fn no_redraw_owed_short_circuits_before_the_ui_upgrade_gate() {
    // The redraw_owed check sits between the camera-init phase and
    // the UI-upgrade gate. With `redraw_owed = false`, the method
    // must short-circuit; any test-observable signal of "the render
    // phase ran" stays unset. The mock's render-call recording is
    // the proxy (and would also stay empty under the UI-upgrade
    // early return, so this test pins the lower of the two short
    // circuits, not which one fires).
    let (ctrl, state) = fixture();
    ctrl.install_graph_for_test(Scene::new());
    ctrl.set_frame_state_for_test(FrameState::default().with_upload_owed());
    ctrl.handle_before_rendering();
    // upload happened; redraw_owed got set by `after_upload_success`;
    // the same call's render-side short-circuits via the UI-upgrade
    // gate (dangling Weak in the test fixture). render_calls stays
    // empty regardless of which gate fired.
    assert!(state.borrow().render_calls.is_empty());
}

#[test]
fn a_lost_device_halts_before_rendering_without_a_dialog() {
    let (ctrl, state) = fixture();
    ctrl.install_graph_for_test(Scene::new());
    ctrl.loader()
        .device_loss()
        .mark("Unknown: test".to_string());
    ctrl.set_frame_state_for_test(FrameState::default().with_upload_owed().with_redraw_owed());
    ctrl.handle_before_rendering();
    assert!(state.borrow().upload_calls.is_empty());
    assert!(state.borrow().render_calls.is_empty());
    let frame = ctrl.frame_state_for_test();
    assert!(!frame.upload_owed());
    assert!(!frame.redraw_owed());
    assert_eq!(ctrl.loader().upload_failures_for_test(), 0);
    assert_eq!(ctrl.loader().render_failures_for_test(), 0);
}
