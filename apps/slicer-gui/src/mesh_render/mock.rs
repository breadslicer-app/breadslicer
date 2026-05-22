/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Test-only stand-in for [`super::Renderer`]. Implements
//! [`super::MeshRenderer`] without touching wgpu, so callers like
//! `ViewportController::handle_before_rendering` can be exercised
//! purely in CPU memory.
//!
//! The mock's recordings live behind a shared [`Rc<RefCell<_>>`]
//! handle: the [`MockRenderer`] holds one clone and the test holds
//! the other. Once the mock is boxed into the render-host slot, the
//! controller cannot be queried for its insides; the shared handle
//! is the inspection seam.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use anyhow::Result;
use glam::Vec3;
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene_node::NodeId;
use slint::wgpu_28::wgpu;

use super::scene_upload::GpuScene;
use super::{MeshRenderer, SceneRender};
use crate::build_volume::BuildVolume;
use crate::camera::OrbitCamera;

/// Recorded arguments of a single [`MockRenderer::render`] call.
/// Captures only the fields the audit's behavioural tests assert on;
/// the full [`SceneRender`] / [`OrbitCamera`] references are not
/// snapshotted because tests don't compare them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MockRenderArgs {
    /// `true` when `render` was called with `Some(SceneRender { .. })`,
    /// `false` for the no-scene fallback call.
    pub(crate) scene_present: bool,
    /// The selection passed in via `SceneRender::selection`, or `None`
    /// when no scene was present.
    pub(crate) selection: Option<NodeId>,
    /// `true` when `SceneRender::gizmo` was `Some(_)` for this call.
    pub(crate) gizmo_present: bool,
    pub(crate) build_volume: BuildVolume,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Shared recording / configuration cell for a [`MockRenderer`].
/// Construct via [`MockRendererState::new_shared`].
pub(crate) struct MockRendererState {
    /// Every `upload_scene` call's `graph` argument, cloned, in the
    /// order calls happened.
    pub(crate) upload_calls: Vec<Scene>,
    pub(crate) render_calls: Vec<MockRenderArgs>,
    pub(crate) clear_scene_calls: usize,
    pub(crate) invalidate_sizing_calls: usize,
    pub(crate) install_scene_calls: usize,
    /// What `scene_bounds()` returns. `None` by default; the test
    /// sets this through the shared handle to simulate "scene has
    /// been installed and its AABB is X".
    pub(crate) scene_bounds_value: Option<(Vec3, Vec3)>,
    /// What `install_scene()` returns. Default `true` (worker
    /// matched the device); set `false` to simulate device rotation.
    pub(crate) install_scene_returns: bool,
    /// Queue of `upload_scene` results: each upload pops the front.
    /// Empty queue (the default) means subsequent uploads return
    /// `Ok(())`. Pre-fill with `Err(...)` entries to script failures.
    pub(crate) next_upload_results: VecDeque<Result<()>>,
}

impl MockRendererState {
    pub(crate) fn new_shared() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            upload_calls: Vec::new(),
            render_calls: Vec::new(),
            clear_scene_calls: 0,
            invalidate_sizing_calls: 0,
            install_scene_calls: 0,
            scene_bounds_value: None,
            install_scene_returns: true,
            next_upload_results: VecDeque::new(),
        }))
    }
}

/// Test stand-in for [`super::Renderer`]. All [`MeshRenderer`]
/// methods record their calls into the shared state and return
/// configured values; none touch wgpu.
pub(crate) struct MockRenderer {
    state: Rc<RefCell<MockRendererState>>,
}

impl MockRenderer {
    pub(crate) fn new(state: Rc<RefCell<MockRendererState>>) -> Self {
        Self { state }
    }
}

impl MeshRenderer for MockRenderer {
    fn upload_scene(&mut self, graph: &Scene) -> Result<()> {
        let mut s = self.state.borrow_mut();
        s.upload_calls.push(graph.clone());
        s.next_upload_results.pop_front().unwrap_or(Ok(()))
    }

    fn install_scene(&mut self, _gpu: GpuScene) -> bool {
        let mut s = self.state.borrow_mut();
        s.install_scene_calls += 1;
        s.install_scene_returns
    }

    fn scene_bounds(&self) -> Option<(Vec3, Vec3)> {
        self.state.borrow().scene_bounds_value
    }

    fn clear_scene(&mut self) {
        self.state.borrow_mut().clear_scene_calls += 1;
    }

    fn invalidate_sizing(&mut self) {
        self.state.borrow_mut().invalidate_sizing_calls += 1;
    }

    fn render(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        _camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Option<wgpu::Texture> {
        let (scene_present, selection, gizmo_present) = match scene_render.as_ref() {
            Some(sr) => (true, sr.selection, sr.gizmo.is_some()),
            None => (false, None, false),
        };
        self.state.borrow_mut().render_calls.push(MockRenderArgs {
            scene_present,
            selection,
            gizmo_present,
            build_volume,
            width,
            height,
        });
        // The mock cannot construct a real wgpu::Texture; always
        // returns `None`. Tests that need the texture-handoff branch
        // run under the software-wgpu integration vehicle (Phase 1.F).
        None
    }
}

#[cfg(test)]
mod mock_smoke_tests {
    use super::*;
    use crate::build_volume::BuildVolume;
    use crate::camera::OrbitCamera;
    use glam::Vec3;

    #[test]
    fn fresh_state_has_no_recorded_calls() {
        let state = MockRendererState::new_shared();
        let s = state.borrow();
        assert!(s.upload_calls.is_empty());
        assert!(s.render_calls.is_empty());
        assert_eq!(s.clear_scene_calls, 0);
        assert_eq!(s.invalidate_sizing_calls, 0);
        assert_eq!(s.install_scene_calls, 0);
        assert!(s.scene_bounds_value.is_none());
        assert!(s.install_scene_returns);
        assert!(s.next_upload_results.is_empty());
    }

    #[test]
    fn upload_scene_records_a_clone_and_returns_ok_by_default() {
        let state = MockRendererState::new_shared();
        let mut mock = MockRenderer::new(state.clone());
        let scene = Scene::new();
        let result = mock.upload_scene(&scene);
        assert!(result.is_ok());
        let s = state.borrow();
        assert_eq!(s.upload_calls.len(), 1);
        assert_eq!(s.upload_calls[0], scene);
    }

    #[test]
    fn upload_scene_pops_scripted_error_before_falling_back_to_ok() {
        let state = MockRendererState::new_shared();
        state
            .borrow_mut()
            .next_upload_results
            .push_back(Err(anyhow::anyhow!("scripted failure")));
        let mut mock = MockRenderer::new(state.clone());
        let scene = Scene::new();
        let first = mock.upload_scene(&scene);
        let second = mock.upload_scene(&scene);
        assert!(first.is_err());
        assert!(second.is_ok());
        assert_eq!(state.borrow().upload_calls.len(), 2);
    }

    #[test]
    fn render_captures_scene_presence_and_sizing() {
        let state = MockRendererState::new_shared();
        let mut mock = MockRenderer::new(state.clone());
        let camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        let bv = BuildVolume::new(100.0, 60.0, 80.0);
        let texture = mock.render(None, bv, &camera, 800, 600);
        assert!(texture.is_none());
        let s = state.borrow();
        assert_eq!(s.render_calls.len(), 1);
        assert!(!s.render_calls[0].scene_present);
        assert_eq!(s.render_calls[0].selection, None);
        assert!(!s.render_calls[0].gizmo_present);
        assert_eq!(s.render_calls[0].width, 800);
        assert_eq!(s.render_calls[0].height, 600);
    }
}
