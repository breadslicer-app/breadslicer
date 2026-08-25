/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Build-plate render pipeline. Four pipelines share a vertex layout
//! and bind-group layout:
//!
//! - `top_pipeline`: fills the plate top face (inner grid + outer
//!   rounded border) at `Z = 0`. Depth-write off so models resting
//!   at `Z = 0` always win z-fighting against the plate.
//! - `solid_pipeline`: fills the plate sides only. Depth-write on so
//!   the sides occlude content behind them when the camera orbits
//!   low.
//! - `bottom_pipeline`: fills the plate bottom face. Alpha blending
//!   on, depth-write off, and `cull_mode = Back` so the bottom is
//!   only drawn when its front face faces the camera (i.e. from
//!   below); from above the bottom is back-facing and culled, which
//!   keeps the from-above view fully opaque. Drawn last in the main
//!   pass so the cage edges and any meshes already in the
//!   framebuffer blend through the bottom from below.
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
use slint::wgpu_29::wgpu;

use super::super::device_ops::{scoped, upload_static_buffer};
use super::super::error::{RenderAllocation, RenderError};
use super::super::frame::Uniforms;
use super::super::{DEPTH_FORMAT, SAMPLE_COUNT, TEXTURE_FORMAT};
use super::mesh::MeshPipeline;
use crate::build_volume::BuildVolume;

const PLATE_SHADER_SOURCE: &str = include_str!("../shaders/plate.wgsl");

/// Vertex format shared by floor, plate skin, and cage. `kind`
/// selects the fragment-shader colour path.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(in crate::mesh_render) struct PlateVertex {
    pub(in crate::mesh_render) position: [f32; 3],
    pub(in crate::mesh_render) kind: u32,
}

const KIND_FLOOR: u32 = 0;
const KIND_CAGE: u32 = 1;
const KIND_PLATE_TOP_BORDER: u32 = 2;
const KIND_PLATE_SIDE: u32 = 3;
const KIND_PLATE_BOTTOM: u32 = 4;
/// Five quads coplanar with the build envelope's side walls and top.
/// Each fragment of these quads is discarded by the plate shader
/// unless the matching bit of `face_breach_mask.x` is set; when it
/// is set, the fragment draws as part of a diagonal red stripe
/// pattern. The five kinds are numbered consecutively so the shader
/// derives the face index as `kind - KIND_ENVELOPE_WALL_X_NEG`.
const KIND_ENVELOPE_WALL_X_NEG: u32 = 5;
const KIND_ENVELOPE_WALL_X_POS: u32 = 6;
const KIND_ENVELOPE_WALL_Y_NEG: u32 = 7;
const KIND_ENVELOPE_WALL_Y_POS: u32 = 8;
const KIND_ENVELOPE_WALL_Z_POS: u32 = 9;

/// Plate thickness, mm. The plate extends from `Z = 0` (top, the
/// build floor) down to `Z = -PLATE_THICKNESS_MM`.
const PLATE_THICKNESS_MM: f32 = 4.0;
/// How far the plate extends in XY beyond the build envelope, mm.
const PLATE_OVERHANG_MM: f32 = 8.0;
/// Corner radius (XY) for the rounded outer plate edge, mm.
const PLATE_CORNER_RADIUS_MM: f32 = 6.0;
/// Tessellation per quarter-arc on the rounded corners. Higher =
/// smoother corner, more triangles. 8 segments = a 90 degree arc
/// stepped at 11.25 degrees, smooth at default zoom.
const PLATE_CORNER_SEGMENTS: u32 = 8;

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
    /// Top face of the plate (inner grid + outer rounded border).
    /// Depth-write off so models resting at `Z = 0` win the
    /// z-fight against the plate.
    pub(in crate::mesh_render) top_pipeline: wgpu::RenderPipeline,
    /// Plate sides only. Depth-write on so the slab occludes content
    /// behind it when the camera orbits low.
    pub(in crate::mesh_render) solid_pipeline: wgpu::RenderPipeline,
    /// Plate bottom face. Alpha blending on, depth-write off,
    /// `cull_mode = Back` so it only renders when the camera looks
    /// up from below. Drawn last in the main pass so cage and mesh
    /// pixels show through the translucent bottom.
    pub(in crate::mesh_render) bottom_pipeline: wgpu::RenderPipeline,
    pub(in crate::mesh_render) line_pipeline: wgpu::RenderPipeline,
    /// Bind group built against this pipeline's own layout, but
    /// pointing at the mesh pipeline's uniform buffer. Reuses the
    /// per-frame `view_proj` write the mesh pass already issues.
    pub(in crate::mesh_render) uniforms_bind_group: wgpu::BindGroup,
}

impl PlatePipeline {
    pub(in crate::mesh_render) fn create(
        device: &wgpu::Device,
        mesh: &MeshPipeline,
    ) -> Result<Self, wgpu::Error> {
        scoped(device, || Self::create_unscoped(device, mesh))
    }

    fn create_unscoped(device: &wgpu::Device, mesh: &MeshPipeline) -> Self {
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
                    min_binding_size: NonZeroU64::new(size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });

        debug_assert_eq!(PlateBindGroup::Uniforms.idx(), 0);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer plate pll"),
            bind_group_layouts: &[Some(&uniforms_bgl)],
            immediate_size: 0,
        });

        // Alpha blending is always enabled. The plate fragment
        // shader emits `alpha = 1` for every kind except the bottom
        // fan (`KIND_PLATE_BOTTOM`), so the blend is a no-op
        // everywhere except the bottom face, which appears
        // translucent when the camera looks up from below.
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
                        array_stride: size_of::<PlateVertex>() as u64,
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
                    depth_write_enabled: Some(depth_write_enabled),
                    depth_compare: Some(wgpu::CompareFunction::Less),
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
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let top_pipeline = make_pipeline(
            "mesh-viewer plate top pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            // Top face does not write depth: a model resting at
            // Z = 0 always wins z-fighting against the plate.
            false,
        );
        let solid_pipeline = make_pipeline(
            "mesh-viewer plate solid pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            // Sides + bottom write depth so they occlude content
            // behind them when the camera orbits low.
            true,
        );
        let line_pipeline = make_pipeline(
            "mesh-viewer plate cage pipeline",
            wgpu::PrimitiveTopology::LineList,
            true,
        );

        // Bottom face has its own descriptor: alpha blending, no
        // depth write, and back-face culling. The bottom-fan winding
        // is CCW from -Z, so it appears CCW (front-facing) from
        // below the plate and CW (back-facing) from above. With
        // `cull_mode = Back`, the GPU draws it only from below; from
        // above it's culled and the top face stays opaque.
        let bottom_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer plate bottom pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: size_of::<PlateVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32],
                }],
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
                depth_compare: Some(wgpu::CompareFunction::Less),
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
        });

        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer plate uniforms bg"),
            layout: &uniforms_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mesh.uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            top_pipeline,
            solid_pipeline,
            bottom_pipeline,
            line_pipeline,
            uniforms_bind_group,
        }
    }
}

/// GPU resources for one specific [`BuildVolume`]: one vertex buffer
/// holding all plate + cage geometry, plus four index buffers (top
/// face, plate sides, bottom face, cage edges). The bottom face is
/// indexed separately because it draws through its own pipeline.
/// Cloning copies wgpu handles (reference counted), not GPU memory.
#[derive(Clone)]
pub(in crate::mesh_render) struct PlateBuffers {
    pub(in crate::mesh_render) build_volume: BuildVolume,
    pub(in crate::mesh_render) vertex_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) top_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) solid_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) bottom_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) cage_index_buffer: wgpu::Buffer,
    /// Index buffer for the 5 envelope-wall quads. Drawn through
    /// `top_pipeline` after the mesh pass; the plate shader gates
    /// each fragment on the matching bit of `face_breach_mask`.
    pub(in crate::mesh_render) wall_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) top_index_count: u32,
    pub(in crate::mesh_render) solid_index_count: u32,
    pub(in crate::mesh_render) bottom_index_count: u32,
    pub(in crate::mesh_render) cage_index_count: u32,
    pub(in crate::mesh_render) wall_index_count: u32,
}

impl PlateBuffers {
    pub(in crate::mesh_render) fn create(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        build_volume: BuildVolume,
    ) -> Result<Self, RenderError> {
        let refused = |source| RenderError::DeviceRefused {
            what: RenderAllocation::PlateGeometry,
            source,
        };
        let geom = plate_geometry(build_volume);
        let vertex_buffer = upload_static_buffer(
            device,
            queue,
            "mesh-viewer plate vbo",
            bytemuck::cast_slice(&geom.vertices),
            wgpu::BufferUsages::VERTEX,
        )
        .map_err(refused)?;
        let top_index_buffer = upload_static_buffer(
            device,
            queue,
            "mesh-viewer plate top ibo",
            bytemuck::cast_slice(&geom.top_indices),
            wgpu::BufferUsages::INDEX,
        )
        .map_err(refused)?;
        let solid_index_buffer = upload_static_buffer(
            device,
            queue,
            "mesh-viewer plate solid ibo",
            bytemuck::cast_slice(&geom.solid_indices),
            wgpu::BufferUsages::INDEX,
        )
        .map_err(refused)?;
        let bottom_index_buffer = upload_static_buffer(
            device,
            queue,
            "mesh-viewer plate bottom ibo",
            bytemuck::cast_slice(&geom.bottom_indices),
            wgpu::BufferUsages::INDEX,
        )
        .map_err(refused)?;
        let cage_index_buffer = upload_static_buffer(
            device,
            queue,
            "mesh-viewer plate cage ibo",
            bytemuck::cast_slice(&geom.cage_indices),
            wgpu::BufferUsages::INDEX,
        )
        .map_err(refused)?;
        let wall_index_buffer = upload_static_buffer(
            device,
            queue,
            "mesh-viewer plate wall ibo",
            bytemuck::cast_slice(&geom.wall_indices),
            wgpu::BufferUsages::INDEX,
        )
        .map_err(refused)?;
        Ok(Self {
            build_volume,
            vertex_buffer,
            top_index_buffer,
            solid_index_buffer,
            bottom_index_buffer,
            cage_index_buffer,
            wall_index_buffer,
            top_index_count: geom.top_indices.len() as u32,
            solid_index_count: geom.solid_indices.len() as u32,
            bottom_index_count: geom.bottom_indices.len() as u32,
            cage_index_count: geom.cage_indices.len() as u32,
            wall_index_count: geom.wall_indices.len() as u32,
        })
    }
}

/// Bundle returned by [`plate_geometry`]: one vertex buffer plus
/// four index streams (top face, plate sides, bottom face, cage
/// edges). The bottom face is emitted separately so it can be drawn
/// last with its own pipeline.
pub(in crate::mesh_render) struct PlateGeometry {
    pub(in crate::mesh_render) vertices: Vec<PlateVertex>,
    pub(in crate::mesh_render) top_indices: Vec<u32>,
    pub(in crate::mesh_render) solid_indices: Vec<u32>,
    pub(in crate::mesh_render) bottom_indices: Vec<u32>,
    pub(in crate::mesh_render) cage_indices: Vec<u32>,
    pub(in crate::mesh_render) wall_indices: Vec<u32>,
}

/// Build the per-`BuildVolume` static geometry for the 3D plate slab
/// + cage.
///
/// The plate is a rounded-rectangle slab extending `PLATE_OVERHANG_MM`
/// beyond the build envelope in XY, with rounded outer corners of
/// radius `PLATE_CORNER_RADIUS_MM`, occupying
/// `Z in [-PLATE_THICKNESS_MM, 0]`. The top face is split: the inner
/// rectangle that matches the build envelope footprint draws the
/// grid (`KIND_FLOOR`), the rounded ring around it draws plain
/// border colour (`KIND_PLATE_TOP_BORDER`). Sides and bottom share
/// `KIND_PLATE_SIDE` (a darker shade so the slab reads as 3D).
///
/// Vertex buffer layout (n = `PLATE_CORNER_SEGMENTS + 1`, the count
/// of points per quarter-arc; outline length = `4n`):
///
/// - `[0..4]`: inner build-envelope corners at `Z = 0`, kind = floor.
///   The 2 triangles of the inner top face index these.
/// - `[4..8]`: same inner-corner positions duplicated at `Z = 0`
///   with kind = top border. Used as the inner edge of the outer
///   border ring; the duplication keeps every border-ring triangle
///   uniformly border-kind so flat interpolation never bleeds the
///   grid into the rounded ring (the provoking vertex is the
///   first, and a triangle mixing kinds would inherit whichever
///   appeared first in its index list).
/// - `[8..8+4n]`: outer rounded outline at `Z = 0`, kind = top
///   border.
/// - `[8+4n..8+8n]`: same outline points duplicated at `Z = 0`,
///   kind = side (used for the top edge of side panels).
/// - `[8+8n..8+12n]`: outline at `Z = -PLATE_THICKNESS_MM`,
///   kind = side (bottom edge of side panels + bottom face).
/// - `[8+12n]`: bottom-face fan centre at the origin XY,
///   `Z = -PLATE_THICKNESS_MM`, kind = bottom (provoking vertex of
///   every fan triangle, so flat interpolation routes the whole fan
///   through the translucent fragment-shader branch).
/// - `[8+12n+1..8+12n+9]`: 8 cage corners (4 at `Z = 0`, 4 at
///   `Z = H`), kind = cage.
pub(in crate::mesh_render) fn plate_geometry(build_volume: BuildVolume) -> PlateGeometry {
    let lo = build_volume.envelope_min_mm();
    let hi = build_volume.envelope_max_mm();
    let half_w = 0.5 * build_volume.width_mm();
    let half_d = 0.5 * build_volume.depth_mm();
    let outer_w = half_w + PLATE_OVERHANG_MM;
    let outer_d = half_d + PLATE_OVERHANG_MM;
    // Clamp the corner radius so a tiny envelope doesn't produce a
    // self-intersecting outline.
    let max_radius = outer_w.min(outer_d);
    let radius = PLATE_CORNER_RADIUS_MM.min(max_radius);
    let segments = PLATE_CORNER_SEGMENTS;
    let arc_pts = segments + 1;
    let outline_len = (4 * arc_pts) as usize;
    let z_top = 0.0;
    let z_bot = -PLATE_THICKNESS_MM;

    // CCW (viewed from +Z) outline: arc 0 at +X+Y, then top edge,
    // arc 1 at -X+Y, left edge, arc 2 at -X-Y, bottom edge, arc 3
    // at +X-Y, right edge. Each arc is `arc_pts` points; the first
    // point of the next arc forms the implicit straight-edge end of
    // the previous one.
    let mut outline_xy: Vec<[f32; 2]> = Vec::with_capacity(outline_len);
    let corner_centres = [
        (outer_w - radius, outer_d - radius, 0.0_f32),
        (
            -outer_w + radius,
            outer_d - radius,
            std::f32::consts::FRAC_PI_2,
        ),
        (-outer_w + radius, -outer_d + radius, std::f32::consts::PI),
        (
            outer_w - radius,
            -outer_d + radius,
            3.0 * std::f32::consts::FRAC_PI_2,
        ),
    ];
    for (cx, cy, start_angle) in corner_centres {
        for i in 0..arc_pts {
            let t = i as f32 / segments as f32;
            let angle = start_angle + t * std::f32::consts::FRAC_PI_2;
            outline_xy.push([cx + radius * angle.cos(), cy + radius * angle.sin()]);
        }
    }
    debug_assert_eq!(outline_xy.len(), outline_len);

    let mut vertices: Vec<PlateVertex> = Vec::with_capacity(8 + 12 * outline_len + 1 + 8);

    // [0..4]: inner top corners (build envelope footprint), used by
    // the inner grid triangles.
    let inner_corners = [[hi.x, hi.y], [lo.x, hi.y], [lo.x, lo.y], [hi.x, lo.y]];
    for [x, y] in inner_corners {
        vertices.push(PlateVertex {
            position: [x, y, z_top],
            kind: KIND_FLOOR,
        });
    }

    // [4..8]: same inner-corner positions, kind = top border. Used
    // as the inner edge of the rounded ring so each ring triangle
    // is uniformly border-kind.
    let inner_as_border_off = 4u32;
    for [x, y] in inner_corners {
        vertices.push(PlateVertex {
            position: [x, y, z_top],
            kind: KIND_PLATE_TOP_BORDER,
        });
    }

    let outer_top_off = 8u32;
    for [x, y] in &outline_xy {
        vertices.push(PlateVertex {
            position: [*x, *y, z_top],
            kind: KIND_PLATE_TOP_BORDER,
        });
    }

    let side_top_off = outer_top_off + outline_len as u32;
    for [x, y] in &outline_xy {
        vertices.push(PlateVertex {
            position: [*x, *y, z_top],
            kind: KIND_PLATE_SIDE,
        });
    }

    let side_bot_off = side_top_off + outline_len as u32;
    for [x, y] in &outline_xy {
        vertices.push(PlateVertex {
            position: [*x, *y, z_bot],
            kind: KIND_PLATE_SIDE,
        });
    }

    // The bottom-fan provoking vertex (first index of every fan
    // triangle is `bottom_centre`); tagging it as `KIND_PLATE_BOTTOM`
    // is enough to route the whole fan through the translucent
    // fragment-shader branch under flat interpolation.
    let bottom_centre = side_bot_off + outline_len as u32;
    vertices.push(PlateVertex {
        position: [0.0, 0.0, z_bot],
        kind: KIND_PLATE_BOTTOM,
    });

    let cage_off = bottom_centre + 1;
    for &z in &[lo.z, hi.z] {
        for [x, y] in [[lo.x, lo.y], [hi.x, lo.y], [hi.x, hi.y], [lo.x, hi.y]] {
            vertices.push(PlateVertex {
                position: [x, y, z],
                kind: KIND_CAGE,
            });
        }
    }

    // 5 envelope-wall quads (4 sides + Z = hi.z top), 4 vertices each.
    // Their fragments are normally discarded; the bit corresponding
    // to `kind - KIND_ENVELOPE_WALL_X_NEG` in `face_breach_mask` gates
    // visibility. The walls share the cage's plane: depth-test Less
    // against the cage edges (which have written depth first) means
    // the cage edges stay visible on top of the wall stripes.
    let wall_off = cage_off + 8;
    let wall_corners: [(u32, [[f32; 3]; 4]); 5] = [
        (
            KIND_ENVELOPE_WALL_X_NEG,
            [
                [lo.x, lo.y, lo.z],
                [lo.x, hi.y, lo.z],
                [lo.x, hi.y, hi.z],
                [lo.x, lo.y, hi.z],
            ],
        ),
        (
            KIND_ENVELOPE_WALL_X_POS,
            [
                [hi.x, lo.y, lo.z],
                [hi.x, hi.y, lo.z],
                [hi.x, hi.y, hi.z],
                [hi.x, lo.y, hi.z],
            ],
        ),
        (
            KIND_ENVELOPE_WALL_Y_NEG,
            [
                [lo.x, lo.y, lo.z],
                [hi.x, lo.y, lo.z],
                [hi.x, lo.y, hi.z],
                [lo.x, lo.y, hi.z],
            ],
        ),
        (
            KIND_ENVELOPE_WALL_Y_POS,
            [
                [lo.x, hi.y, lo.z],
                [hi.x, hi.y, lo.z],
                [hi.x, hi.y, hi.z],
                [lo.x, hi.y, hi.z],
            ],
        ),
        (
            KIND_ENVELOPE_WALL_Z_POS,
            [
                [lo.x, lo.y, hi.z],
                [hi.x, lo.y, hi.z],
                [hi.x, hi.y, hi.z],
                [lo.x, hi.y, hi.z],
            ],
        ),
    ];
    for (kind, quad) in &wall_corners {
        for position in quad {
            vertices.push(PlateVertex {
                position: *position,
                kind: *kind,
            });
        }
    }

    let outline_len_u32 = outline_len as u32;
    let mut top_indices: Vec<u32> = Vec::with_capacity(6 + 6 * outline_len);

    // Inner rectangle: 2 triangles, CCW from +Z.
    top_indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);

    // Outer top border ring: connect each inner corner to the
    // outline arc points ringing it, plus quad strips along each
    // straight edge between adjacent arcs. The outline points of
    // arc `c` are at indices `[c * arc_pts .. (c+1) * arc_pts)` in
    // the outline, in CCW order. Each arc rounds a single inner
    // corner; index the inner corners (via their border-kind
    // duplicates) in matching CCW order.
    //
    // CCW order from +Z, starting at +X+Y: I0 = (hi.x, hi.y),
    // I1 = (lo.x, hi.y), I2 = (lo.x, lo.y), I3 = (hi.x, lo.y).
    for c in 0..4u32 {
        let inner = inner_as_border_off + c;
        let next_inner = inner_as_border_off + (c + 1) % 4;
        // Arc fan around inner corner `c`.
        for s in 0..segments {
            let arc_idx = c * arc_pts + s;
            let next_arc_idx = c * arc_pts + s + 1;
            top_indices.push(inner);
            top_indices.push(outer_top_off + arc_idx);
            top_indices.push(outer_top_off + next_arc_idx);
        }
        // Straight edge between this arc's last point and the next
        // arc's first point: one quad (inner -> next_inner ->
        // next_arc_first -> arc_last) split into two triangles.
        let arc_last = c * arc_pts + segments;
        let next_arc_first = ((c + 1) % 4) * arc_pts;
        top_indices.push(inner);
        top_indices.push(next_inner);
        top_indices.push(outer_top_off + next_arc_first);
        top_indices.push(inner);
        top_indices.push(outer_top_off + next_arc_first);
        top_indices.push(outer_top_off + arc_last);
    }

    // Solid: side panels only.
    let mut solid_indices: Vec<u32> = Vec::with_capacity(6 * outline_len);

    // Side panels: one quad per outline segment (i, i+1), with the
    // top edge from the side-top vertex range and the bottom edge
    // from the side-bottom vertex range. Wound CCW from outside the
    // plate so the front face is the visible one.
    for i in 0..outline_len_u32 {
        let next = (i + 1) % outline_len_u32;
        let t_i = side_top_off + i;
        let t_n = side_top_off + next;
        let b_i = side_bot_off + i;
        let b_n = side_bot_off + next;
        solid_indices.push(t_i);
        solid_indices.push(b_i);
        solid_indices.push(b_n);
        solid_indices.push(t_i);
        solid_indices.push(b_n);
        solid_indices.push(t_n);
    }

    // Bottom face: fan from `bottom_centre` outward. CCW from -Z
    // (the visible side when the camera orbits below the plate),
    // which is CW from +Z and gets back-face-culled by
    // `bottom_pipeline` when the camera is above. The fan provoking
    // vertex `bottom_centre` carries `KIND_PLATE_BOTTOM`, which
    // routes the fan through the translucent fragment-shader branch.
    let mut bottom_indices: Vec<u32> = Vec::with_capacity(3 * outline_len);
    for i in 0..outline_len_u32 {
        let next = (i + 1) % outline_len_u32;
        bottom_indices.push(bottom_centre);
        bottom_indices.push(side_bot_off + next);
        bottom_indices.push(side_bot_off + i);
    }

    // Cage: 4 bottom edges, 4 top edges, 4 verticals.
    let cage_bottom = cage_off;
    let cage_top = cage_off + 4;
    let mut cage_indices: Vec<u32> = Vec::with_capacity(24);
    for i in 0..4u32 {
        let next = (i + 1) % 4;
        cage_indices.push(cage_bottom + i);
        cage_indices.push(cage_bottom + next);
        cage_indices.push(cage_top + i);
        cage_indices.push(cage_top + next);
        cage_indices.push(cage_bottom + i);
        cage_indices.push(cage_top + i);
    }

    // Envelope-wall quads: 2 triangles each, 5 quads. Winding doesn't
    // affect visibility (the wall pipeline uses `cull_mode: None`),
    // so a single ordering works for all five faces.
    let mut wall_indices: Vec<u32> = Vec::with_capacity(30);
    for face in 0..5u32 {
        let base = wall_off + 4 * face;
        wall_indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    PlateGeometry {
        vertices,
        top_indices,
        solid_indices,
        bottom_indices,
        cage_indices,
        wall_indices,
    }
}

#[cfg(test)]
mod plate_geometry_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn vol() -> BuildVolume {
        BuildVolume::new(100.0, 60.0, 80.0)
    }

    fn outline_len() -> usize {
        4 * (PLATE_CORNER_SEGMENTS + 1) as usize
    }

    fn cage_off() -> u32 {
        // 4 inner-floor + 4 inner-as-border + 3 outline copies + 1
        // bottom-fan centre.
        8 + 3 * outline_len() as u32 + 1
    }

    fn wall_off() -> u32 {
        cage_off() + 8
    }

    #[test]
    fn vertex_buffer_has_inner_quad_border_dups_three_outlines_a_centre_eight_cage_and_five_walls()
    {
        let g = plate_geometry(BuildVolume::new(192.0, 120.0, 200.0));
        let n = outline_len();
        // 5 envelope walls add 20 vertices (4 per quad).
        assert_eq!(g.vertices.len(), 8 + 3 * n + 1 + 8 + 20);

        for v in &g.vertices[..4] {
            assert_eq!(v.kind, KIND_FLOOR);
        }
        for v in &g.vertices[4..8] {
            assert_eq!(v.kind, KIND_PLATE_TOP_BORDER);
        }
        for v in &g.vertices[8..8 + n] {
            assert_eq!(v.kind, KIND_PLATE_TOP_BORDER);
        }
        // Side panel outlines (top edge then bottom edge) are
        // KIND_PLATE_SIDE; the bottom-fan centre that follows is
        // KIND_PLATE_BOTTOM (the provoking vertex of every bottom
        // fan triangle).
        for v in &g.vertices[8 + n..8 + 3 * n] {
            assert_eq!(v.kind, KIND_PLATE_SIDE);
        }
        assert_eq!(g.vertices[8 + 3 * n].kind, KIND_PLATE_BOTTOM);
        let cage_end = wall_off() as usize;
        for v in &g.vertices[8 + 3 * n + 1..cage_end] {
            assert_eq!(v.kind, KIND_CAGE);
        }
        // Wall quads come last, in face order: -X, +X, -Y, +Y, +Z.
        let expected_wall_kinds = [
            KIND_ENVELOPE_WALL_X_NEG,
            KIND_ENVELOPE_WALL_X_POS,
            KIND_ENVELOPE_WALL_Y_NEG,
            KIND_ENVELOPE_WALL_Y_POS,
            KIND_ENVELOPE_WALL_Z_POS,
        ];
        for (face, expected) in expected_wall_kinds.iter().enumerate() {
            let base = cage_end + 4 * face;
            for v in &g.vertices[base..base + 4] {
                assert_eq!(v.kind, *expected);
            }
        }
    }

    #[test]
    fn inner_top_corners_are_envelope_corners_at_z_zero() {
        let g = plate_geometry(vol());
        for v in &g.vertices[..4] {
            assert_eq!(v.position[2], 0.0);
            assert_eq!(v.position[0].abs(), 50.0);
            assert_eq!(v.position[1].abs(), 30.0);
        }
        // The inner-as-border duplicates share the same XY.
        for (a, b) in g.vertices[..4].iter().zip(&g.vertices[4..8]) {
            assert_eq!(a.position, b.position);
        }
    }

    #[test]
    fn outline_extends_overhang_beyond_envelope() {
        // The outline's bounding box should be the build envelope
        // expanded by `PLATE_OVERHANG_MM` on each axis.
        let g = plate_geometry(vol());
        let n = outline_len();
        let outline = &g.vertices[8..8 + n];
        let max_x = outline
            .iter()
            .map(|v| v.position[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let max_y = outline
            .iter()
            .map(|v| v.position[1])
            .fold(f32::NEG_INFINITY, f32::max);
        assert_approx_eq!(f32, max_x, 50.0 + PLATE_OVERHANG_MM, epsilon = 1e-4);
        assert_approx_eq!(f32, max_y, 30.0 + PLATE_OVERHANG_MM, epsilon = 1e-4);
    }

    #[test]
    fn outline_has_thickness_between_top_and_bottom_copies() {
        let g = plate_geometry(vol());
        let n = outline_len();
        let side_top_off = 8 + n;
        let side_bot_off = side_top_off + n;
        for i in 0..n {
            let top = &g.vertices[side_top_off + i];
            let bot = &g.vertices[side_bot_off + i];
            assert_eq!(top.position[0], bot.position[0]);
            assert_eq!(top.position[1], bot.position[1]);
            assert_approx_eq!(f32, top.position[2], 0.0);
            assert_approx_eq!(f32, bot.position[2], -PLATE_THICKNESS_MM);
        }
    }

    #[test]
    fn top_border_ring_triangles_have_uniform_border_kind() {
        // A border-ring triangle must have all three vertices at
        // KIND_PLATE_TOP_BORDER. If any inner-corner FLOOR vertex
        // sneaks in, flat interpolation would bleed the grid into
        // the rounded ring.
        let g = plate_geometry(vol());
        // Skip the inner quad's first 6 indices (2 triangles).
        let ring_indices = &g.top_indices[6..];
        for tri in ring_indices.chunks(3) {
            for &idx in tri {
                let kind = g.vertices[idx as usize].kind;
                assert_eq!(
                    kind, KIND_PLATE_TOP_BORDER,
                    "border ring index {idx} has kind {kind}, expected PLATE_TOP_BORDER"
                );
            }
        }
    }

    #[test]
    fn cage_uses_low_and_high_z_layers() {
        let g = plate_geometry(vol());
        let off = cage_off() as usize;
        for v in &g.vertices[off..off + 4] {
            assert_eq!(v.position[2], 0.0);
        }
        for v in &g.vertices[off + 4..off + 8] {
            assert_eq!(v.position[2], 80.0);
        }
    }

    #[test]
    fn cage_index_count_is_twelve_edges() {
        let g = plate_geometry(vol());
        // Two indices per edge, 12 edges = 24 indices.
        assert_eq!(g.cage_indices.len(), 24);
    }

    #[test]
    fn cage_indices_reference_only_cage_vertices() {
        let g = plate_geometry(vol());
        let off = cage_off();
        for idx in g.cage_indices {
            assert!(
                (off..off + 8).contains(&idx),
                "cage index {idx} out of cage range starting at {off}"
            );
        }
    }

    #[test]
    fn top_indices_cover_inner_quad_plus_border_ring() {
        // Inner quad: 2 triangles = 6 indices. Outer border ring:
        // per inner corner, `segments` arc triangles + 2 strip
        // triangles = 3 * (segments + 2) indices, times 4 corners.
        let g = plate_geometry(vol());
        let segments = PLATE_CORNER_SEGMENTS;
        let expected = 6 + 4 * 3 * (segments + 2) as usize;
        assert_eq!(g.top_indices.len(), expected);
    }

    #[test]
    fn solid_indices_cover_side_panels_only() {
        // Sides: outline_len quads = 6 * n indices.
        let g = plate_geometry(vol());
        let n = outline_len();
        assert_eq!(g.solid_indices.len(), 6 * n);
    }

    #[test]
    fn bottom_indices_cover_the_fan_only() {
        // Bottom fan: outline_len triangles = 3 * n indices. Each
        // fan triangle starts with the bottom-fan centre, the
        // provoking vertex with KIND_PLATE_BOTTOM. Any other
        // ordering would route the fan through the wrong fragment-
        // shader branch (loses translucency).
        let g = plate_geometry(vol());
        let n = outline_len();
        assert_eq!(g.bottom_indices.len(), 3 * n);
        let bottom_centre = 8 + 3 * outline_len() as u32;
        for tri in g.bottom_indices.chunks(3) {
            assert_eq!(tri[0], bottom_centre);
        }
    }

    #[test]
    fn wall_indices_cover_five_quads() {
        // 5 quads, 2 triangles per quad, 3 indices per triangle.
        let g = plate_geometry(vol());
        assert_eq!(g.wall_indices.len(), 30);
    }

    #[test]
    fn wall_indices_reference_only_wall_vertices() {
        let g = plate_geometry(vol());
        let off = wall_off();
        for idx in &g.wall_indices {
            assert!(
                (off..off + 20).contains(idx),
                "wall index {idx} out of wall range starting at {off}"
            );
        }
    }

    #[test]
    fn wall_vertices_sit_on_their_envelope_face() {
        let g = plate_geometry(vol());
        let lo = vol().envelope_min_mm();
        let hi = vol().envelope_max_mm();
        let off = wall_off() as usize;
        // -X quad: x = lo.x.
        for v in &g.vertices[off..off + 4] {
            assert_approx_eq!(f32, v.position[0], lo.x);
        }
        // +X quad: x = hi.x.
        for v in &g.vertices[off + 4..off + 8] {
            assert_approx_eq!(f32, v.position[0], hi.x);
        }
        // -Y quad: y = lo.y.
        for v in &g.vertices[off + 8..off + 12] {
            assert_approx_eq!(f32, v.position[1], lo.y);
        }
        // +Y quad: y = hi.y.
        for v in &g.vertices[off + 12..off + 16] {
            assert_approx_eq!(f32, v.position[1], hi.y);
        }
        // +Z top quad: z = hi.z.
        for v in &g.vertices[off + 16..off + 20] {
            assert_approx_eq!(f32, v.position[2], hi.z);
        }
    }
}
