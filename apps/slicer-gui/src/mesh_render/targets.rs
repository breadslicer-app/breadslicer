/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Viewport-sized GPU resources owned by the renderer: the MSAA
//! attachments (color + depth/stencil + private gizmo depth) and the
//! double-buffered destination color textures handed off to Slint.
//!
//! Both bundles are recreated whenever `(width, height)` changes; the
//! `matches` helpers let the renderer cheaply skip the rebuild when the
//! cached size already matches.

use slint::wgpu_29::wgpu;

use super::{DEPTH_FORMAT, GIZMO_DEPTH_FORMAT, SAMPLE_COUNT, TEXTURE_FORMAT};

pub(super) struct MsaaTargets {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) color_view: wgpu::TextureView,
    pub(super) depth_view: wgpu::TextureView,
    /// Stencil-aspect view of the depth-stencil texture, bound by the
    /// outline post-pass as a multisampled u32 texture. Recreated
    /// alongside `depth_view` since they share the underlying texture.
    pub(super) outline_bind_group: wgpu::BindGroup,
    /// Private depth attachment for the gizmo pass. The gizmo
    /// renders to the resolved (non-MSAA) target with depth cleared
    /// at pass start so it sits on top of the mesh, but the gizmo
    /// pipeline still runs depth-test-on internally so the three
    /// axes order correctly where they overlap near the pivot. We
    /// can't reuse the main MSAA depth: its sample count (4)
    /// doesn't match the gizmo pipeline's count (1).
    pub(super) gizmo_depth_view: wgpu::TextureView,
}

impl MsaaTargets {
    pub(super) fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    pub(super) fn create(
        device: &wgpu::Device,
        outline_bind_group_layout: &wgpu::BindGroupLayout,
        width: u32,
        height: u32,
    ) -> Self {
        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mesh-viewer msaa color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mesh-viewer msaa depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            // TEXTURE_BINDING lets the outline post-pass sample the
            // stencil aspect after the main pass writes it.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let stencil_view = depth_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("mesh-viewer msaa stencil view"),
            aspect: wgpu::TextureAspect::StencilOnly,
            ..Default::default()
        });

        let outline_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer outline bg"),
            layout: outline_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&stencil_view),
            }],
        });

        let gizmo_depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mesh-viewer gizmo depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: GIZMO_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let gizmo_depth_view = gizmo_depth.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            width,
            height,
            color_view,
            depth_view,
            outline_bind_group,
            gizmo_depth_view,
        }
    }
}

/// Double-buffered ring of destination color textures. `render` writes
/// to one texture while the `slint::Image` returned last frame (which
/// wraps the *other* texture) is still being sampled by Slint.
/// Retaining both across frames avoids the per-frame GPU-allocator
/// churn of freshly creating a `RENDER_ATTACHMENT | TEXTURE_BINDING`
/// texture on every orbit tick.
pub(super) struct TargetPool {
    pub(super) width: u32,
    pub(super) height: u32,
    textures: [wgpu::Texture; 2],
    next: usize,
}

impl TargetPool {
    pub(super) fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    pub(super) fn create(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            textures: [
                create_color_target(device, width, height),
                create_color_target(device, width, height),
            ],
            next: 0,
        }
    }

    /// Hand out the next destination texture and advance the ring index.
    pub(super) fn acquire(&mut self) -> wgpu::Texture {
        let idx = self.next;
        self.next = (idx + 1) % self.textures.len();
        self.textures[idx].clone()
    }
}

fn create_color_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    // COPY_SRC is included so a debug screenshot or a test-time
    // readback path can copy the resolved frame back to the CPU. The
    // flag is free at runtime, the driver only widens the texture's
    // allowed-operations mask.
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mesh-viewer color"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
