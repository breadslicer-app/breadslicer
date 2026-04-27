/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::borrow::Cow;
use std::num::NonZeroU64;

use anyhow::{Result, anyhow};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use sc_mesh_core::scene::{Scene, SceneObjectContent, Transform3D};
use sc_mesh_core::transform::AffineTransform3D;
use slint::wgpu_28::wgpu;
use wgpu::util::DeviceExt;

use crate::camera::{DEFAULT_DISTANCE_FACTOR, OrbitCamera};
use crate::gizmo::Axis;
use crate::gizmo_geometry::{self, GizmoVertex};

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
struct GpuVertex {
    position: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
}

/// Per-object entry in the model uniform buffer. Mirrors the WGSL
/// `ModelEntry` struct one-for-one. `flags.x` is the selection bit
/// (1 for the selected object, 0 otherwise); the remaining lanes are
/// reserved for future per-object hints (hover, gizmo focus, …).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct ModelEntry {
    model: [[f32; 4]; 4],
    flags: [u32; 4],
}

const MODEL_ENTRY_SIZE: u64 = std::mem::size_of::<ModelEntry>() as u64;

/// Per-frame uniform for the gizmo pass: the camera transform plus
/// the world-space pivot and shaft-length, both packed into a vec4
/// so the shader gets one contiguous load.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct GizmoUniformsGpu {
    view_proj: [[f32; 4]; 4],
    pivot_and_scale: [f32; 4],
}

/// Per-axis draw entry uploaded to the dynamic-offset uniform buffer.
/// Mirrors `DrawEntry` in `gizmo.wgsl` one-for-one.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct GizmoDrawEntry {
    axis_rotation: [[f32; 4]; 4],
    tint: [f32; 4],
}

const GIZMO_DRAW_ENTRY_SIZE: u64 = std::mem::size_of::<GizmoDrawEntry>() as u64;

/// Total number of per-frame draw entries reserved in
/// `gizmo_entries_buffer`. The slots are laid out so each pass
/// binds a contiguous range:
/// - `[0, 3)` translate (one per X/Y/Z),
/// - `[3, 6)` rotate (one per X/Y/Z),
/// - `[6, 9)` scale-axis midpoint cubes (one per X/Y/Z),
/// - `[9, 10)` centre cube (uniform-scale).
const GIZMO_ENTRY_SLOTS: u64 = 10;
const GIZMO_TRANSLATE_SLOT_BASE: u64 = 0;
const GIZMO_ROTATE_SLOT_BASE: u64 = 3;
const GIZMO_SCALE_AXIS_SLOT_BASE: u64 = 6;
const GIZMO_CENTRE_SLOT: u64 = 9;

/// Per-frame data the renderer needs to draw the gizmo. `pivot` is
/// the world-space gizmo origin; `scale` is the world-unit length of
/// the translate shaft (so the gizmo subtends a constant pixel size);
/// `visibility` tells the renderer which handle types to draw and
/// which axis (if any) is the currently-active drag target — handles
/// matching the active axis render at full brightness, the rest at
/// dim brightness. `locked_translate` / `locked_rotate` flag axes the
/// user can't grab right now (matching `gizmo::axis_pickable` and
/// `gizmo::arc_pickable` respectively); locked axes render greyed.
#[derive(Copy, Clone, Debug)]
pub struct GizmoFrame {
    pub pivot: Vec3,
    pub scale: f32,
    pub visibility: GizmoVisibility,
    /// Indexed [X, Y, Z]. Greys the translate cone *and* the per-axis
    /// scale midpoint cube for that axis (both share the closest-
    /// point-on-axis math and thus the same lock-out condition).
    pub locked_translate: [bool; 3],
    /// Indexed [X, Y, Z]. Greys the rotate arc for that axis.
    pub locked_rotate: [bool; 3],
}

/// Which gizmo handle category is currently visible (and which axis,
/// if any, is the active drag target). The rule:
/// - `Idle`: render all four handle categories (translate cones,
///   midpoint scale cubes, rotate arcs, centre cube). No axis is
///   highlighted as "active".
/// - `Dragging*`: render only the *category* the drag belongs to.
///   The named axis is the active drag target and renders bright;
///   other axes in the same category render dim.
///
/// `DraggingScaleAxis` and `DraggingScaleUniform` both render the
/// same category (the scale group: 3 midpoint cubes + the centre
/// cube). They differ only in which sub-handle is highlighted.
//
// `DraggingRotate` / `DraggingScaleAxis` / `DraggingScaleUniform`
// have no controller-side producer until the FSM extension lands;
// they're consumed by tests today.
#[allow(dead_code)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GizmoVisibility {
    Idle,
    DraggingTranslate { axis: Axis },
    DraggingRotate { axis: Axis },
    DraggingScaleAxis { axis: Axis },
    DraggingScaleUniform,
}

/// Round `size` up to the next multiple of `alignment`. Used to space
/// successive `ModelEntry` slots in the per-scene uniform buffer at the
/// device's `min_uniform_buffer_offset_alignment`. Both inputs are u64
/// and non-zero in practice; the `alignment - 1` mask trick is the
/// standard `assume-power-of-two` form used everywhere wgpu samples
/// align uniform-buffer offsets.
const fn align_up(size: u64, alignment: u64) -> u64 {
    (size + alignment - 1) & !(alignment - 1)
}

/// GPU-resident draw data for a single `SceneObject`: its local-space
/// vertex+index buffers (the object's transform is *not* baked in — it
/// arrives at draw time through the model storage buffer).
struct ObjectDraw {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

/// GPU mirror of a `Scene`. Holds one [`ObjectDraw`] per object plus a
/// uniform buffer of [`ModelEntry`] slots, padded to the device's
/// `min_uniform_buffer_offset_alignment` so the render loop can address
/// each slot via a dynamic-offset bind. The world-space AABB is captured
/// at build time from `Scene::aabb`; it is used once for camera framing
/// and is not refreshed on per-frame transform updates (the camera
/// persists across edits, so a post-edit AABB is irrelevant for
/// framing).
///
/// Construction ([`GpuScene::build`]) touches a `wgpu::Device` but not
/// the UI loop, so it can run on the worker thread that parsed the
/// scene; the UI thread then attaches the matching bind group via
/// [`Renderer::install_scene`].
pub(crate) struct GpuScene {
    objects: Vec<ObjectDraw>,
    model_buffer: wgpu::Buffer,
    /// Distance in bytes between consecutive `ModelEntry` slots in
    /// `model_buffer`. Always ≥ `MODEL_ENTRY_SIZE` and a multiple of
    /// the device's `min_uniform_buffer_offset_alignment`.
    model_stride: u64,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

impl GpuScene {
    pub(crate) fn build(device: &wgpu::Device, scene: &Scene) -> Result<Self> {
        let data = build_gpu_scene_data(scene)?;
        let mut objects = Vec::with_capacity(data.objects.len());
        for obj in &data.objects {
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer object vertex buffer"),
                contents: bytemuck::cast_slice(&obj.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer object index buffer"),
                contents: bytemuck::cast_slice(&obj.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            objects.push(ObjectDraw {
                vertex_buffer,
                index_buffer,
                index_count: obj.indices.len() as u32,
            });
        }

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let model_stride = align_up(MODEL_ENTRY_SIZE, alignment);

        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer model uniform"),
            // The bind group requires a non-zero binding size; we already
            // bail above if the scene has no objects.
            size: model_stride * objects.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            objects,
            model_buffer,
            model_stride,
            aabb_min: data.aabb_min,
            aabb_max: data.aabb_max,
        })
    }

    fn object_count(&self) -> usize {
        self.objects.len()
    }
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
        self.scene
            .as_ref()
            .map(|s| (s.gpu.aabb_min, s.gpu.aabb_max))
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
                    buffer: &gpu.model_buffer,
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
            let offset = i as u64 * installed.gpu.model_stride;
            self.queue.write_buffer(
                &installed.gpu.model_buffer,
                offset,
                bytemuck::bytes_of(entry),
            );
        }

        let diagonal = (installed.gpu.aabb_max - installed.gpu.aabb_min)
            .length()
            .max(1e-3);

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
            for (i, obj) in installed.gpu.objects.iter().enumerate() {
                let offset = (i as u64 * installed.gpu.model_stride) as u32;
                pass.set_bind_group(1, &installed.models_bind_group, &[offset]);
                pass.set_vertex_buffer(0, obj.vertex_buffer.slice(..));
                pass.set_index_buffer(obj.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_stencil_reference(if Some(i) == selection { 1 } else { 0 });
                pass.draw_indexed(0..obj.index_count, 0, 0..1);
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

/// Translates `AffineTransform3D`'s row-major 3×3 + translation form
/// into glam's column-major 4×4. f64 internals are downcast to f32
/// here at the GPU boundary.
fn affine_to_mat4(a: &AffineTransform3D) -> Mat4 {
    let l = &a.linear;
    let t = &a.translation;
    Mat4::from_cols(
        Vec4::new(l[0][0] as f32, l[1][0] as f32, l[2][0] as f32, 0.0),
        Vec4::new(l[0][1] as f32, l[1][1] as f32, l[2][1] as f32, 0.0),
        Vec4::new(l[0][2] as f32, l[1][2] as f32, l[2][2] as f32, 0.0),
        Vec4::new(t[0] as f32, t[1] as f32, t[2] as f32, 1.0),
    )
}

/// Per-axis colours (full saturation). The renderer modulates these
/// with a brightness factor that depends on the axis's active/locked
/// state for the current handle category.
const fn axis_base_color(axis: Axis) -> [f32; 3] {
    match axis {
        Axis::X => [1.0, 0.25, 0.25],
        Axis::Y => [0.25, 1.0, 0.25],
        Axis::Z => [0.35, 0.45, 1.0],
    }
}

/// Brightness multipliers for the three handle states. Active is
/// the drag-target axis (or hover-target, conceptually identical);
/// inactive axes still render visibly so the user can see the
/// gizmo, just at reduced contrast.
const TINT_ACTIVE: f32 = 1.0;
const TINT_INACTIVE: f32 = 0.65;
const TINT_LOCKED: f32 = 0.35;

fn tint_for(axis: Axis, active: bool, locked: bool) -> [f32; 4] {
    if locked {
        return [TINT_LOCKED, TINT_LOCKED, TINT_LOCKED, 1.0];
    }
    let brightness = if active { TINT_ACTIVE } else { TINT_INACTIVE };
    let [r, g, b] = axis_base_color(axis);
    [r * brightness, g * brightness, b * brightness, 1.0]
}

/// Locked grey for the centre cube — it has no per-axis lock-out
/// concept so this function returns a fixed greyscale palette,
/// dim/bright following the visibility's "uniform-scale active"
/// flag.
fn centre_tint(active: bool) -> [f32; 4] {
    let g = if active { TINT_ACTIVE } else { TINT_INACTIVE };
    [g, g, g, 1.0]
}

/// Rotation that maps handle-local +X onto each world axis. Indexed
/// in the order `[X, Y, Z]`.
fn axis_world_rotations() -> [Mat4; 3] {
    use std::f32::consts::FRAC_PI_2;
    [
        Mat4::IDENTITY,
        Mat4::from_rotation_z(FRAC_PI_2),
        Mat4::from_rotation_y(-FRAC_PI_2),
    ]
}

/// Three translate-pass entries (X/Y/Z), one shaft+cone draw each.
fn build_translate_entries(frame: &GizmoFrame) -> [GizmoDrawEntry; 3] {
    let rotations = axis_world_rotations();
    let axes = [Axis::X, Axis::Y, Axis::Z];
    let active_axis = match frame.visibility {
        GizmoVisibility::DraggingTranslate { axis } => Some(axis),
        _ => None,
    };
    let mut out = [GizmoDrawEntry {
        axis_rotation: [[0.0; 4]; 4],
        tint: [0.0; 4],
    }; 3];
    for (i, axis) in axes.into_iter().enumerate() {
        let active = active_axis == Some(axis);
        out[i] = GizmoDrawEntry {
            axis_rotation: rotations[i].to_cols_array_2d(),
            tint: tint_for(axis, active, frame.locked_translate[i]),
        };
    }
    out
}

/// Three scale-axis-pass entries (X/Y/Z midpoint cubes), sharing
/// the per-axis world rotation with translate. The cube sub-range
/// of the axis IBO carries the geometry; only the tint differs from
/// the translate entry. The scale-axis lock-out is the same as
/// translate's (closest-point-on-axis math).
fn build_scale_axis_entries(frame: &GizmoFrame) -> [GizmoDrawEntry; 3] {
    let rotations = axis_world_rotations();
    let axes = [Axis::X, Axis::Y, Axis::Z];
    let active_axis = match frame.visibility {
        GizmoVisibility::DraggingScaleAxis { axis } => Some(axis),
        _ => None,
    };
    let mut out = [GizmoDrawEntry {
        axis_rotation: [[0.0; 4]; 4],
        tint: [0.0; 4],
    }; 3];
    for (i, axis) in axes.into_iter().enumerate() {
        let active = active_axis == Some(axis);
        out[i] = GizmoDrawEntry {
            axis_rotation: rotations[i].to_cols_array_2d(),
            tint: tint_for(axis, active, frame.locked_translate[i]),
        };
    }
    out
}

/// Centre-cube entry. Identity rotation (the cube is axis-aligned
/// at the pivot) and a greyscale tint so the centre handle reads as
/// a single "uniform-scale" target rather than a fourth axis.
fn build_scale_centre_entry(frame: &GizmoFrame) -> GizmoDrawEntry {
    let active = matches!(frame.visibility, GizmoVisibility::DraggingScaleUniform);
    GizmoDrawEntry {
        axis_rotation: Mat4::IDENTITY.to_cols_array_2d(),
        tint: centre_tint(active),
    }
}

/// Three rotate-pass entries (X/Y/Z arcs). Each axis bakes a
/// per-frame X-roll into its `axis_rotation` so the 90° arc faces
/// the camera in handle-local space *before* the world-axis rotation
/// maps it onto +X / +Y / +Z.
fn build_rotate_entries(frame: &GizmoFrame, camera: &OrbitCamera) -> [GizmoDrawEntry; 3] {
    let rotations = axis_world_rotations();
    let axes = [Axis::X, Axis::Y, Axis::Z];
    let active_axis = match frame.visibility {
        GizmoVisibility::DraggingRotate { axis } => Some(axis),
        _ => None,
    };
    let mut out = [GizmoDrawEntry {
        axis_rotation: [[0.0; 4]; 4],
        tint: [0.0; 4],
    }; 3];
    for (i, axis) in axes.into_iter().enumerate() {
        let roll = arc_camera_roll(camera, frame.pivot, axis);
        // Compose: world_axis_rotation × X-roll. The arc is laid out
        // around handle-local +X, centred on +Y; rolling it around
        // +X swings the centre onto the camera-facing direction
        // before the world-axis rotation lifts it onto X/Y/Z.
        let composed = rotations[i] * Mat4::from_rotation_x(roll);
        let active = active_axis == Some(axis);
        out[i] = GizmoDrawEntry {
            axis_rotation: composed.to_cols_array_2d(),
            tint: tint_for(axis, active, frame.locked_rotate[i]),
        };
    }
    out
}

/// Per-axis X-roll in handle-local space that swings the rotate arc
/// onto the camera-facing 90° quadrant.
///
/// In handle-local space the arc is centred on +Y, spanning ±π/4. We
/// want the centre of the arc to align with the camera direction
/// projected into the handle-local YZ plane. The roll is the angle
/// from +Y to that projection, measured CCW about +X (right-hand
/// rule). When the camera is on the axis itself (rotate lock-out),
/// the projection is degenerate; the controller's lock-out should
/// already have removed the axis from the visible set, but the
/// geometry guards against div-by-zero with a fallback to `0.0`.
fn arc_camera_roll(camera: &OrbitCamera, pivot: Vec3, axis: Axis) -> f32 {
    // Camera direction in world space, then expressed in handle-
    // local coords by inverting the world-axis rotation. The
    // rotation matrix is orthogonal, so the inverse is the
    // transpose — but glam's `inverse` on rigid rotations is cheap
    // enough to avoid premature optimisation here.
    let world_to_local = world_to_handle_local(axis);
    let to_camera_world = camera.eye() - pivot;
    let to_camera_local = world_to_local.transform_vector3(to_camera_world);
    // YZ-plane projection (drop +X axis component).
    let y = to_camera_local.y;
    let z = to_camera_local.z;
    if y.hypot(z) < 1e-6 {
        return 0.0;
    }
    z.atan2(y)
}

/// Inverse of the per-axis world rotation: maps world-space vectors
/// into the axis's handle-local frame. The world rotation maps
/// handle-local +X onto the world axis; the inverse undoes that.
fn world_to_handle_local(axis: Axis) -> Mat4 {
    use std::f32::consts::FRAC_PI_2;
    match axis {
        Axis::X => Mat4::IDENTITY,
        // World rotation is Rz(+π/2); inverse is Rz(-π/2).
        Axis::Y => Mat4::from_rotation_z(-FRAC_PI_2),
        // World rotation is Ry(-π/2); inverse is Ry(+π/2).
        Axis::Z => Mat4::from_rotation_y(FRAC_PI_2),
    }
}

fn transform_to_mat4(t: &Transform3D) -> Mat4 {
    match t {
        Transform3D::Identity => Mat4::IDENTITY,
        Transform3D::Affine(a) => affine_to_mat4(a),
    }
}

/// `selection` is an index into `scene.objects`; out-of-bounds values
/// (or `None`) mean nothing is selected and every entry's `flags.x`
/// stays 0. The shader reads `flags.x` and mixes a warm tint when it
/// is 1.
fn build_model_entries(scene: &Scene, selection: Option<usize>) -> Vec<ModelEntry> {
    scene
        .objects
        .iter()
        .enumerate()
        .map(|(idx, obj)| {
            let selected = selection == Some(idx);
            ModelEntry {
                model: transform_to_mat4(&obj.transform).to_cols_array_2d(),
                flags: [u32::from(selected), 0, 0, 0],
            }
        })
        .collect()
}

/// CPU-only build of the per-object draw data plus the world-space
/// scene AABB. Kept separate from buffer creation so the transformation
/// can be unit-tested without a `wgpu::Device`.
struct ObjectData {
    vertices: Vec<GpuVertex>,
    indices: Vec<u32>,
}

struct SceneData {
    objects: Vec<ObjectData>,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

fn build_gpu_scene_data(scene: &Scene) -> Result<SceneData> {
    if scene.objects.is_empty() {
        return Err(anyhow!("Scene has no geometry to render"));
    }

    let mut objects: Vec<ObjectData> = Vec::with_capacity(scene.objects.len());

    for object in &scene.objects {
        let mesh = match &object.content {
            SceneObjectContent::Mesh(m) => m,
            SceneObjectContent::Resource(idx) => scene
                .resources
                .get(*idx)
                .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
        };

        if mesh.faces.is_empty() {
            // An object with no geometry contributes nothing to the
            // draw set — skip it but keep its slot in the model
            // buffer so `instance_index` keeps lining up with
            // `Scene::objects` indices in later phases. We achieve
            // that by emitting a zero-vertex / zero-index ObjectData,
            // which the renderer's per-object loop draws as a no-op
            // (index_count = 0).
            objects.push(ObjectData {
                vertices: Vec::new(),
                indices: Vec::new(),
            });
            continue;
        }

        let vertices: Vec<GpuVertex> = mesh
            .vertices
            .iter()
            .map(|v| GpuVertex { position: v.0 })
            .collect();

        let mut indices: Vec<u32> = Vec::with_capacity(mesh.faces.len() * 3);
        for face in &mesh.faces {
            for &vidx in &face.vertices {
                indices.push(vidx);
            }
        }

        objects.push(ObjectData { vertices, indices });
    }

    // World-space scene AABB via `Scene::aabb`, which already does Arvo
    // per object and respects each object's `Transform3D`. If every
    // object is empty, fall back to a degenerate bbox at the origin
    // (the renderer's diagonal-clamp `.max(1e-3)` keeps the projection
    // matrix well-defined).
    let (aabb_min, aabb_max) = match scene.aabb()? {
        Some((min, max)) => (Vec3::from(min.0), Vec3::from(max.0)),
        None => (Vec3::ZERO, Vec3::ZERO),
    };

    Ok(SceneData {
        objects,
        aabb_min,
        aabb_max,
    })
}

#[cfg(test)]
mod build_gpu_scene_data_tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;

    fn triangle_mesh(vertices: Vec<[f32; 3]>, face_vertices: [u32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vertices.into_iter().map(Vertex::new).collect(),
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: face_vertices,
            }],
        }
    }

    fn inline_object(mesh: IndexedMesh) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn affine_object(mesh: IndexedMesh, t: AffineTransform3D) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Affine(t),
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn resource_object(idx: usize) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Resource(idx),
        }
    }

    #[test]
    fn empty_scene_returns_error() {
        let scene = Scene::default();
        assert!(build_gpu_scene_data(&scene).is_err());
    }

    #[test]
    fn dangling_resource_index_returns_error() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![resource_object(7)],
        };
        assert!(build_gpu_scene_data(&scene).is_err());
    }

    #[test]
    fn valid_resource_reference_is_resolved() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![mesh.clone()],
            objects: vec![resource_object(0)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects.len(), 1);
        assert_eq!(data.objects[0].vertices.len(), 3);
        assert_eq!(data.objects[0].indices, vec![0, 1, 2]);
    }

    #[test]
    fn vertices_stay_local_under_identity() {
        let mesh = triangle_mesh(
            vec![[1.0, -2.0, 3.0], [4.0, 5.0, -6.0], [0.0, 0.0, 0.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(mesh)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects.len(), 1);
        assert_eq!(data.objects[0].vertices[0].position, [1.0, -2.0, 3.0]);
        assert_eq!(data.objects[0].vertices[1].position, [4.0, 5.0, -6.0]);
        assert_eq!(data.objects[0].vertices[2].position, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn vertices_stay_local_even_with_affine_transform() {
        // Vertex buffers are local-space; the transform is applied at
        // draw time via the per-object model uniform, so
        // `build_gpu_scene_data` must NOT bake it into the vertex
        // stream.
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                mesh,
                AffineTransform3D::from_translation([100.0, -50.0, 25.0]),
            )],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects[0].vertices[0].position, [1.0, 0.0, 0.0]);
        assert_eq!(data.objects[0].vertices[1].position, [0.0, 1.0, 0.0]);
        assert_eq!(data.objects[0].vertices[2].position, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn indices_are_local_per_object_not_offset() {
        // With per-object draws, each object's index buffer references its
        // own vertex buffer, so indices stay local — not offset by
        // previous objects' vertex counts as the merged build did.
        let a = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let b = triangle_mesh(
            vec![[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(a), inline_object(b)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects.len(), 2);
        assert_eq!(data.objects[0].indices, vec![0, 1, 2]);
        assert_eq!(data.objects[1].indices, vec![0, 1, 2]);
    }

    #[test]
    fn scene_aabb_reflects_world_space_under_transforms() {
        let mesh = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                mesh,
                AffineTransform3D::from_translation([10.0, 20.0, 30.0]),
            )],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        // Scene AABB = local AABB ([0,0,0]..[1,1,0]) translated by (10,20,30).
        assert_approx_eq!(f32, data.aabb_min.x, 10.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_min.y, 20.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_min.z, 30.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.x, 11.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.y, 21.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.z, 30.0, epsilon = 1e-5);
    }

    #[test]
    fn scene_aabb_unions_two_objects() {
        let a = triangle_mesh(
            vec![[1.0, -2.0, 3.0], [4.0, 5.0, -6.0], [0.0, 0.0, 0.0]],
            [0, 1, 2],
        );
        let b = triangle_mesh(
            vec![[-7.0, 8.0, 0.5], [0.0, 0.0, 9.0], [2.0, 2.0, 2.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(a), inline_object(b)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.aabb_min, Vec3::new(-7.0, -2.0, -6.0));
        assert_eq!(data.aabb_max, Vec3::new(4.0, 8.0, 9.0));
    }

    #[test]
    fn empty_object_keeps_a_slot_in_the_object_list() {
        // An object with no faces still occupies an index in
        // `Scene::objects`; the renderer indexes into `models[]` by that
        // index, so the GpuScene must keep a parallel slot. The slot
        // must be drawable as a no-op (index_count = 0).
        let empty = IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        };
        let nonempty = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(empty), inline_object(nonempty)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects.len(), 2);
        assert!(data.objects[0].indices.is_empty());
        assert_eq!(data.objects[1].indices.len(), 3);
    }

    #[test]
    fn singular_transform_propagates_error() {
        // `Scene::aabb` itself doesn't fail on singular linear parts
        // (it uses Arvo), so this currently succeeds. The check here
        // pins the behaviour: the per-object data is built without
        // touching `apply_to_normal`, so even a degenerate transform
        // doesn't bring the build down — degenerate matrices only
        // matter at `merge_meshes` / export time.
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(mesh, singular)],
        };
        // Singular transforms aren't rejected here — the GPU path is
        // fine with a singular model matrix (it just collapses the
        // mesh to a plane). Mirror this in the assertion so the test
        // fails loudly if the behaviour changes.
        assert!(build_gpu_scene_data(&scene).is_ok());
    }
}

#[cfg(test)]
mod transform_to_mat4_tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::transform::AffineTransform3D;

    fn assert_mat4_approx(a: Mat4, b: Mat4, eps: f32) {
        let a = a.to_cols_array();
        let b = b.to_cols_array();
        for (x, y) in a.iter().zip(b.iter()) {
            assert_approx_eq!(f32, *x, *y, epsilon = eps);
        }
    }

    #[test]
    fn identity_transform_is_identity_matrix() {
        let m = transform_to_mat4(&Transform3D::Identity);
        assert_mat4_approx(m, Mat4::IDENTITY, 1e-6);
    }

    #[test]
    fn translation_lands_in_the_last_column() {
        let m = transform_to_mat4(&Transform3D::Affine(AffineTransform3D::from_translation([
            10.0, -5.0, 2.5,
        ])));
        // Glam stores column-major; the translation column is index 3.
        let translation = m.col(3);
        assert_approx_eq!(f32, translation.x, 10.0, epsilon = 1e-6);
        assert_approx_eq!(f32, translation.y, -5.0, epsilon = 1e-6);
        assert_approx_eq!(f32, translation.z, 2.5, epsilon = 1e-6);
        assert_approx_eq!(f32, translation.w, 1.0, epsilon = 1e-6);
    }

    #[test]
    fn affine_to_mat4_matches_apply_to_vertex() {
        // The matrix should transform vertices the same way the
        // mesh-core `apply_to_vertex` does — that is the contract the
        // shader relies on.
        let a = AffineTransform3D::from_translation([1.0, 2.0, 3.0]).compose(
            &AffineTransform3D::from_rotation_z(std::f64::consts::FRAC_PI_4)
                .compose(&AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0])),
        );
        let m = affine_to_mat4(&a);
        for v in [[1.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            let by_matrix = m.transform_point3(Vec3::from(v));
            let by_apply = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new(v));
            assert_approx_eq!(f32, by_matrix.x, by_apply.0[0], epsilon = 1e-5);
            assert_approx_eq!(f32, by_matrix.y, by_apply.0[1], epsilon = 1e-5);
            assert_approx_eq!(f32, by_matrix.z, by_apply.0[2], epsilon = 1e-5);
        }
    }

    #[test]
    fn build_model_entries_one_per_object() {
        use sc_mesh_core::IndexedMesh;
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
        use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 0, 0],
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
                        7.0, 0.0, 0.0,
                    ])),
                    content: SceneObjectContent::Mesh(mesh),
                },
            ],
        };
        let entries = build_model_entries(&scene, None);
        assert_eq!(entries.len(), 2);
        // First entry is identity.
        assert_approx_eq!(f32, entries[0].model[0][0], 1.0);
        assert_approx_eq!(f32, entries[0].model[3][0], 0.0);
        // Second entry has +7 in the translation slot (column 3, row 0).
        assert_approx_eq!(f32, entries[1].model[3][0], 7.0);
        assert_approx_eq!(f32, entries[1].model[3][1], 0.0);
        assert_approx_eq!(f32, entries[1].model[3][2], 0.0);
    }

    fn two_object_scene() -> Scene {
        use sc_mesh_core::IndexedMesh;
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
        use sc_mesh_core::scene::{SceneObject, SceneObjectContent};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 0, 0],
            }],
        };
        Scene {
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
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(mesh),
                },
            ],
        }
    }

    #[test]
    fn no_selection_leaves_every_flags_x_zero() {
        let scene = two_object_scene();
        let entries = build_model_entries(&scene, None);
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 0);
    }

    #[test]
    fn selected_index_sets_only_that_entrys_flags_x() {
        let scene = two_object_scene();
        let entries = build_model_entries(&scene, Some(1));
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 1);
    }

    #[test]
    fn out_of_bounds_selection_falls_back_to_no_highlight() {
        let scene = two_object_scene();
        let entries = build_model_entries(&scene, Some(99));
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 0);
    }
}

#[cfg(test)]
mod gizmo_entry_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn frame_with(visibility: GizmoVisibility, locked_translate: [bool; 3]) -> GizmoFrame {
        GizmoFrame {
            pivot: Vec3::ZERO,
            scale: 1.0,
            visibility,
            locked_translate,
            locked_rotate: [false; 3],
        }
    }

    fn idle_frame() -> GizmoFrame {
        frame_with(GizmoVisibility::Idle, [false; 3])
    }

    #[test]
    fn translate_axis_rotations_map_handle_local_x_to_world_axes() {
        let entries = build_translate_entries(&idle_frame());
        let local_x = Vec4::new(1.0, 0.0, 0.0, 1.0);
        let rx = Mat4::from_cols_array_2d(&entries[0].axis_rotation) * local_x;
        let ry = Mat4::from_cols_array_2d(&entries[1].axis_rotation) * local_x;
        let rz = Mat4::from_cols_array_2d(&entries[2].axis_rotation) * local_x;
        assert_approx_eq!(f32, rx.x, 1.0, epsilon = 1e-5);
        assert_approx_eq!(f32, ry.y, 1.0, epsilon = 1e-5);
        assert_approx_eq!(f32, rz.z, 1.0, epsilon = 1e-5);
    }

    #[test]
    fn idle_visibility_yields_inactive_brightness_for_all_translate_axes() {
        let entries = build_translate_entries(&idle_frame());
        // Inactive brightness multiplier is 0.65; X base is (1.0, 0.25, 0.25).
        assert_approx_eq!(f32, entries[0].tint[0], 0.65, epsilon = 1e-5);
        assert_approx_eq!(f32, entries[0].tint[1], 0.25 * 0.65, epsilon = 1e-5);
    }

    #[test]
    fn dragging_translate_renders_active_axis_at_full_brightness() {
        let entries = build_translate_entries(&frame_with(
            GizmoVisibility::DraggingTranslate { axis: Axis::X },
            [false; 3],
        ));
        assert_approx_eq!(f32, entries[0].tint[0], 1.0, epsilon = 1e-5);
        // Y stays inactive — only the dragging axis brightens.
        assert_approx_eq!(f32, entries[1].tint[1], 1.0 * 0.65, epsilon = 1e-5);
    }

    #[test]
    fn locked_axis_renders_grey_regardless_of_active_state() {
        let entries = build_translate_entries(&frame_with(
            GizmoVisibility::DraggingTranslate { axis: Axis::X },
            [true, false, false],
        ));
        assert_approx_eq!(f32, entries[0].tint[0], 0.35, epsilon = 1e-5);
        assert_approx_eq!(f32, entries[0].tint[1], 0.35, epsilon = 1e-5);
        assert_approx_eq!(f32, entries[0].tint[2], 0.35, epsilon = 1e-5);
    }

    #[test]
    fn dragging_rotate_does_not_brighten_translate_axes() {
        // Visibility carries the active *for the rotate category*;
        // the translate entries should see no active axis even
        // though `axis: X` is named.
        let entries = build_translate_entries(&frame_with(
            GizmoVisibility::DraggingRotate { axis: Axis::X },
            [false; 3],
        ));
        assert_approx_eq!(f32, entries[0].tint[0], 0.65, epsilon = 1e-5);
    }

    #[test]
    fn dragging_scale_axis_brightens_only_the_matching_midpoint_cube() {
        let entries = build_scale_axis_entries(&frame_with(
            GizmoVisibility::DraggingScaleAxis { axis: Axis::Y },
            [false; 3],
        ));
        // Y axis is active.
        assert_approx_eq!(f32, entries[1].tint[1], 1.0, epsilon = 1e-5);
        // X / Z stay inactive.
        assert_approx_eq!(f32, entries[0].tint[0], 0.65, epsilon = 1e-5);
        assert_approx_eq!(f32, entries[2].tint[2], 0.65, epsilon = 1e-5);
    }

    #[test]
    fn scale_axis_inherits_translate_lockout() {
        // The midpoint cubes share the closest-point-on-axis math
        // with the translate cones, so they must grey together.
        let entries = build_scale_axis_entries(&frame_with(
            GizmoVisibility::DraggingScaleAxis { axis: Axis::X },
            [true, false, false],
        ));
        assert_approx_eq!(f32, entries[0].tint[0], 0.35, epsilon = 1e-5);
    }

    #[test]
    fn centre_cube_is_grey_and_active_only_under_uniform_scale() {
        let idle = build_scale_centre_entry(&idle_frame());
        // All three RGB channels equal under greyscale tint.
        assert_approx_eq!(f32, idle.tint[0], idle.tint[1], epsilon = 1e-6);
        assert_approx_eq!(f32, idle.tint[1], idle.tint[2], epsilon = 1e-6);
        // Inactive brightness 0.65.
        assert_approx_eq!(f32, idle.tint[0], 0.65, epsilon = 1e-5);

        let active = build_scale_centre_entry(&frame_with(
            GizmoVisibility::DraggingScaleUniform,
            [false; 3],
        ));
        assert_approx_eq!(f32, active.tint[0], 1.0, epsilon = 1e-5);
    }

    #[test]
    fn rotate_arc_roll_aligns_with_camera_in_handle_local_yz_plane() {
        // Camera at +Z above the pivot. For the X-axis arc (axis = +X
        // in handle-local space), the camera-facing direction projects
        // onto the YZ plane as +Z. The roll should bring +Y onto +Z,
        // i.e. +π/2 about +X. Loose tolerance because `set_axis_view`
        // uses a clamped pitch slightly less than π/2 to avoid the
        // gimbal-lock degenerate, so the eye sits ~1e-3 off the +Z
        // axis.
        let mut camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        camera.set_axis_view(crate::camera::AxisView::Top);
        let pivot = Vec3::ZERO;
        let roll = arc_camera_roll(&camera, pivot, Axis::X);
        assert_approx_eq!(f32, roll, std::f32::consts::FRAC_PI_2, epsilon = 1e-2);
    }

    #[test]
    fn rotate_locked_axis_renders_grey() {
        let mut frame = idle_frame();
        frame.visibility = GizmoVisibility::DraggingRotate { axis: Axis::X };
        frame.locked_rotate = [true, false, false];
        let camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        let entries = build_rotate_entries(&frame, &camera);
        assert_approx_eq!(f32, entries[0].tint[0], 0.35, epsilon = 1e-5);
    }
}

#[cfg(test)]
mod align_up_tests {
    use super::*;

    #[test]
    fn align_up_pads_to_next_alignment_boundary() {
        assert_eq!(align_up(80, 256), 256);
        assert_eq!(align_up(257, 256), 512);
    }

    #[test]
    fn align_up_is_identity_when_size_is_a_multiple() {
        assert_eq!(align_up(256, 256), 256);
        assert_eq!(align_up(512, 256), 512);
    }

    #[test]
    fn align_up_handles_alignment_one() {
        // alignment = 1 means no padding; size passes through unchanged.
        assert_eq!(align_up(1, 1), 1);
        assert_eq!(align_up(80, 1), 80);
    }
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
        assert!(gpu.model_stride >= MODEL_ENTRY_SIZE);
        assert_eq!(gpu.model_stride % alignment, 0);
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
