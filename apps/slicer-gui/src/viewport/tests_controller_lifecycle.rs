/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Behavioural tests for `ViewportController`'s rendering-lifecycle
//! hooks. The Slint-callable entries (`handle_rendering_setup`,
//! `handle_rendering_teardown`) carry wgpu types, so the testable
//! surface here is:
//!
//! - `post_renderer_install` (the wgpu-free post-install logic that
//!   `handle_rendering_setup` invokes after the renderer has been
//!   installed),
//! - `handle_rendering_teardown` (which takes no wgpu types and is
//!   fully exercisable against a [`MockRenderer`]).
//!
//! Tests that drive `handle_rendering_setup` end-to-end land under
//! the software-wgpu integration vehicle (Phase 1.F).

use sc_mesh_core::scene::Scene;

use super::controller::ViewportController;
use super::types::FrameState;
use crate::mesh_render::mock::{MockRenderer, MockRendererState};

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
fn post_renderer_install_resets_the_loader_upload_failure_counter() {
    let (ctrl, _state) = fixture();
    ctrl.loader()
        .note_upload_failure(&anyhow::anyhow!("stale failure"));
    assert_eq!(ctrl.loader().upload_failures_for_test(), 1);
    ctrl.post_renderer_install();
    assert_eq!(ctrl.loader().upload_failures_for_test(), 0);
}

#[test]
fn post_renderer_install_ends_the_loader_render_failure_run() {
    use crate::mesh_render::{RenderAllocation, RenderError};
    use slint::wgpu_29::wgpu;
    let (ctrl, _state) = fixture();
    ctrl.loader()
        .note_render_failure(&RenderError::DeviceRefused {
            what: RenderAllocation::ColourTargets,
            source: wgpu::Error::Validation {
                source: Box::new(std::io::Error::other("stale")),
                description: "stale".to_string(),
            },
        });
    assert_eq!(ctrl.loader().render_failures_for_test(), 1);
    ctrl.post_renderer_install();
    assert_eq!(ctrl.loader().render_failures_for_test(), 0);
}

#[test]
fn post_renderer_install_sets_upload_owed_when_a_scene_is_loaded() {
    let (ctrl, _state) = fixture();
    ctrl.install_graph_for_test(Scene::new());
    // Ensure we start with no upload_owed flag set so the post-
    // install transition is the only path that could set it.
    ctrl.set_frame_state_for_test(FrameState::default());
    ctrl.post_renderer_install();
    assert!(ctrl.frame_state_for_test().upload_owed());
}

#[test]
fn post_renderer_install_leaves_upload_owed_unset_when_no_scene_is_loaded() {
    let (ctrl, _state) = fixture();
    ctrl.set_frame_state_for_test(FrameState::default());
    ctrl.post_renderer_install();
    assert!(!ctrl.frame_state_for_test().upload_owed());
}

#[test]
fn handle_rendering_teardown_drops_the_renderer_and_clears_redraw_owed() {
    let (ctrl, _state) = fixture();
    assert!(ctrl.renderer_installed_for_test());
    // Mark redraw_owed via the test helper so the teardown's
    // `after_render` transition is observable.
    ctrl.set_frame_state_for_test(FrameState::default().with_redraw_owed());
    assert!(ctrl.frame_state_for_test().redraw_owed());
    ctrl.handle_rendering_teardown();
    assert!(!ctrl.renderer_installed_for_test());
    assert!(!ctrl.frame_state_for_test().redraw_owed());
}
