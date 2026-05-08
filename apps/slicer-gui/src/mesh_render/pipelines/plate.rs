/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Build-plate render pipeline. Two pipelines share a vertex layout
//! and bind-group layout, differing only in primitive topology and
//! depth-write policy:
//!
//! - `triangle_pipeline`: fills the floor quad. Depth-write off so
//!   models resting at `Z = 0` always win z-fighting against the
//!   plate.
//! - `line_pipeline`: draws the 12 cage edges as 1-pixel lines.
//!   Depth-write on (so the cage is occluded by closer geometry as
//!   expected), `cull_mode = None` (lines have no winding).
//!
//! The pipeline borrows the mesh pipeline's `Uniforms` bind-group
//! layout (the shared `view_proj` slot in `frame::Uniforms`) so we
//! avoid a redundant per-frame uniform write. The plate shader does
//! not read `light_dir`; the unused field is harmless.

use std::borrow::Cow;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use slint::wgpu_28::wgpu;
use wgpu::util::DeviceExt;

use super::super::frame::Uniforms;
use super::super::{DEPTH_FORMAT, SAMPLE_COUNT, TEXTURE_FORMAT};
use super::mesh::MeshPipeline;
use crate::build_volume::BuildVolume;

const PLATE_SHADER_SOURCE: &str = include_str!("../shaders/plate.wgsl");

/// Vertex format shared by floor and cage. `kind` selects the
/// fragment-shader colour path (0 = floor, 1 = cage).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(in crate::mesh_render) struct PlateVertex {
    pub(in crate::mesh_render) position: [f32; 3],
    pub(in crate::mesh_render) kind: u32,
}

const KIND_FLOOR: u32 = 0;
const KIND_CAGE: u32 = 1;

/// Bind-group indices for the plate pipeline. Variant order must
/// match `shaders/plate.wgsl`'s `@group(N)` decls and the layout
/// passed to `create_pipeline_layout`.
#[repr(u32)]
#[derive(Clone, Copy)]
pub(in crate::mesh_render) enum PlateBindGroup {
    Uniforms = 0,
}

impl PlateBindGroup {
    pub(in crate::mesh_render) fn idx(self) -> u32 {
        self as u32
    }
}

pub(in crate::mesh_render) struct PlatePipeline {
    pub(in crate::mesh_render) triangle_pipeline: wgpu::RenderPipeline,
    pub(in crate::mesh_render) line_pipeline: wgpu::RenderPipeline,
    /// Bind group built against this pipeline's own layout, but
    /// pointing at the mesh pipeline's uniform buffer. Reuses the
    /// per-frame `view_proj` write the mesh pass already issues.
    pub(in crate::mesh_render) uniforms_bind_group: wgpu::BindGroup,
}

impl PlatePipeline {
    pub(in crate::mesh_render) fn create(device: &wgpu::Device, mesh: &MeshPipeline) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer plate shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(PLATE_SHADER_SOURCE)),
        });

        // The plate's group(0) is structurally identical to the mesh
        // pipeline's uniforms group, but wgpu compares bind-group
        // layouts by identity, not shape. The plate therefore owns
        // a layout instance and a matching bind group whose buffer
        // resource is the mesh's `uniform_buffer`, so the per-frame
        // `view_proj` write the mesh pass already issues is the only
        // upload either pipeline needs.
        let uniforms_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer plate uniforms bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });

        debug_assert_eq!(PlateBindGroup::Uniforms.idx(), 0);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer plate pll"),
            bind_group_layouts: &[&uniforms_bgl],
            immediate_size: 0,
        });

        let make_pipeline = |label: &'static str,
                             topology: wgpu::PrimitiveTopology,
                             depth_write_enabled: bool|
         -> wgpu::RenderPipeline {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<PlateVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32],
                    }],
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    // Floor quad and cage edges both render as
                    // double-sided: floor so the camera below the
                    // plate still sees the surface, cage because
                    // line primitives have no winding.
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled,
                    depth_compare: wgpu::CompareFunction::Less,
                    // Stencil writes stay at the default keep, so
                    // the plate leaves stencil at 0 and the outline
                    // post-pass ignores it.
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
            })
        };

        let triangle_pipeline = make_pipeline(
            "mesh-viewer plate floor pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            // Floor does not write depth: a model resting at Z = 0
            // always wins z-fighting against the plate.
            false,
        );
        let line_pipeline = make_pipeline(
            "mesh-viewer plate cage pipeline",
            wgpu::PrimitiveTopology::LineList,
            true,
        );

        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer plate uniforms bg"),
            layout: &uniforms_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mesh.uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            triangle_pipeline,
            line_pipeline,
            uniforms_bind_group,
        }
    }
}

/// GPU resources for one specific [`BuildVolume`]: a single vertex
/// buffer holding both the floor quad's four corners and the eight
/// cage corners, plus two index buffers (floor triangles and cage
/// edges). The `build_volume` field is the cache key: when the
/// renderer's per-frame `BuildVolume` differs, the buffers rebuild.
pub(in crate::mesh_render) struct PlateBuffers {
    pub(in crate::mesh_render) build_volume: BuildVolume,
    pub(in crate::mesh_render) vertex_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) floor_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) cage_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) floor_index_count: u32,
    pub(in crate::mesh_render) cage_index_count: u32,
}

impl PlateBuffers {
    pub(in crate::mesh_render) fn create(device: &wgpu::Device, build_volume: BuildVolume) -> Self {
        let (vertices, floor_indices, cage_indices) = plate_geometry(build_volume);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer plate vbo"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let floor_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer plate floor ibo"),
            contents: bytemuck::cast_slice(&floor_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let cage_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer plate cage ibo"),
            contents: bytemuck::cast_slice(&cage_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            build_volume,
            vertex_buffer,
            floor_index_buffer,
            cage_index_buffer,
            floor_index_count: floor_indices.len() as u32,
            cage_index_count: cage_indices.len() as u32,
        }
    }
}

/// Build the per-`BuildVolume` static geometry: 4 floor corners
/// (kind = 0) followed by 8 cage corners (kind = 1), with a
/// triangle-list index pattern for the floor and a line-list index
/// pattern for the 12 cage edges.
///
/// Floor corner order is CCW when viewed from `+Z` (so the front
/// face is up-facing); both sides are visible regardless because
/// the pipeline has `cull_mode = None`.
///
/// Cage corner index layout (low-Z layer 0..3, high-Z layer 4..7):
///
/// ```text
///       7 -------- 6
///      /|         /|
///     4 -------- 5 |
///     | 3 ------ | 2
///     |/         |/
///     0 -------- 1
/// ```
pub(in crate::mesh_render) fn plate_geometry(
    build_volume: BuildVolume,
) -> (Vec<PlateVertex>, Vec<u32>, Vec<u32>) {
    let lo = build_volume.envelope_min_mm();
    let hi = build_volume.envelope_max_mm();

    let mut vertices = Vec::with_capacity(12);

    // Floor quad: 4 corners on Z = 0 (= lo.z by the centred-on-XY,
    // floor-on-Z=0 envelope convention). Indices 0..3.
    vertices.push(PlateVertex {
        position: [lo.x, lo.y, lo.z],
        kind: KIND_FLOOR,
    });
    vertices.push(PlateVertex {
        position: [hi.x, lo.y, lo.z],
        kind: KIND_FLOOR,
    });
    vertices.push(PlateVertex {
        position: [hi.x, hi.y, lo.z],
        kind: KIND_FLOOR,
    });
    vertices.push(PlateVertex {
        position: [lo.x, hi.y, lo.z],
        kind: KIND_FLOOR,
    });

    // Cage corners: 4 on the low-Z plane (= floor plane) and 4 on
    // the high-Z plane (= ceiling). Indices 4..11. Reuses the
    // same XY pattern as the floor corners.
    for &z in &[lo.z, hi.z] {
        vertices.push(PlateVertex {
            position: [lo.x, lo.y, z],
            kind: KIND_CAGE,
        });
        vertices.push(PlateVertex {
            position: [hi.x, lo.y, z],
            kind: KIND_CAGE,
        });
        vertices.push(PlateVertex {
            position: [hi.x, hi.y, z],
            kind: KIND_CAGE,
        });
        vertices.push(PlateVertex {
            position: [lo.x, hi.y, z],
            kind: KIND_CAGE,
        });
    }

    let floor_indices = vec![0u32, 1, 2, 0, 2, 3];

    // 12 cage edges: 4 on the bottom rectangle, 4 on the top
    // rectangle, 4 verticals connecting matching corners.
    let bottom_offset = 4u32;
    let top_offset = 8u32;
    let mut cage_indices: Vec<u32> = Vec::with_capacity(24);
    for i in 0..4u32 {
        let next = (i + 1) % 4;
        cage_indices.push(bottom_offset + i);
        cage_indices.push(bottom_offset + next);
        cage_indices.push(top_offset + i);
        cage_indices.push(top_offset + next);
        cage_indices.push(bottom_offset + i);
        cage_indices.push(top_offset + i);
    }

    (vertices, floor_indices, cage_indices)
}

#[cfg(test)]
mod plate_geometry_tests {
    use super::*;

    #[test]
    fn vertex_count_is_four_floor_plus_eight_cage() {
        let (verts, _, _) = plate_geometry(BuildVolume::new(192.0, 120.0, 200.0));
        assert_eq!(verts.len(), 12);
        // First four are floor (kind = 0); next eight are cage.
        for v in &verts[..4] {
            assert_eq!(v.kind, KIND_FLOOR);
        }
        for v in &verts[4..] {
            assert_eq!(v.kind, KIND_CAGE);
        }
    }

    #[test]
    fn floor_corners_are_centred_on_xy_at_z_zero() {
        let (verts, _, _) = plate_geometry(BuildVolume::new(100.0, 60.0, 80.0));
        // Floor corners at +/- 50 on X, +/- 30 on Y, all at Z = 0.
        for v in &verts[..4] {
            assert_eq!(v.position[2], 0.0);
            assert_eq!(v.position[0].abs(), 50.0);
            assert_eq!(v.position[1].abs(), 30.0);
        }
    }

    #[test]
    fn cage_uses_low_and_high_z_layers() {
        let (verts, _, _) = plate_geometry(BuildVolume::new(100.0, 60.0, 80.0));
        // First four cage verts on Z = 0, last four on Z = H = 80.
        for v in &verts[4..8] {
            assert_eq!(v.position[2], 0.0);
        }
        for v in &verts[8..12] {
            assert_eq!(v.position[2], 80.0);
        }
    }

    #[test]
    fn floor_index_count_is_two_triangles() {
        let (_, floor, _) = plate_geometry(BuildVolume::new(100.0, 60.0, 80.0));
        assert_eq!(floor.len(), 6);
    }

    #[test]
    fn cage_index_count_is_twelve_edges() {
        let (_, _, cage) = plate_geometry(BuildVolume::new(100.0, 60.0, 80.0));
        // Two indices per edge, 12 edges = 24 indices.
        assert_eq!(cage.len(), 24);
    }

    #[test]
    fn cage_indices_reference_only_cage_vertices() {
        let (_, _, cage) = plate_geometry(BuildVolume::new(100.0, 60.0, 80.0));
        for idx in cage {
            assert!((4..12).contains(&idx), "cage index {idx} out of cage range");
        }
    }
}
