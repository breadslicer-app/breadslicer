/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::borrow::Cow;
use std::num::NonZeroU64;

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use sc_mesh_core::scene::Scene;
use slint::wgpu_28::wgpu;
use wgpu::util::DeviceExt;

use crate::camera::{DEFAULT_DISTANCE_FACTOR, OrbitCamera};
use crate::gizmo_geometry::{self, GizmoVertex};

mod gizmo_entries;
mod scene_upload;

use gizmo_entries::{
    GIZMO_CENTRE_SLOT, GIZMO_DRAW_ENTRY_SIZE, GIZMO_ENTRY_SLOTS, GIZMO_ROTATE_SLOT_BASE,
    GIZMO_SCALE_AXIS_SLOT_BASE, GIZMO_TRANSLATE_SLOT_BASE, GizmoDrawEntry, GizmoUniformsGpu,
    build_rotate_entries, build_scale_axis_entries, build_scale_centre_entry,
    build_translate_entries,
};
pub use gizmo_entries::{GizmoFrame, GizmoVisibility};
pub(crate) use scene_upload::GpuScene;
use scene_upload::{GpuVertex, MODEL_ENTRY_SIZE, align_up, build_model_entries};

const MESH_SHADER_SOURCE: &str = include_str!("shaders/mesh_render.wgsl");
const OUTLINE_SHADER_SOURCE: &str = include_str!("shaders/outline.wgsl");
const GIZMO_SHADER_SOURCE: &str = include_str!("shaders/gizmo.wgsl");

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Format for the gizmo pass's private depth attachment. Stencil is
/// not used by the gizmo pipeline, so a plain `Depth32Float` (default-
/// limits supported, single-aspect) suffices.
const GIZMO_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
// Depth24PlusStencil8 carries an 8-bit stencil alongside depth in a
// single attachment. The selection-outline post-pass writes `1` to
// stencil for the selected object via per-draw stencil reference, then
// reads that stencil aspect in a fullscreen edge-detect. Default-limits
// supported on every wgpu backend Slint may pick.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
const SAMPLE_COUNT: u32 = 4;
const FIT_FRACTION: f32 = 0.75;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
}

struct MsaaTargets {
    width: u32,
    height: u32,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    /// Stencil-aspect view of the depth-stencil texture, bound by the
    /// outline post-pass as a multisampled u32 texture. Recreated
    /// alongside `depth_view` since they share the underlying texture.
    outline_bind_group: wgpu::BindGroup,
    /// Private depth attachment for the gizmo pass. The gizmo
    /// renders to the resolved (non-MSAA) target with depth cleared
    /// at pass start so it sits on top of the mesh, but the gizmo
    /// pipeline still runs depth-test-on internally so the three
    /// axes order correctly where they overlap near the pivot. We
    /// can't reuse the main MSAA depth — its sample count (4)
    /// doesn't match the gizmo pipeline's count (1).
    gizmo_depth_view: wgpu::TextureView,
}

// Double-buffered ring of destination color textures. `render` writes to one
// texture while the `slint::Image` returned last frame (which wraps the
// *other* texture) is still being sampled by Slint. Retaining both across
// frames avoids the per-frame GPU-allocator churn of freshly creating a
// RENDER_ATTACHMENT | TEXTURE_BINDING texture on every orbit tick.
struct TargetPool {
    width: u32,
    height: u32,
    textures: [wgpu::Texture; 2],
    next: usize,
}

/// A `GpuScene` paired with its models bind group. The bind group
/// references the scene's storage buffer by handle, so it must be
/// rebuilt every time the storage buffer is recreated (i.e. on every
/// scene install or upload).
struct InstalledScene {
    gpu: GpuScene,
    models_bind_group: wgpu::BindGroup,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    outline_bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    uniforms_bind_group: wgpu::BindGroup,
    models_bind_group_layout: wgpu::BindGroupLayout,
    gizmo_pipeline: wgpu::RenderPipeline,
    /// Axis geometry: shaft prism + cone tip + per-axis midpoint
    /// cube, all in handle-local space (axis = +X). The renderer
    /// draws the shaft+cone subrange as the translate handle and
    /// the midpoint-cube subrange as the scale-axis handle.
    gizmo_axis_vertex_buffer: wgpu::Buffer,
    gizmo_axis_index_buffer: wgpu::Buffer,
    /// Number of indices in the translate (shaft + cone) subrange,
    /// from `[0, gizmo_axis_shaft_tip_index_count)`. The remaining
    /// indices `[gizmo_axis_shaft_tip_index_count, end)` belong to
    /// the per-axis midpoint cube.
    gizmo_axis_shaft_tip_index_count: u32,
    gizmo_axis_total_index_count: u32,
    /// Rotate-arc geometry: a 90° torus segment around +X in
    /// handle-local space, centred on +Y. The per-frame X-roll
    /// (baked into `axis_rotation`) swings the arc to face the
    /// camera before the world-axis rotation maps it onto X/Y/Z.
    /// Decorative — the click target is the cone spindle below.
    gizmo_arc_vertex_buffer: wgpu::Buffer,
    gizmo_arc_index_buffer: wgpu::Buffer,
    gizmo_arc_index_count: u32,
    /// Rotate-handle two-cone spindle: bases touching at the arc's
    /// camera-facing midpoint, tips along ±axis. The active rotate
    /// handle (mirrors `gizmo::ray_hits_rotate_cones` on the picker
    /// side).
    gizmo_rotate_cones_vertex_buffer: wgpu::Buffer,
    gizmo_rotate_cones_index_buffer: wgpu::Buffer,
    gizmo_rotate_cones_index_count: u32,
    /// Centre cube geometry for the uniform-scale handle.
    gizmo_centre_vertex_buffer: wgpu::Buffer,
    gizmo_centre_index_buffer: wgpu::Buffer,
    gizmo_centre_index_count: u32,
    gizmo_uniforms_buffer: wgpu::Buffer,
    /// Per-frame draw-entry buffer with `GIZMO_ENTRY_SLOTS` slots.
    /// Each pass binds a window of one entry via a dynamic offset:
    /// translate occupies `[0, 3)`, rotate `[3, 6)`, scale-axis
    /// `[6, 9)`, scale-uniform / centre `[9, 10)`.
    gizmo_entries_buffer: wgpu::Buffer,
    gizmo_entry_stride: u64,
    gizmo_uniforms_bind_group: wgpu::BindGroup,
    gizmo_entries_bind_group: wgpu::BindGroup,
    scene: Option<InstalledScene>,
    msaa: Option<MsaaTargets>,
    targets: Option<TargetPool>,
}

/// Vertical field of view in radians.
///
/// Chosen so that a sphere of the scene's bounding radius fills
/// `FIT_FRACTION` of the viewport height when the camera sits at its initial
/// framing distance (`DEFAULT_DISTANCE_FACTOR * radius`). Being a pure
/// function of those constants, the FOV is viewport-independent — resizing
/// the window rescales the image but never distorts perspective.
pub fn default_fov_y() -> f32 {
    2.0 * (1.0 / (FIT_FRACTION * DEFAULT_DISTANCE_FACTOR)).atan()
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(MESH_SHADER_SOURCE)),
        });
        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer outline shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(OUTLINE_SHADER_SOURCE)),
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
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer pll"),
            bind_group_layouts: &[&uniforms_bind_group_layout, &models_bind_group_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
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
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                // Every fragment that passes depth replaces the stencil
                // with the per-draw `stencil_reference` value (set by
                // the render loop to 1 for the selected object and 0
                // otherwise). This means after the main pass the
                // stencil holds 1 only over the selected object's
                // visible silhouette — exactly what the outline
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

        let outline_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

        let outline_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mesh-viewer outline pll"),
                bind_group_layouts: &[&outline_bind_group_layout],
                immediate_size: 0,
            });

        let outline_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer outline pipeline"),
            layout: Some(&outline_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &outline_shader,
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
            // and samples the still-MSAA stencil as input — colour and
            // attachment sample counts agree at 1, while the texture
            // binding is multisampled.
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
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

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
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

        // ---- gizmo pipeline + static buffers --------------------------------
        let gizmo_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer gizmo shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(GIZMO_SHADER_SOURCE)),
        });

        let (axis_vertices, axis_indices, axis_shaft_tip_index_count) =
            gizmo_geometry::build_axis_geometry();
        let gizmo_axis_total_index_count = axis_indices.len() as u32;
        let gizmo_axis_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo axis vbo"),
                contents: bytemuck::cast_slice(&axis_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let gizmo_axis_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo axis ibo"),
                contents: bytemuck::cast_slice(&axis_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        let (arc_vertices, arc_indices) = gizmo_geometry::build_rotate_arc_geometry();
        let gizmo_arc_index_count = arc_indices.len() as u32;
        let gizmo_arc_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo arc vbo"),
                contents: bytemuck::cast_slice(&arc_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let gizmo_arc_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo arc ibo"),
            contents: bytemuck::cast_slice(&arc_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (rotate_cones_vertices, rotate_cones_indices) =
            gizmo_geometry::build_rotate_cones_geometry();
        let gizmo_rotate_cones_index_count = rotate_cones_indices.len() as u32;
        let gizmo_rotate_cones_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo rotate cones vbo"),
                contents: bytemuck::cast_slice(&rotate_cones_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let gizmo_rotate_cones_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo rotate cones ibo"),
                contents: bytemuck::cast_slice(&rotate_cones_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        let (centre_vertices, centre_indices) = gizmo_geometry::build_centre_cube_geometry();
        let gizmo_centre_index_count = centre_indices.len() as u32;
        let gizmo_centre_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo centre vbo"),
                contents: bytemuck::cast_slice(&centre_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let gizmo_centre_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo centre ibo"),
                contents: bytemuck::cast_slice(&centre_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        let gizmo_uniforms_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mesh-viewer gizmo uniforms bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(
                            std::mem::size_of::<GizmoUniformsGpu>() as u64
                        ),
                    },
                    count: None,
                }],
            });

        let gizmo_entries_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

        let gizmo_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mesh-viewer gizmo pll"),
                bind_group_layouts: &[&gizmo_uniforms_bgl, &gizmo_entries_bgl],
                immediate_size: 0,
            });

        let gizmo_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer gizmo pipeline"),
            layout: Some(&gizmo_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &gizmo_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GizmoVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
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
                format: GIZMO_DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            // Resolved (non-MSAA) target → 1-sample pipeline.
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &gizmo_shader,
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

        let gizmo_uniforms_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer gizmo uniforms"),
            size: std::mem::size_of::<GizmoUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let gizmo_entry_stride = align_up(GIZMO_DRAW_ENTRY_SIZE, alignment);
        let gizmo_entries_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer gizmo entries"),
            size: gizmo_entry_stride * GIZMO_ENTRY_SLOTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let gizmo_uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer gizmo uniforms bg"),
            layout: &gizmo_uniforms_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: gizmo_uniforms_buffer.as_entire_binding(),
            }],
        });
        let gizmo_entries_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer gizmo entries bg"),
            layout: &gizmo_entries_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &gizmo_entries_buffer,
                    offset: 0,
                    size: NonZeroU64::new(GIZMO_DRAW_ENTRY_SIZE),
                }),
            }],
        });

        Self {
            device,
            queue,
            pipeline,
            outline_pipeline,
            outline_bind_group_layout,
            uniform_buffer,
            uniforms_bind_group,
            models_bind_group_layout,
            gizmo_pipeline,
            gizmo_axis_vertex_buffer,
            gizmo_axis_index_buffer,
            gizmo_axis_shaft_tip_index_count: axis_shaft_tip_index_count,
            gizmo_axis_total_index_count,
            gizmo_arc_vertex_buffer,
            gizmo_arc_index_buffer,
            gizmo_arc_index_count,
            gizmo_rotate_cones_vertex_buffer,
            gizmo_rotate_cones_index_buffer,
            gizmo_rotate_cones_index_count,
            gizmo_centre_vertex_buffer,
            gizmo_centre_index_buffer,
            gizmo_centre_index_count,
            gizmo_uniforms_buffer,
            gizmo_entries_buffer,
            gizmo_entry_stride,
            gizmo_uniforms_bind_group,
            gizmo_entries_bind_group,
            scene: None,
            msaa: None,
            targets: None,
        }
    }

    pub fn clear_scene(&mut self) {
        self.scene = None;
        self.msaa = None;
        self.targets = None;
    }

    /// Drops the viewport-sized allocations (MSAA + target pool) so the
    /// next `render` reallocates them at the current `(width, height)`.
    /// Used by `ViewportController::on_resize_settled` to snap back from
    /// bucketed drag-resize sizing to exact pixel sizing once the user
    /// stops resizing.
    pub fn invalidate_sizing(&mut self) {
        self.msaa = None;
        self.targets = None;
    }

    /// World-space AABB of the currently uploaded scene, captured at
    /// install time via `Scene::aabb`. `None` if nothing has been
    /// uploaded yet.
    pub fn scene_bounds(&self) -> Option<(Vec3, Vec3)> {
        self.scene.as_ref().map(|s| s.gpu.aabb())
    }

    /// UI-thread upload path. Used only on `RenderingTeardown` →
    /// `RenderingSetup` recovery, where the new device is local to this
    /// `Renderer` and a worker-built `GpuScene` from the previous device
    /// would be unusable. New scene loads go through the worker via
    /// [`GpuScene::build`] + [`Renderer::install_scene`] instead.
    pub fn upload_scene(&mut self, scene: &Scene) -> Result<()> {
        let gpu = GpuScene::build(&self.device, scene)?;
        self.attach_scene(gpu);
        Ok(())
    }

    /// Installs a `GpuScene` built on a worker thread, but only if it
    /// was built against this renderer's device. Returns `false` if the
    /// device has rotated since the worker started (mid-load device
    /// loss); the caller falls back to the recovery upload path.
    pub fn install_scene(&mut self, gpu: GpuScene, built_with: &wgpu::Device) -> bool {
        if &self.device != built_with {
            return false;
        }
        self.attach_scene(gpu);
        true
    }

    fn attach_scene(&mut self, gpu: GpuScene) {
        // The bind group references a single ModelEntry-sized window into
        // the per-scene uniform buffer; the dynamic offset shifts that
        // window to the right slot per draw call. The window size must
        // match `min_binding_size` declared in the layout.
        let models_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer models bg"),
            layout: &self.models_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: gpu.model_buffer(),
                    offset: 0,
                    size: NonZeroU64::new(MODEL_ENTRY_SIZE),
                }),
            }],
        });
        self.scene = Some(InstalledScene {
            gpu,
            models_bind_group,
        });
    }

    fn ensure_msaa(&mut self, width: u32, height: u32) {
        let same = self
            .msaa
            .as_ref()
            .is_some_and(|m| m.width == width && m.height == height);
        if same {
            return;
        }

        let color_texture = self.device.create_texture(&wgpu::TextureDescriptor {
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

        let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
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

        let outline_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer outline bg"),
            layout: &self.outline_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&stencil_view),
            }],
        });

        let gizmo_depth = self.device.create_texture(&wgpu::TextureDescriptor {
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

        self.msaa = Some(MsaaTargets {
            width,
            height,
            color_view,
            depth_view,
            outline_bind_group,
            gizmo_depth_view,
        });
    }

    fn ensure_targets(&mut self, width: u32, height: u32) {
        let same = self
            .targets
            .as_ref()
            .is_some_and(|t| t.width == width && t.height == height);
        if same {
            return;
        }

        let a = create_color_target(&self.device, width, height);
        let b = create_color_target(&self.device, width, height);
        self.targets = Some(TargetPool {
            width,
            height,
            textures: [a, b],
            next: 0,
        });
    }

    fn write_gizmo_entries(&self, slot_base: u64, entries: &[GizmoDrawEntry]) {
        for (i, entry) in entries.iter().enumerate() {
            let offset = (slot_base + i as u64) * self.gizmo_entry_stride;
            self.queue.write_buffer(
                &self.gizmo_entries_buffer,
                offset,
                bytemuck::bytes_of(entry),
            );
        }
    }

    pub fn render(
        &mut self,
        scene: &Scene,
        camera: &OrbitCamera,
        selection: Option<usize>,
        gizmo: Option<GizmoFrame>,
        width: u32,
        height: u32,
    ) -> Option<wgpu::Texture> {
        if width == 0 || height == 0 || self.scene.is_none() {
            return None;
        }

        self.ensure_msaa(width, height);
        self.ensure_targets(width, height);
        let installed = self.scene.as_ref().unwrap();
        let msaa = self.msaa.as_ref().unwrap();

        // Per-frame upload of model matrices. The CPU-side `Scene` is the
        // source of truth; the GPU uniform buffer is a mirror that lives
        // long enough to be sampled across the next draw. The renderer's
        // GpuScene must match the Scene 1:1 — caller-side invariant
        // checked by debug_assert so test failures surface early.
        debug_assert_eq!(
            installed.gpu.object_count(),
            scene.objects.len(),
            "GpuScene and Scene have diverged in object count"
        );
        let entries = build_model_entries(scene, selection);
        if entries.is_empty() {
            // Defensive: nothing to draw.
            return None;
        }
        // Each entry lives at `i * model_stride`. The stride is at least
        // `MODEL_ENTRY_SIZE` and a multiple of the device's uniform
        // offset alignment (typically 256 bytes), so consecutive entries
        // are not contiguous and must be written individually.
        for (i, entry) in entries.iter().enumerate() {
            let offset = i as u64 * installed.gpu.model_stride();
            self.queue.write_buffer(
                installed.gpu.model_buffer(),
                offset,
                bytemuck::bytes_of(entry),
            );
        }

        let (aabb_min, aabb_max) = installed.gpu.aabb();
        let diagonal = (aabb_max - aabb_min).length().max(1e-3);

        let aspect = width as f32 / height as f32;
        let fov_y = default_fov_y();
        let view_mat = camera.view_matrix();
        let proj = Mat4::perspective_rh(fov_y, aspect, 0.01 * diagonal, 100.0 * diagonal);
        let view_proj = proj * view_mat;

        let uniforms = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: [0.5, 0.7, 1.0, 0.0],
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let target = {
            let pool = self
                .targets
                .as_mut()
                .expect("ensure_targets populated the pool");
            let idx = pool.next;
            pool.next = (idx + 1) % pool.textures.len();
            pool.textures[idx].clone()
        };
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mesh-viewer encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh-viewer pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa.color_view,
                    resolve_target: Some(&target_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.12,
                            g: 0.12,
                            b: 0.13,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &msaa.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0),
                        store: wgpu::StoreOp::Store,
                    }),
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniforms_bind_group, &[]);
            // One draw call per object. The models bind group is rebound
            // at each step with the dynamic offset of the matching
            // ModelEntry slot, so the vertex shader's `model_entry`
            // uniform reads that object's matrix. The stencil
            // reference is `1` only for the selected object, so after
            // this pass the stencil holds 1 over its visible silhouette
            // and 0 everywhere else — what the outline post-pass
            // dilates.
            for (i, obj) in installed.gpu.objects().iter().enumerate() {
                let offset = (i as u64 * installed.gpu.model_stride()) as u32;
                pass.set_bind_group(1, &installed.models_bind_group, &[offset]);
                pass.set_vertex_buffer(0, obj.vertex_buffer().slice(..));
                pass.set_index_buffer(obj.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
                pass.set_stencil_reference(if Some(i) == selection { 1 } else { 0 });
                pass.draw_indexed(0..obj.index_count(), 0, 0..1);
            }
        }
        // Outline post-pass: only run when something is selected, since
        // an empty stencil yields no edges anyway and the extra draw
        // is wasted bandwidth on every unselected frame.
        if selection.is_some() {
            let mut outline_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh-viewer outline pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Preserve the resolved colour from the main
                        // pass; non-edge fragments `discard` so the
                        // existing pixels survive.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            outline_pass.set_pipeline(&self.outline_pipeline);
            outline_pass.set_bind_group(0, &msaa.outline_bind_group, &[]);
            outline_pass.draw(0..3, 0..1);
        }
        // Gizmo pass: drawn last so it sits on top of the mesh + the
        // outline. Depth is cleared at pass start so the gizmo is
        // never occluded by the mesh; depth-test stays enabled
        // *within* the pass so the three axes order correctly where
        // they overlap near the pivot.
        if let Some(frame) = gizmo {
            let uniforms = GizmoUniformsGpu {
                view_proj: view_proj.to_cols_array_2d(),
                pivot_and_scale: [frame.pivot.x, frame.pivot.y, frame.pivot.z, frame.scale],
            };
            self.queue.write_buffer(
                &self.gizmo_uniforms_buffer,
                0,
                bytemuck::bytes_of(&uniforms),
            );

            // Decide which categories to draw and write the matching
            // entry slots. Other slots are not used this frame, so we
            // skip writing them — the dynamic-offset bind only ever
            // points at slots that received fresh data.
            let render_translate = matches!(
                frame.visibility,
                GizmoVisibility::Idle | GizmoVisibility::DraggingTranslate { .. }
            );
            let render_rotate = matches!(
                frame.visibility,
                GizmoVisibility::Idle | GizmoVisibility::DraggingRotate { .. }
            );
            let render_scale = matches!(
                frame.visibility,
                GizmoVisibility::Idle
                    | GizmoVisibility::DraggingScaleAxis { .. }
                    | GizmoVisibility::DraggingScaleUniform
            );

            if render_translate {
                let entries = build_translate_entries(&frame);
                self.write_gizmo_entries(GIZMO_TRANSLATE_SLOT_BASE, &entries);
            }
            if render_rotate {
                let entries = build_rotate_entries(&frame, camera);
                self.write_gizmo_entries(GIZMO_ROTATE_SLOT_BASE, &entries);
            }
            if render_scale {
                let axis_entries = build_scale_axis_entries(&frame);
                self.write_gizmo_entries(GIZMO_SCALE_AXIS_SLOT_BASE, &axis_entries);
                let centre_entry = build_scale_centre_entry(&frame);
                self.write_gizmo_entries(GIZMO_CENTRE_SLOT, &[centre_entry]);
            }

            let mut gizmo_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh-viewer gizmo pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &msaa.gizmo_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            gizmo_pass.set_pipeline(&self.gizmo_pipeline);
            gizmo_pass.set_bind_group(0, &self.gizmo_uniforms_bind_group, &[]);

            if render_translate {
                gizmo_pass.set_vertex_buffer(0, self.gizmo_axis_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_axis_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for i in 0..3u32 {
                    let offset =
                        ((GIZMO_TRANSLATE_SLOT_BASE + i as u64) * self.gizmo_entry_stride) as u32;
                    gizmo_pass.set_bind_group(1, &self.gizmo_entries_bind_group, &[offset]);
                    // Translate handle = shaft + cone subrange.
                    gizmo_pass.draw_indexed(0..self.gizmo_axis_shaft_tip_index_count, 0, 0..1);
                }
            }
            if render_rotate {
                // Decorative torus arc.
                gizmo_pass.set_vertex_buffer(0, self.gizmo_arc_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_arc_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for i in 0..3u32 {
                    let offset =
                        ((GIZMO_ROTATE_SLOT_BASE + i as u64) * self.gizmo_entry_stride) as u32;
                    gizmo_pass.set_bind_group(1, &self.gizmo_entries_bind_group, &[offset]);
                    gizmo_pass.draw_indexed(0..self.gizmo_arc_index_count, 0, 0..1);
                }
                // Active two-cone spindle at the arc's midpoint —
                // shares the per-axis camera-rolled `axis_rotation`
                // so it lands at the visual centre of the arc.
                gizmo_pass.set_vertex_buffer(0, self.gizmo_rotate_cones_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_rotate_cones_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for i in 0..3u32 {
                    let offset =
                        ((GIZMO_ROTATE_SLOT_BASE + i as u64) * self.gizmo_entry_stride) as u32;
                    gizmo_pass.set_bind_group(1, &self.gizmo_entries_bind_group, &[offset]);
                    gizmo_pass.draw_indexed(0..self.gizmo_rotate_cones_index_count, 0, 0..1);
                }
            }
            if render_scale {
                gizmo_pass.set_vertex_buffer(0, self.gizmo_axis_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_axis_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for i in 0..3u32 {
                    let offset =
                        ((GIZMO_SCALE_AXIS_SLOT_BASE + i as u64) * self.gizmo_entry_stride) as u32;
                    gizmo_pass.set_bind_group(1, &self.gizmo_entries_bind_group, &[offset]);
                    // Scale-axis handle = midpoint-cube subrange.
                    gizmo_pass.draw_indexed(
                        self.gizmo_axis_shaft_tip_index_count..self.gizmo_axis_total_index_count,
                        0,
                        0..1,
                    );
                }

                gizmo_pass.set_vertex_buffer(0, self.gizmo_centre_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_centre_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                let offset = (GIZMO_CENTRE_SLOT * self.gizmo_entry_stride) as u32;
                gizmo_pass.set_bind_group(1, &self.gizmo_entries_bind_group, &[offset]);
                gizmo_pass.draw_indexed(0..self.gizmo_centre_index_count, 0, 0..1);
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        Some(target)
    }
}

fn create_color_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// End-to-end GPU tests. They construct a fresh wgpu device with the
/// **default-limits profile** (the same profile Slint uses), so they
/// catch regressions that would only show up at runtime — most
/// importantly, any binding type that is gated by a non-default
/// limit (storage buffers in the vertex stage have
/// `max_storage_buffers_per_shader_stage = 0` under default limits and
/// silently broke an earlier draft of this module).
///
/// The tests soft-skip when no adapter is available (CI environments
/// without GPU drivers), printing a warning. On a developer's
/// workstation with a working driver they exercise the full pipeline:
/// shader compilation, bind-group layout validation, per-frame uniform
/// writes, and a real draw call against a tiny scene.
#[cfg(test)]
mod gpu_integration_tests {
    use super::*;
    use crate::gizmo::Axis;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;

    /// Builds a wgpu (Device, Queue) with the default-limits profile
    /// Slint requests. Returns `None` if no adapter is available — the
    /// caller soft-skips so CI without a GPU isn't blocked.
    fn make_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mesh-viewer test device"),
            // Default-limits matches what Slint asks for. If a binding
            // we declare exceeds these limits, `Renderer::new` will
            // panic in `create_bind_group_layout` — that's the
            // regression we want this test to catch.
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        Some((device, queue))
    }

    fn triangle_scene_with_translation(t: AffineTransform3D) -> Scene {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 1, 2],
            }],
        };
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Affine(t),
                content: SceneObjectContent::Mesh(mesh),
            }],
        }
    }

    /// Smoke test: `Renderer::new` succeeds against the default-limits
    /// device. The original failure mode this guards against is a
    /// `create_bind_group_layout` validation error coming out of
    /// `Renderer::new`.
    #[test]
    fn renderer_construction_under_default_limits_does_not_panic() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let _renderer = Renderer::new(device, queue);
    }

    /// `GpuScene::build` succeeds against the default-limits device
    /// for a non-trivial scene with a non-identity transform.
    #[test]
    fn gpu_scene_build_succeeds_under_default_limits() {
        let Some((device, _queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let scene =
            triangle_scene_with_translation(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
        let gpu = GpuScene::build(&device, &scene).expect("GpuScene::build should succeed");
        // Stride must be at least one ModelEntry and a multiple of the
        // device's reported alignment.
        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        assert!(gpu.model_stride() >= MODEL_ENTRY_SIZE);
        assert_eq!(gpu.model_stride() % alignment, 0);
    }

    /// Full end-to-end render: build the renderer, install a scene
    /// with a non-identity transform, drive a render at a tiny size.
    /// The original storage-buffer-in-vertex bug surfaced as a runtime
    /// panic during `RenderingSetup`'s `Renderer::new`; this test
    /// reproduces that exact code path against the same limits and
    /// would have caught the regression before runtime.
    #[test]
    fn render_succeeds_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let scene =
            triangle_scene_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&scene)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let texture = renderer
            .render(&scene, &camera, None, None, 64, 64)
            .expect("render should produce a texture");
        assert_eq!(texture.size().width, 64);
        assert_eq!(texture.size().height, 64);
    }

    /// Identical to `render_succeeds_under_default_limits` but with a
    /// non-`None` selection so the `flags.x` lane carries a 1 through
    /// the per-frame uniform write. Catches any default-limits
    /// regression in the selection-flag path before runtime.
    #[test]
    fn render_with_selection_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let scene =
            triangle_scene_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&scene)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let texture = renderer
            .render(&scene, &camera, Some(0), None, 64, 64)
            .expect("render with selection should produce a texture");
        assert_eq!(texture.size().width, 64);
    }

    /// Outline post-pass smoke test. Drives a render with `Some(0)`
    /// so the stencil-aspect bind group is bound and the fullscreen
    /// edge-detect runs; pins that
    /// `RENDER_ATTACHMENT | TEXTURE_BINDING` on a multisampled
    /// `Depth24PlusStencil8` is accepted under the default-limits
    /// profile, plus that the `multisampled: true, sample_type: Uint`
    /// stencil binding declared in the layout matches the WGSL
    /// `texture_multisampled_2d<u32>` the outline shader expects.
    #[test]
    fn outline_post_pass_under_default_limits_is_validated() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let scene =
            triangle_scene_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&scene)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        // Twice — first frame creates the MSAA targets and outline
        // bind group; second exercises the cached path.
        let _ = renderer
            .render(&scene, &camera, Some(0), None, 64, 64)
            .expect("render with selection runs the outline pass");
        let _ = renderer
            .render(&scene, &camera, Some(0), None, 64, 64)
            .expect("cached outline bind group survives a second frame");
    }

    /// Gizmo pass smoke test: drive a render with `Some(GizmoFrame)`
    /// so the gizmo pipeline binds, the per-axis dynamic-offset
    /// uniform path runs, and the private depth attachment is
    /// validated under default limits. Two consecutive frames so the
    /// MSAA-targets cache + the gizmo depth view both go through the
    /// reuse path.
    #[test]
    fn render_with_gizmo_under_default_limits_is_validated() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let scene =
            triangle_scene_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&scene)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let frame = GizmoFrame {
            pivot: center,
            scale: 0.5,
            visibility: GizmoVisibility::DraggingTranslate { axis: Axis::X },
            locked_translate: [false, false, true],
            locked_rotate: [false; 3],
        };
        let _ = renderer
            .render(&scene, &camera, Some(0), Some(frame), 64, 64)
            .expect("first gizmo render should succeed");
        let _ = renderer
            .render(&scene, &camera, Some(0), Some(frame), 64, 64)
            .expect("cached gizmo resources survive a second frame");
    }

    /// Idle visibility renders all four handle categories
    /// simultaneously — exercises the translate, rotate, and scale
    /// draws plus the centre cube in a single frame.
    #[test]
    fn render_with_idle_gizmo_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let scene =
            triangle_scene_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&scene)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let frame = GizmoFrame {
            pivot: center,
            scale: 0.5,
            visibility: GizmoVisibility::Idle,
            locked_translate: [false; 3],
            locked_rotate: [false; 3],
        };
        let _ = renderer
            .render(&scene, &camera, Some(0), Some(frame), 64, 64)
            .expect("idle gizmo render should succeed");
    }

    /// Rotate-drag visibility renders only the three rotate arcs.
    /// Pins the rotate-arc VBO/IBO + per-axis camera-roll wiring
    /// under default-limits.
    #[test]
    fn render_with_rotate_gizmo_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let scene =
            triangle_scene_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&scene)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let frame = GizmoFrame {
            pivot: center,
            scale: 0.5,
            visibility: GizmoVisibility::DraggingRotate { axis: Axis::X },
            locked_translate: [false; 3],
            locked_rotate: [false, false, false],
        };
        let _ = renderer
            .render(&scene, &camera, Some(0), Some(frame), 64, 64)
            .expect("rotate gizmo render should succeed");
    }

    /// Scale-axis and uniform-scale variants both render the same
    /// pair of geometries (axis midpoint cubes + centre cube) — pins
    /// the scale-pass wiring and the centre-cube VBO/IBO.
    #[test]
    fn render_with_scale_gizmo_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let scene =
            triangle_scene_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&scene)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let scale_axis = GizmoFrame {
            pivot: center,
            scale: 0.5,
            visibility: GizmoVisibility::DraggingScaleAxis { axis: Axis::Y },
            locked_translate: [false; 3],
            locked_rotate: [false; 3],
        };
        let _ = renderer
            .render(&scene, &camera, Some(0), Some(scale_axis), 64, 64)
            .expect("scale-axis gizmo render should succeed");
        let scale_uniform = GizmoFrame {
            pivot: center,
            scale: 0.5,
            visibility: GizmoVisibility::DraggingScaleUniform,
            locked_translate: [false; 3],
            locked_rotate: [false; 3],
        };
        let _ = renderer
            .render(&scene, &camera, Some(0), Some(scale_uniform), 64, 64)
            .expect("scale-uniform gizmo render should succeed");
    }

    /// Multi-object scene drives one draw call per object; the
    /// dynamic-offset bind path must keep `model_entry` pointing at
    /// the right slot for each draw. Pure smoke test (no read-back),
    /// but it exercises the full draw loop with N > 1.
    #[test]
    fn render_multi_object_scene_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 1, 2],
            }],
        };
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(mesh.clone()),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Affine(AffineTransform3D::from_translation([
                        10.0, 0.0, 0.0,
                    ])),
                    content: SceneObjectContent::Mesh(mesh.clone()),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Affine(AffineTransform3D::from_translation([
                        0.0, 10.0, 0.0,
                    ])),
                    content: SceneObjectContent::Mesh(mesh),
                },
            ],
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        renderer.upload_scene(&scene).expect("upload");
        let bounds = renderer.scene_bounds().expect("bounds");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let _texture = renderer
            .render(&scene, &camera, None, None, 64, 64)
            .expect("render multi-object");
    }
}
