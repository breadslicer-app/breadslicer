/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Main mesh pipeline: renders every scene object's triangles into the
//! MSAA color attachment and writes the per-object selection bit
//! (`stencil_reference == 1`) into the stencil aspect for the
//! outline post-pass to read.

use std::borrow::Cow;
use std::num::NonZeroU64;

use slint::wgpu_30::wgpu;

use super::super::device_ops::scoped;
use super::super::frame::Uniforms;
use super::super::scene_upload::{GpuVertex, MODEL_ENTRY_SIZE};
use super::super::{DEPTH_FORMAT, SAMPLE_COUNT, TEXTURE_FORMAT};

const MESH_SHADER_SOURCE: &str = include_str!("../shaders/mesh_render.wgsl");

/// Bind-group indices for the mesh pipeline. Variant order must match
/// the `bind_group_layouts: &[…]` ordering passed to
/// `create_pipeline_layout` and the `@group(N)` decls in
/// `shaders/mesh_render.wgsl`.
#[repr(u32)]
#[derive(Clone, Copy)]
pub(in crate::mesh_render) enum MeshBindGroup {
    Uniforms = 0,
    Models = 1,
}

impl MeshBindGroup {
    pub(in crate::mesh_render) fn idx(self) -> u32 {
        self as u32
    }
}

pub(in crate::mesh_render) struct MeshPipeline {
    pub(in crate::mesh_render) pipeline: wgpu::RenderPipeline,
    /// The cuts the hidden rafts leave in the support meshes (#481): the
    /// main pipeline with the front faces culled, so a support mesh's
    /// inside draws through `fs_cap` where the raft no longer covers it.
    pub(in crate::mesh_render) cap_pipeline: wgpu::RenderPipeline,
    /// Preview overlays: the mesh shader's `fs_overlay`, no culling, depth
    /// neither tested nor written, stencil untouched, alpha-blended.
    pub(in crate::mesh_render) overlay_pipeline: wgpu::RenderPipeline,
    pub(in crate::mesh_render) uniform_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) uniforms_bind_group: wgpu::BindGroup,
    /// Layout of the per-draw-call dynamic-offset models bind group.
    /// Held by the pipeline because every newly attached `GpuScene`
    /// needs a fresh bind group built against this layout (the scene's
    /// storage buffer is referenced by handle).
    pub(in crate::mesh_render) models_bind_group_layout: wgpu::BindGroupLayout,
}

impl MeshPipeline {
    pub(in crate::mesh_render) fn create(device: &wgpu::Device) -> Result<Self, wgpu::Error> {
        scoped(device, || Self::create_unscoped(device))
    }

    fn create_unscoped(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(MESH_SHADER_SOURCE)),
        });

        let uniforms_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mesh-viewer uniforms bgl"),
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

        let models_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mesh-viewer models bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        // One ModelEntry per draw call, addressed by a
                        // dynamic offset. Uniform-buffers-with-dynamic-
                        // offset are available under default-limits on
                        // every wgpu backend; storage buffers in the
                        // vertex stage are not (they are gated by
                        // `max_storage_buffers_per_shader_stage`, which
                        // is 0 in the downlevel default profile Slint
                        // currently uses).
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(MODEL_ENTRY_SIZE),
                    },
                    count: None,
                }],
            });

        debug_assert_eq!(MeshBindGroup::Uniforms.idx(), 0);
        debug_assert_eq!(MeshBindGroup::Models.idx(), 1);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer pll"),
            bind_group_layouts: &[
                Some(&uniforms_bind_group_layout),
                Some(&models_bind_group_layout),
            ],
            immediate_size: 0,
        });

        let solid_pipeline = |label: &'static str, cull: wgpu::Face, fragment: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
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
                    cull_mode: Some(cull),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    // Every fragment that passes depth replaces the stencil
                    // with the per-draw `stencil_reference` value (set by
                    // the render loop to 1 for the selected object and 0
                    // otherwise). This means after the main pass the
                    // stencil holds 1 only over the selected object's
                    // visible silhouette, exactly what the outline
                    // post-pass dilates.
                    stencil: wgpu::StencilState {
                        front: wgpu::StencilFaceState {
                            compare: wgpu::CompareFunction::Always,
                            fail_op: wgpu::StencilOperation::Keep,
                            depth_fail_op: wgpu::StencilOperation::Keep,
                            pass_op: wgpu::StencilOperation::Replace,
                        },
                        back: wgpu::StencilFaceState {
                            compare: wgpu::CompareFunction::Always,
                            fail_op: wgpu::StencilOperation::Keep,
                            depth_fail_op: wgpu::StencilOperation::Keep,
                            pass_op: wgpu::StencilOperation::Replace,
                        },
                        read_mask: 0xFF,
                        write_mask: 0xFF,
                    },
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TEXTURE_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = solid_pipeline("mesh-viewer pipeline", wgpu::Face::Back, "fs_main");
        let cap_pipeline = solid_pipeline(
            "mesh-viewer support cap pipeline",
            wgpu::Face::Front,
            "fs_cap",
        );

        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer overlay pipeline"),
            layout: Some(&pipeline_layout),
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
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState {
                    front: wgpu::StencilFaceState::IGNORE,
                    back: wgpu::StencilFaceState::IGNORE,
                    read_mask: 0,
                    write_mask: 0,
                },
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_overlay"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TEXTURE_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer uniforms bg"),
            layout: &uniforms_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            cap_pipeline,
            overlay_pipeline,
            uniform_buffer,
            uniforms_bind_group,
            models_bind_group_layout,
        }
    }
}

#[cfg(test)]
mod bind_group_index_tests {
    use super::*;

    #[test]
    fn mesh_bind_group_indices_match_wgsl() {
        // shaders/mesh_render.wgsl: @group(0) → uniforms, @group(1) → model_entry.
        assert_eq!(MeshBindGroup::Uniforms.idx(), 0);
        assert_eq!(MeshBindGroup::Models.idx(), 1);
    }
}
