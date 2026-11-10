/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Front-label pipeline. Draws the word the plate's front tab carries
//! as one quad at `Z = 0`, textured with the label's coverage raster
//! and blended over the tab underneath, and a mirrored copy on the
//! tab's underside. Each quad faces out of its own face and back faces
//! are culled, so from above only the top one shows and from below
//! only the underside one, which reads the right way round.
//!
//! Group 0 reuses the mesh `Uniforms` buffer, as the plate and cap
//! pipelines do; group 1 is the coverage texture, uploaded when the
//! label text changes. The quad rides its own vertex buffer: unlike
//! the cap's, its rectangle comes from the tab rather than from the
//! build envelope the uniforms carry.

use std::borrow::Cow;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use slint::wgpu_30::wgpu;

use super::super::device_ops::scoped;
use super::super::frame::Uniforms;
use super::super::{DEPTH_FORMAT, SAMPLE_COUNT, TEXTURE_FORMAT};
use super::mesh::MeshPipeline;

const LABEL_SHADER_SOURCE: &str = include_str!("../shaders/label.wgsl");

/// Corners of the label quad, in world millimetres with the coverage
/// raster's texture coordinates.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(in crate::mesh_render) struct LabelVertex {
    pub(in crate::mesh_render) position: [f32; 3],
    pub(in crate::mesh_render) uv: [f32; 2],
}

/// Builds the two triangles covering `text_box`
/// (`[min_x, min_y, max_x, max_y]` in world mm) at the plate's
/// surface, with `v` running up from the raster's bottom row.
pub(in crate::mesh_render) fn label_quad(text_box: [f32; 4]) -> [LabelVertex; 6] {
    let [min_x, min_y, max_x, max_y] = text_box;
    let corner = |x: f32, y: f32, u: f32, v: f32| LabelVertex {
        position: [x, y, 0.0],
        uv: [u, v],
    };
    let bottom_left = corner(min_x, min_y, 0.0, 0.0);
    let bottom_right = corner(max_x, min_y, 1.0, 0.0);
    let top_right = corner(max_x, max_y, 1.0, 1.0);
    let top_left = corner(min_x, max_y, 0.0, 1.0);
    [
        bottom_left,
        bottom_right,
        top_right,
        bottom_left,
        top_right,
        top_left,
    ]
}

/// Builds the copy of [`label_quad`] read from below: the same box at
/// height `z`, mirrored in `v` so the word reads the right way round
/// from underneath (looking up, the plate's front edge is at the top
/// of the view), and wound counter-clockwise as seen from below.
/// The quad of a ruler's number: its ink box in world mm at `Z = 0`, and
/// where it is in the raster, `[u0, v0, u1, v1]`.
pub(in crate::mesh_render) fn mark_quad(world: [f32; 4], uv: [f32; 4]) -> [LabelVertex; 6] {
    let [min_x, min_y, max_x, max_y] = world;
    let [u0, v0, u1, v1] = uv;
    let corner = |x: f32, y: f32, u: f32, v: f32| LabelVertex {
        position: [x, y, 0.0],
        uv: [u, v],
    };
    let bottom_left = corner(min_x, min_y, u0, v0);
    let bottom_right = corner(max_x, min_y, u1, v0);
    let top_right = corner(max_x, max_y, u1, v1);
    let top_left = corner(min_x, max_y, u0, v1);
    [
        bottom_left,
        bottom_right,
        top_right,
        bottom_left,
        top_right,
        top_left,
    ]
}

pub(in crate::mesh_render) fn underside_label_quad(text_box: [f32; 4], z: f32) -> [LabelVertex; 6] {
    let [min_x, min_y, max_x, max_y] = text_box;
    let corner = |x: f32, y: f32, u: f32, v: f32| LabelVertex {
        position: [x, y, z],
        uv: [u, v],
    };
    let bottom_left = corner(min_x, min_y, 0.0, 1.0);
    let bottom_right = corner(max_x, min_y, 1.0, 1.0);
    let top_right = corner(max_x, max_y, 1.0, 0.0);
    let top_left = corner(min_x, max_y, 0.0, 0.0);
    [
        bottom_left,
        top_right,
        bottom_right,
        bottom_left,
        top_left,
        top_right,
    ]
}

pub(in crate::mesh_render) struct LabelPipeline {
    pub(in crate::mesh_render) pipeline: wgpu::RenderPipeline,
    pub(in crate::mesh_render) uniforms_bind_group: wgpu::BindGroup,
    pub(in crate::mesh_render) texture_bind_group_layout: wgpu::BindGroupLayout,
    pub(in crate::mesh_render) sampler: wgpu::Sampler,
}

impl LabelPipeline {
    pub(in crate::mesh_render) fn create(
        device: &wgpu::Device,
        mesh: &MeshPipeline,
    ) -> Result<Self, wgpu::Error> {
        scoped(device, || Self::create_unscoped(device, mesh))
    }

    fn create_unscoped(device: &wgpu::Device, mesh: &MeshPipeline) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer label shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LABEL_SHADER_SOURCE)),
        });

        // Group 0: structurally identical to the mesh uniforms group,
        // over the mesh's own buffer (see the plate and cap pipelines
        // for the same arrangement).
        let uniforms_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer label uniforms bgl"),
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

        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer label texture bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer label pll"),
            bind_group_layouts: &[Some(&uniforms_bgl), Some(&texture_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer label pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<LabelVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                // The top quad faces up and the underside one down: each
                // shows only from its own side of the plate.
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // Sits on the tab's own surface, so it neither writes
                // depth nor loses the tie against it.
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
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
                    // Coverage is an alpha mask, so the glyph edges
                    // blend into the tab instead of stepping.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer label uniforms bg"),
            layout: &uniforms_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mesh.uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            uniforms_bind_group,
            texture_bind_group_layout: texture_bgl,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("mesh-viewer label sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }
}

#[cfg(test)]
mod label_quad_tests {
    use super::*;

    #[test]
    fn the_quad_spans_the_text_box_at_the_plate_surface() {
        let quad = label_quad([-10.0, -70.0, 10.0, -64.0]);

        for corner in &quad {
            assert_eq!(corner.position[2], 0.0);
            assert!((-10.0..=10.0).contains(&corner.position[0]));
            assert!((-70.0..=-64.0).contains(&corner.position[1]));
        }
        assert!(quad.iter().any(|c| c.position[0] == -10.0));
        assert!(quad.iter().any(|c| c.position[0] == 10.0));
        assert!(quad.iter().any(|c| c.position[1] == -70.0));
        assert!(quad.iter().any(|c| c.position[1] == -64.0));
    }

    #[test]
    fn texture_coordinates_run_with_the_world_axes() {
        // Row 0 of the raster is the bottom of the word, so v has to
        // climb with +Y, and u with +X.
        for corner in label_quad([-10.0, -70.0, 10.0, -64.0]) {
            assert_eq!(
                corner.uv[0],
                if corner.position[0] < 0.0 { 0.0 } else { 1.0 }
            );
            assert_eq!(
                corner.uv[1],
                if corner.position[1] < -67.0 { 0.0 } else { 1.0 }
            );
        }
    }

    /// The turn of `triangle` seen from above: positive when it winds
    /// counter-clockwise.
    fn turn_from_above(triangle: &[LabelVertex]) -> f32 {
        let [a, b, c] = [
            triangle[0].position,
            triangle[1].position,
            triangle[2].position,
        ];
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    }

    #[test]
    fn the_underside_copy_faces_down_and_is_mirrored_in_v() {
        let quad = underside_label_quad([-10.0, -70.0, 10.0, -64.0], -4.0);

        for corner in &quad {
            assert_eq!(corner.position[2], -4.0);
            // Mirrored in v: the top of the word points to the plate's
            // front edge, which is at the top of the view from below;
            // the word still runs with +X.
            assert_eq!(
                corner.uv[0],
                if corner.position[0] < 0.0 { 0.0 } else { 1.0 }
            );
            assert_eq!(
                corner.uv[1],
                if corner.position[1] < -67.0 { 1.0 } else { 0.0 }
            );
        }
        for triangle in quad.chunks(3) {
            assert!(turn_from_above(triangle) < 0.0, "faces down");
        }
    }

    #[test]
    fn both_triangles_wind_counter_clockwise_from_above() {
        let quad = label_quad([-10.0, -70.0, 10.0, -64.0]);

        for triangle in quad.chunks(3) {
            let [a, b, c] = [
                triangle[0].position,
                triangle[1].position,
                triangle[2].position,
            ];
            let turn = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert!(turn > 0.0);
        }
    }
}
