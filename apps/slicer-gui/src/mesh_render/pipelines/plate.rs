/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Build-plate render pipeline. Three pipelines share a vertex
//! layout and bind-group layout, differing only in primitive topology
//! and depth-write policy:
//!
//! - `top_pipeline`: fills the plate top face (inner grid + outer
//!   rounded border) at `Z = 0`. Depth-write off so models resting
//!   at `Z = 0` always win z-fighting against the plate.
//! - `solid_pipeline`: fills the plate sides and bottom (the slab
//!   below `Z = 0`). Depth-write on so the sides occlude content
//!   behind them when the camera orbits below the plate.
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
    /// Plate sides + bottom face. Depth-write on so the slab
    /// occludes content behind it when the camera orbits low.
    pub(in crate::mesh_render) solid_pipeline: wgpu::RenderPipeline,
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
            line_pipeline,
            uniforms_bind_group,
        }
    }
}

/// GPU resources for one specific [`BuildVolume`]: one vertex buffer
/// holding all plate + cage geometry, plus three index buffers (top
/// face, plate sides + bottom, cage edges). The `build_volume` field
/// is the cache key: when the renderer's per-frame `BuildVolume`
/// differs, the buffers rebuild.
pub(in crate::mesh_render) struct PlateBuffers {
    pub(in crate::mesh_render) build_volume: BuildVolume,
    pub(in crate::mesh_render) vertex_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) top_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) solid_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) cage_index_buffer: wgpu::Buffer,
    pub(in crate::mesh_render) top_index_count: u32,
    pub(in crate::mesh_render) solid_index_count: u32,
    pub(in crate::mesh_render) cage_index_count: u32,
}

impl PlateBuffers {
    pub(in crate::mesh_render) fn create(device: &wgpu::Device, build_volume: BuildVolume) -> Self {
        let geom = plate_geometry(build_volume);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer plate vbo"),
            contents: bytemuck::cast_slice(&geom.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let top_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer plate top ibo"),
            contents: bytemuck::cast_slice(&geom.top_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let solid_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer plate solid ibo"),
            contents: bytemuck::cast_slice(&geom.solid_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let cage_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer plate cage ibo"),
            contents: bytemuck::cast_slice(&geom.cage_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            build_volume,
            vertex_buffer,
            top_index_buffer,
            solid_index_buffer,
            cage_index_buffer,
            top_index_count: geom.top_indices.len() as u32,
            solid_index_count: geom.solid_indices.len() as u32,
            cage_index_count: geom.cage_indices.len() as u32,
        }
    }
}

/// Bundle returned by [`plate_geometry`]: one vertex buffer plus
/// three index streams (top face, plate sides + bottom, cage edges).
pub(in crate::mesh_render) struct PlateGeometry {
    pub(in crate::mesh_render) vertices: Vec<PlateVertex>,
    pub(in crate::mesh_render) top_indices: Vec<u32>,
    pub(in crate::mesh_render) solid_indices: Vec<u32>,
    pub(in crate::mesh_render) cage_indices: Vec<u32>,
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
///   `Z = -PLATE_THICKNESS_MM`, kind = side.
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

    let bottom_centre = side_bot_off + outline_len as u32;
    vertices.push(PlateVertex {
        position: [0.0, 0.0, z_bot],
        kind: KIND_PLATE_SIDE,
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

    // Solid: side panels + bottom face.
    let mut solid_indices: Vec<u32> = Vec::with_capacity(6 * outline_len + 3 * outline_len);

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
    // which is CW from +Z, i.e. the reverse of the top-face order.
    for i in 0..outline_len_u32 {
        let next = (i + 1) % outline_len_u32;
        solid_indices.push(bottom_centre);
        solid_indices.push(side_bot_off + next);
        solid_indices.push(side_bot_off + i);
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

    PlateGeometry {
        vertices,
        top_indices,
        solid_indices,
        cage_indices,
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

    #[test]
    fn vertex_buffer_has_inner_quad_border_dups_three_outlines_a_centre_and_eight_cage() {
        let g = plate_geometry(BuildVolume::new(192.0, 120.0, 200.0));
        let n = outline_len();
        assert_eq!(g.vertices.len(), 8 + 3 * n + 1 + 8);

        for v in &g.vertices[..4] {
            assert_eq!(v.kind, KIND_FLOOR);
        }
        for v in &g.vertices[4..8] {
            assert_eq!(v.kind, KIND_PLATE_TOP_BORDER);
        }
        for v in &g.vertices[8..8 + n] {
            assert_eq!(v.kind, KIND_PLATE_TOP_BORDER);
        }
        for v in &g.vertices[8 + n..8 + 3 * n + 1] {
            assert_eq!(v.kind, KIND_PLATE_SIDE);
        }
        for v in &g.vertices[8 + 3 * n + 1..] {
            assert_eq!(v.kind, KIND_CAGE);
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
    fn solid_indices_cover_side_panels_and_bottom_fan() {
        // Sides: outline_len quads = 6 * n indices. Bottom fan:
        // outline_len triangles = 3 * n indices.
        let g = plate_geometry(vol());
        let n = outline_len();
        assert_eq!(g.solid_indices.len(), 6 * n + 3 * n);
    }
}
