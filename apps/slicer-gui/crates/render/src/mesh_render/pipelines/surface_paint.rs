/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Paint pipelines (#183): the tint over a model's painted faces and the
//! brush's ring on its surface. Both read the model's own vertex buffer and
//! draw after the models, testing depth with `LessEqual` and a small bias so
//! they land exactly on the surface drawn before them, writing neither depth
//! nor stencil. No storage buffers and no change to the vertex format: they
//! stay within the downlevel limits the mesh pipeline keeps to.

use std::borrow::Cow;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use slint::wgpu_30::wgpu;

use super::super::device_ops::scoped;
use super::super::frame::Uniforms;
use super::super::scene_upload::GpuVertex;
use super::super::{DEPTH_FORMAT, SAMPLE_COUNT, TEXTURE_FORMAT};
use super::mesh::MeshPipeline;

const SHADER_SOURCE: &str = include_str!("../shaders/surface_paint.wgsl");

/// The fragment shaders' parameters: `PaintParams` in the shader.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
pub(in crate::mesh_render) struct PaintParams {
    pub(in crate::mesh_render) tint: [f32; 4],
    pub(in crate::mesh_render) ring: [f32; 4],
    pub(in crate::mesh_render) ring_color: [f32; 4],
}

pub(in crate::mesh_render) struct SurfacePaintPipeline {
    pub(in crate::mesh_render) tint: wgpu::RenderPipeline,
    pub(in crate::mesh_render) ring: wgpu::RenderPipeline,
    pub(in crate::mesh_render) uniforms_bind_group: wgpu::BindGroup,
    pub(in crate::mesh_render) params: wgpu::Buffer,
    pub(in crate::mesh_render) params_bind_group: wgpu::BindGroup,
}

impl SurfacePaintPipeline {
    pub(in crate::mesh_render) fn create(
        device: &wgpu::Device,
        mesh: &MeshPipeline,
    ) -> Result<Self, wgpu::Error> {
        scoped(device, || Self::create_unscoped(device, mesh))
    }

    fn create_unscoped(device: &wgpu::Device, mesh: &MeshPipeline) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer paint shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER_SOURCE)),
        });

        // Group 0: structurally the mesh uniforms group; wgpu compares
        // layouts by identity, so it has its own over the mesh's buffer.
        let uniforms_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer paint uniforms bgl"),
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
        let params_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer paint params bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<PaintParams>() as u64),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer paint pll"),
            bind_group_layouts: &[
                Some(&uniforms_bgl),
                Some(&mesh.models_bind_group_layout),
                Some(&params_bgl),
            ],
            immediate_size: 0,
        });
        let pipeline = |label: &'static str, entry_point: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<GpuVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState {
                        front: wgpu::StencilFaceState::IGNORE,
                        back: wgpu::StencilFaceState::IGNORE,
                        read_mask: 0,
                        write_mask: 0,
                    },
                    bias: wgpu::DepthBiasState {
                        constant: -2,
                        slope_scale: -1.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry_point),
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
        let tint = pipeline("mesh-viewer paint pipeline", "fs_tint");
        let ring = pipeline("mesh-viewer brush ring pipeline", "fs_ring");

        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer paint uniforms bg"),
            layout: &uniforms_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mesh.uniform_buffer.as_entire_binding(),
            }],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer paint params"),
            size: size_of::<PaintParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer paint params bg"),
            layout: &params_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }],
        });
        Self {
            tint,
            ring,
            uniforms_bind_group,
            params,
            params_bind_group,
        }
    }
}
