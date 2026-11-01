/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Gizmo overlay pipelines: draw the translate / rotate / scale
//! handles on top of the scene, into the multisampled colour target so
//! they are antialiased, alpha-blended. Three pipelines share a shader,
//! vertex layout, and bind-group layouts:
//!
//! - `halo`: back faces culled, no depth test or write. The dark outline
//!   of every handle and the faint back halves of the rings, under all
//!   handles.
//! - `core`: back faces culled, depth-tested and written, so the handles
//!   order among themselves where they overlap near the pivot.
//! - `flat`: both faces, no depth. The flat marks of a turn.

use std::borrow::Cow;
use std::num::NonZeroU64;

use slint::wgpu_30::wgpu;

use super::super::device_ops::scoped;
use super::super::gizmo_buffers::GizmoBuffers;
use super::super::gizmo_entries::{GIZMO_DRAW_ENTRY_SIZE, GizmoUniformsGpu};
use super::super::{DEPTH_FORMAT, SAMPLE_COUNT, TEXTURE_FORMAT};
use crate::gizmo_geometry::GizmoVertex;

const GIZMO_SHADER_SOURCE: &str = include_str!("../shaders/gizmo.wgsl");

/// Bind-group indices for the gizmo pipeline. Variant order must
/// match `shaders/gizmo.wgsl`.
#[repr(u32)]
#[derive(Clone, Copy)]
pub(in crate::mesh_render) enum GizmoBindGroup {
    Uniforms = 0,
    Entries = 1,
}

impl GizmoBindGroup {
    pub(in crate::mesh_render) fn idx(self) -> u32 {
        self as u32
    }
}

pub(in crate::mesh_render) struct GizmoPipeline {
    pub(in crate::mesh_render) halo_pipeline: wgpu::RenderPipeline,
    pub(in crate::mesh_render) core_pipeline: wgpu::RenderPipeline,
    pub(in crate::mesh_render) flat_pipeline: wgpu::RenderPipeline,
    pub(in crate::mesh_render) uniforms_bind_group: wgpu::BindGroup,
    pub(in crate::mesh_render) entries_bind_group: wgpu::BindGroup,
}

impl GizmoPipeline {
    pub(in crate::mesh_render) fn create(
        device: &wgpu::Device,
        buffers: &GizmoBuffers,
    ) -> Result<Self, wgpu::Error> {
        scoped(device, || Self::create_unscoped(device, buffers))
    }

    fn create_unscoped(device: &wgpu::Device, buffers: &GizmoBuffers) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer gizmo shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(GIZMO_SHADER_SOURCE)),
        });

        let uniforms_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer gizmo uniforms bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<GizmoUniformsGpu>() as u64),
                },
                count: None,
            }],
        });

        let entries_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer gizmo entries bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // Same dynamic-offset uniform pattern as the main
                // pipeline's models bgl: one entry-sized window, the
                // bind group is rebound with the per-axis offset.
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(GIZMO_DRAW_ENTRY_SIZE),
                },
                count: None,
            }],
        });

        debug_assert_eq!(GizmoBindGroup::Uniforms.idx(), 0);
        debug_assert_eq!(GizmoBindGroup::Entries.idx(), 1);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer gizmo pll"),
            bind_group_layouts: &[Some(&uniforms_bgl), Some(&entries_bgl)],
            immediate_size: 0,
        });

        let make_pipeline = |label: &'static str,
                             cull_mode: Option<wgpu::Face>,
                             depth: bool|
         -> wgpu::RenderPipeline {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<GizmoVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                // The pass borrows the main depth-stencil attachment with its
                // depth cleared, so the gizmo is never hidden by the model.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth),
                    depth_compare: Some(if depth {
                        wgpu::CompareFunction::LessEqual
                    } else {
                        wgpu::CompareFunction::Always
                    }),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TEXTURE_FORMAT,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let halo_pipeline = make_pipeline(
            "mesh-viewer gizmo halo pipeline",
            Some(wgpu::Face::Back),
            false,
        );
        let core_pipeline = make_pipeline(
            "mesh-viewer gizmo core pipeline",
            Some(wgpu::Face::Back),
            true,
        );
        let flat_pipeline = make_pipeline("mesh-viewer gizmo flat pipeline", None, false);

        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer gizmo uniforms bg"),
            layout: &uniforms_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffers.uniforms_buffer.as_entire_binding(),
            }],
        });
        let entries_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer gizmo entries bg"),
            layout: &entries_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffers.entries_binding_window(),
            }],
        });

        Self {
            halo_pipeline,
            core_pipeline,
            flat_pipeline,
            uniforms_bind_group,
            entries_bind_group,
        }
    }
}

#[cfg(test)]
mod bind_group_index_tests {
    use super::*;

    #[test]
    fn gizmo_bind_group_indices_match_wgsl() {
        // shaders/gizmo.wgsl: @group(0) → uniforms, @group(1) → entry.
        assert_eq!(GizmoBindGroup::Uniforms.idx(), 0);
        assert_eq!(GizmoBindGroup::Entries.idx(), 1);
    }
}
