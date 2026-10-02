/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Selection-outline post-pass: a fullscreen edge-detect over the
//! stencil aspect written by the mesh pipeline. Fragments whose
//! stencil neighbourhood crosses the selected/non-selected boundary
//! (1↔0) keep their colour write; the rest `discard`, leaving the
//! resolved mesh-pass colour intact.

use std::borrow::Cow;

use slint::wgpu_30::wgpu;

use super::super::TEXTURE_FORMAT;
use super::super::device_ops::scoped;

const OUTLINE_SHADER_SOURCE: &str = include_str!("../shaders/outline.wgsl");

/// Bind-group indices for the outline post-pass pipeline. Variant
/// order must match `shaders/outline.wgsl`.
#[repr(u32)]
#[derive(Clone, Copy)]
pub(in crate::mesh_render) enum OutlineBindGroup {
    Stencil = 0,
}

impl OutlineBindGroup {
    pub(in crate::mesh_render) fn idx(self) -> u32 {
        self as u32
    }
}

pub(in crate::mesh_render) struct OutlinePipeline {
    pub(in crate::mesh_render) pipeline: wgpu::RenderPipeline,
    /// Layout of the stencil-texture bind group. Held by the
    /// pipeline because every newly sized `MsaaTargets` rebuilds
    /// its outline bind group against this layout (the multisampled
    /// stencil view is recreated alongside the depth attachment).
    pub(in crate::mesh_render) bind_group_layout: wgpu::BindGroupLayout,
}

impl OutlinePipeline {
    pub(in crate::mesh_render) fn create(device: &wgpu::Device) -> Result<Self, wgpu::Error> {
        scoped(device, || Self::create_unscoped(device))
    }

    fn create_unscoped(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer outline shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(OUTLINE_SHADER_SOURCE)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer outline bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    // Stencil reads as Uint. The depth-stencil
                    // texture is bound via a StencilOnly aspect
                    // view; the fragment shader sees a u32 sample.
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: true,
                },
                count: None,
            }],
        });

        debug_assert_eq!(OutlineBindGroup::Stencil.idx(), 0);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer outline pll"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer outline pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            // The post-pass renders to the resolved (non-MSAA) target
            // and samples the still-MSAA stencil as input, colour and
            // attachment sample counts agree at 1, while the texture
            // binding is multisampled.
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TEXTURE_FORMAT,
                    // Outline fragments are opaque; non-edge fragments
                    // `discard` so the underlying main-pass colour
                    // stays put without needing alpha blending.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
        }
    }
}

#[cfg(test)]
mod bind_group_index_tests {
    use super::*;

    #[test]
    fn outline_bind_group_indices_match_wgsl() {
        // shaders/outline.wgsl: @group(0) → stencil texture.
        assert_eq!(OutlineBindGroup::Stencil.idx(), 0);
    }
}
