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
use sc_scene::Scene;
use slint::wgpu_30::wgpu;

use super::scene_upload::GpuScene;
use super::{FramePixels, RenderError, Renderer, SceneRender};
use crate::build_volume::BuildVolume;
use crate::camera::OrbitCamera;
use crate::front_label::LabelRaster;
use crate::slice_cap::CapRaster;

pub trait MeshRenderer {
    /// UI-thread upload path; rebuilds GPU buffers from the CPU-side
    /// scene against this renderer's device.
    fn upload_scene(&mut self, graph: &Scene) -> Result<()>;

    /// Install a [`GpuScene`] built on a worker thread, but only when
    /// its `device_token` matches this renderer's. Returns `false` if
    /// the device has rotated since the worker started, in which case
    /// the caller falls back to [`MeshRenderer::upload_scene`].
    fn install_scene(&mut self, gpu: GpuScene) -> bool;

    /// Keep the mesh buffers of a [`GpuScene`] built on a worker for
    /// models about to join the scene, for the next
    /// [`MeshRenderer::upload_scene`] to reuse. Returns `false`, keeping
    /// nothing, when it was built on another device.
    fn adopt_meshes(&mut self, gpu: GpuScene) -> bool;

    /// World-space AABB of the currently installed scene, or `None`
    /// when no scene has been installed yet.
    fn scene_bounds(&self) -> Option<(Vec3, Vec3)>;

    /// Drop the installed scene plus its viewport-sized allocations.
    fn clear_scene(&mut self);

    /// Drop the viewport-sized allocations so the next render
    /// reallocates them at the current exact pixel size.
    fn invalidate_sizing(&mut self);

    /// Refresh the slice-view cap texture for the layer identified by
    /// `key` (`None` clears it). `build` is called to produce the
    /// coverage raster only when `key` differs from the current cap;
    /// a refused upload leaves the cap off for that key.
    fn update_cap(
        &mut self,
        key: Option<u64>,
        build: &mut dyn FnMut() -> Option<CapRaster>,
    ) -> Result<(), RenderError>;

    /// Refresh the plate's front label for the word identified by
    /// `key`. `build` is called to rasterise it only when `key`
    /// differs from the current label; without a label the plate
    /// carries no tab either.
    fn update_front_label(
        &mut self,
        key: u64,
        build: &mut dyn FnMut() -> Option<LabelRaster>,
    ) -> Result<(), RenderError>;

    /// Run the per-frame render pass. Returns `Ok(None)` when the
    /// viewport is zero-sized. The texture may be smaller than
    /// `(width, height)` when that exceeds the device's texture
    /// ceiling; the aspect is kept.
    fn render(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<wgpu::Texture>, RenderError>;

    /// Install the preview overlay pieces drawn over the scene (`None`
    /// clears them). Re-uploads only when `pieces` is a different `Arc`.
    fn set_overlay(&mut self, _pieces: Option<&super::Overlay>) {}

    /// Paints the models of the workspace frames to come by overhang (`None`
    /// turns the map off).
    fn set_overhang_map(&mut self, _map: Option<super::OverhangMap>) {}

    /// Blocks until the GPU has finished the work submitted so far, so a
    /// caller can time a frame end to end (the performance test).
    fn wait_for_gpu(&self) {}

    /// Draw the models alone and copy the frame off the GPU, for a
    /// picture that is not going on screen, such as the one a printer
    /// file carries. Returns `Ok(None)` for a zero-sized request.
    fn preview_pixels(
        &mut self,
        graph: &Scene,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<FramePixels>, RenderError>;
}

impl MeshRenderer for Renderer {
    fn upload_scene(&mut self, graph: &Scene) -> Result<()> {
        Renderer::upload_scene(self, graph)
    }

    fn install_scene(&mut self, gpu: GpuScene) -> bool {
        Renderer::install_scene(self, gpu)
    }

    fn adopt_meshes(&mut self, gpu: GpuScene) -> bool {
        Renderer::adopt_meshes(self, gpu)
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

    fn update_cap(
        &mut self,
        key: Option<u64>,
        build: &mut dyn FnMut() -> Option<CapRaster>,
    ) -> Result<(), RenderError> {
        Renderer::update_cap(self, key, build)
    }

    fn update_front_label(
        &mut self,
        key: u64,
        build: &mut dyn FnMut() -> Option<LabelRaster>,
    ) -> Result<(), RenderError> {
        Renderer::update_front_label(self, key, build)
    }

    fn render(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<wgpu::Texture>, RenderError> {
        Renderer::render(self, scene_render, build_volume, camera, width, height)
    }

    fn preview_pixels(
        &mut self,
        graph: &Scene,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<FramePixels>, RenderError> {
        Renderer::preview_pixels(self, graph, build_volume, camera, width, height)
    }

    fn set_overlay(&mut self, pieces: Option<&super::Overlay>) {
        Renderer::set_overlay(self, pieces);
    }

    fn set_overhang_map(&mut self, map: Option<super::OverhangMap>) {
        Renderer::set_overhang_map(self, map);
    }

    fn wait_for_gpu(&self) {
        Renderer::wait_for_gpu(self);
    }
}
