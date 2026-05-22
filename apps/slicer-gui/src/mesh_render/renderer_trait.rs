/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Internal seam between the viewport controller and the GPU-backed
//! [`Renderer`]. The controller and render-host call the renderer
//! through this trait instead of the concrete type so unit tests can
//! substitute a non-GPU stand-in. Visibility is `pub(crate)`, so the
//! seam is closed: only impls inside this crate are reachable.

use anyhow::Result;
use glam::Vec3;
use sc_mesh_core::scene::Scene;
use slint::wgpu_28::wgpu;

use super::scene_upload::GpuScene;
use super::{Renderer, SceneRender};
use crate::build_volume::BuildVolume;
use crate::camera::OrbitCamera;

pub(crate) trait MeshRenderer {
    /// UI-thread upload path; rebuilds GPU buffers from the CPU-side
    /// scene against this renderer's device.
    fn upload_scene(&mut self, graph: &Scene) -> Result<()>;

    /// Install a [`GpuScene`] built on a worker thread, but only when
    /// its `device_token` matches this renderer's. Returns `false` if
    /// the device has rotated since the worker started, in which case
    /// the caller falls back to [`MeshRenderer::upload_scene`].
    fn install_scene(&mut self, gpu: GpuScene) -> bool;

    /// World-space AABB of the currently installed scene, or `None`
    /// when no scene has been installed yet.
    fn scene_bounds(&self) -> Option<(Vec3, Vec3)>;

    /// Drop the installed scene plus its viewport-sized allocations.
    fn clear_scene(&mut self);

    /// Drop the viewport-sized allocations so the next render
    /// reallocates them at the current exact pixel size.
    fn invalidate_sizing(&mut self);

    /// Run the per-frame render pass. Returns the produced texture,
    /// or `None` when the viewport is zero-sized.
    fn render(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Option<wgpu::Texture>;
}

impl MeshRenderer for Renderer {
    fn upload_scene(&mut self, graph: &Scene) -> Result<()> {
        Renderer::upload_scene(self, graph)
    }

    fn install_scene(&mut self, gpu: GpuScene) -> bool {
        Renderer::install_scene(self, gpu)
    }

    fn scene_bounds(&self) -> Option<(Vec3, Vec3)> {
        Renderer::scene_bounds(self)
    }

    fn clear_scene(&mut self) {
        Renderer::clear_scene(self);
    }

    fn invalidate_sizing(&mut self) {
        Renderer::invalidate_sizing(self);
    }

    fn render(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Option<wgpu::Texture> {
        Renderer::render(self, scene_render, build_volume, camera, width, height)
    }
}
