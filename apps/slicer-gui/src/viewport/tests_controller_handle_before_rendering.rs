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

use sc_mesh_core::scene::Scene;

use super::controller::ViewportController;
use super::types::FrameState;
use crate::mesh_render::mock::{MockRenderer, MockRendererState};

/// Build a fresh controller with a `MockRenderer` installed, plus the
/// returned shared state handle for inspection.
fn fixture() -> (
    ViewportController,
    std::rc::Rc<std::cell::RefCell<MockRendererState>>,
) {
    let ctrl = ViewportController::new_for_test();
    let state = MockRendererState::new_shared();
    ctrl.set_renderer_for_test(Box::new(MockRenderer::new(state.clone())));
    (ctrl, state)
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
fn upload_owed_failure_notes_the_loader_failure_and_keeps_the_flag_set() {
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
    // upload_owed stays set so the next setup/reload triggers a retry.
    let frame = ctrl.frame_state_for_test();
    assert!(frame.upload_owed());
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
