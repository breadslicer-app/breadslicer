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

const SHADER_SOURCE: &str = include_str!("shaders/mesh_render.wgsl");

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
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
/// `ModelEntry` struct one-for-one. `flags.x` is reserved for the
/// selection bit (Phase B); the remaining lanes are zero today.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct ModelEntry {
    model: [[f32; 4]; 4],
    flags: [u32; 4],
}

const MODEL_ENTRY_SIZE: u64 = std::mem::size_of::<ModelEntry>() as u64;

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
    uniform_buffer: wgpu::Buffer,
    uniforms_bind_group: wgpu::BindGroup,
    models_bind_group_layout: wgpu::BindGroupLayout,
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
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER_SOURCE)),
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

        Self {
            device,
            queue,
            pipeline,
            uniform_buffer,
            uniforms_bind_group,
            models_bind_group_layout,
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        self.msaa = Some(MsaaTargets {
            width,
            height,
            color_view,
            depth_view,
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

    pub fn render(
        &mut self,
        scene: &Scene,
        camera: &OrbitCamera,
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
        let entries = build_model_entries(scene);
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
                    stencil_ops: None,
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
            // uniform reads that object's matrix.
            for (i, obj) in installed.gpu.objects.iter().enumerate() {
                let offset = (i as u64 * installed.gpu.model_stride) as u32;
                pass.set_bind_group(1, &installed.models_bind_group, &[offset]);
                pass.set_vertex_buffer(0, obj.vertex_buffer.slice(..));
                pass.set_index_buffer(obj.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..obj.index_count, 0, 0..1);
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

fn transform_to_mat4(t: &Transform3D) -> Mat4 {
    match t {
        Transform3D::Identity => Mat4::IDENTITY,
        Transform3D::Affine(a) => affine_to_mat4(a),
    }
}

fn build_model_entries(scene: &Scene) -> Vec<ModelEntry> {
    scene
        .objects
        .iter()
        .map(|obj| ModelEntry {
            model: transform_to_mat4(&obj.transform).to_cols_array_2d(),
            flags: [0, 0, 0, 0],
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
        // The whole point of Phase A: vertex buffers are local-space.
        // The transform is applied at draw time via the model storage
        // buffer, so `build_gpu_scene_data` must NOT bake it into the
        // vertex stream.
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
        // Phase A doesn't reject singular transforms here — the GPU
        // path is fine with a singular model matrix (it just collapses
        // the mesh to a plane). Mirror this in the assertion so the
        // test fails loudly if the behaviour changes.
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
        let entries = build_model_entries(&scene);
        assert_eq!(entries.len(), 2);
        // First entry is identity.
        assert_approx_eq!(f32, entries[0].model[0][0], 1.0);
        assert_approx_eq!(f32, entries[0].model[3][0], 0.0);
        // Second entry has +7 in the translation slot (column 3, row 0).
        assert_approx_eq!(f32, entries[1].model[3][0], 7.0);
        assert_approx_eq!(f32, entries[1].model[3][1], 0.0);
        assert_approx_eq!(f32, entries[1].model[3][2], 0.0);
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
            .render(&scene, &camera, 64, 64)
            .expect("render should produce a texture");
        assert_eq!(texture.size().width, 64);
        assert_eq!(texture.size().height, 64);
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
            .render(&scene, &camera, 64, 64)
            .expect("render multi-object");
    }
}
