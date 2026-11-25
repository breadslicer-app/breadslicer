/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Slice-view cap pipeline. Draws the selected layer's polygons, as
//! triangles, flat at the clip height, to fill the opening left by the
//! mesh clip so the cut reads solid (#776). A review finding's pixels
//! (#409) come after the layer in the same buffer and draw over it.
//!
//! Like the plate pipeline, group 0 reuses the mesh `Uniforms` buffer
//! (`view_proj`, plus the clip height in `light_dir.w`), so no extra
//! per-frame uniform write is needed. The vertex buffer is uploaded by the
//! renderer when the selected layer changes.

use std::borrow::Cow;
use std::num::NonZeroU64;

use slint::wgpu_30::wgpu;

use super::super::device_ops::scoped;
use super::super::frame::Uniforms;
use super::super::{DEPTH_FORMAT, SAMPLE_COUNT, TEXTURE_FORMAT};
use super::mesh::MeshPipeline;

const CAP_SHADER_SOURCE: &str = include_str!("../shaders/cap.wgsl");

pub(in crate::mesh_render) struct CapPipeline {
    pub(in crate::mesh_render) pipeline: wgpu::RenderPipeline,
    pub(in crate::mesh_render) uniforms_bind_group: wgpu::BindGroup,
}

impl CapPipeline {
    pub(in crate::mesh_render) fn create(
        device: &wgpu::Device,
        mesh: &MeshPipeline,
    ) -> Result<Self, wgpu::Error> {
        scoped(device, || Self::create_unscoped(device, mesh))
    }

    fn create_unscoped(device: &wgpu::Device, mesh: &MeshPipeline) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer cap shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(CAP_SHADER_SOURCE)),
        });

        // Group 0: structurally identical to the mesh uniforms group.
        // wgpu compares layouts by identity, so the cap owns its own
        // layout instance and a bind group over the mesh's uniform
        // buffer (see the plate pipeline for the same arrangement).
        let uniforms_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer cap uniforms bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer cap pll"),
            bind_group_layouts: &[Some(&uniforms_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer cap pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                // `[x, y, highlight]` per vertex.
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                // The cap is flat and seen from either side.
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                // A finding's pixels lie on the layer, at the same depth,
                // and are drawn after it.
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                // Stencil stays at the default keep, so the cap leaves
                // the selection-outline stencil untouched.
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
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer cap uniforms bg"),
            layout: &uniforms_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mesh.uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            uniforms_bind_group,
        }
    }
}
