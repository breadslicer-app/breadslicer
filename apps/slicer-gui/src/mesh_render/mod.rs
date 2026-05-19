/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::num::NonZeroU64;

use anyhow::Result;
use glam::Vec3;
use sc_mesh_core::scene_graph::SceneGraph;
use sc_mesh_core::scene_graph_node::NodeId;
use slint::wgpu_28::wgpu;

use crate::build_volume::BuildVolume;
use crate::camera::{DEFAULT_DISTANCE_FACTOR, OrbitCamera};
use crate::gizmo::axis::ALL_AXES;

mod frame;
mod gizmo_buffers;
mod gizmo_entries;
mod pipelines;
mod scene_upload;
mod targets;

use frame::Uniforms;
use gizmo_buffers::GizmoBuffers;
use gizmo_entries::{
    GIZMO_SLOTS, GizmoUniformsGpu, build_rotate_entries, build_scale_axis_entries,
    build_translate_entries,
};
pub use gizmo_entries::{GizmoDragVisual, GizmoFrame, GizmoHover, GizmoVisibility};
use pipelines::gizmo::{GizmoBindGroup, GizmoPipeline};
use pipelines::mesh::{MeshBindGroup, MeshPipeline};
use pipelines::outline::{OutlineBindGroup, OutlinePipeline};
use pipelines::plate::{PlateBindGroup, PlateBuffers, PlatePipeline};
pub(crate) use scene_upload::GpuScene;
use scene_upload::{
    MODEL_ENTRY_SIZE, build_envelope_warning_mask, build_model_entries, graph_leaf_count,
};
use targets::{MsaaTargets, TargetPool};

/// Scene-dependent inputs to [`Renderer::render`]. The selection
/// drives the outline post-pass and the gizmo overlay drives the
/// gizmo pass; both are meaningful only when a scene is loaded, so
/// they live alongside the scene reference rather than on
/// [`Renderer::render`]'s top-level signature where the type system
/// would let a caller pass `selection: Some(_)` against
/// `scene: None`.
#[derive(Clone, Copy)]
pub struct SceneRender<'a> {
    pub graph: &'a SceneGraph,
    /// Stable [`NodeId`] whose stencil mask the outline post-pass
    /// dilates. `None` skips the outline pass; a `NodeId` that does
    /// not name a reachable leaf is treated the same as `None`.
    pub selection: Option<NodeId>,
    /// Gizmo overlay parameters; the gizmo pass runs only when
    /// this is `Some`.
    pub gizmo: Option<GizmoFrame>,
}

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
    mesh: MeshPipeline,
    outline: OutlinePipeline,
    gizmo: GizmoPipeline,
    gizmo_buffers: GizmoBuffers,
    plate: PlatePipeline,
    /// Cached plate geometry for the current `BuildVolume`. `None`
    /// until the first render call; rebuilt when the per-frame
    /// `BuildVolume` differs from the cached one.
    plate_buffers: Option<PlateBuffers>,
    scene: Option<InstalledScene>,
    msaa: Option<MsaaTargets>,
    targets: Option<TargetPool>,
}

/// Vertical field of view in radians.
///
/// Chosen so that a sphere of the scene's bounding radius fills
/// `FIT_FRACTION` of the viewport height when the camera sits at its initial
/// framing distance (`DEFAULT_DISTANCE_FACTOR * radius`). Being a pure
/// function of those constants, the FOV is viewport-independent, resizing
/// the window rescales the image but never distorts perspective.
pub fn default_fov_y() -> f32 {
    2.0 * (1.0 / (FIT_FRACTION * DEFAULT_DISTANCE_FACTOR)).atan()
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let mesh = MeshPipeline::create(&device);
        let outline = OutlinePipeline::create(&device);
        let gizmo_buffers = GizmoBuffers::create(&device);
        let gizmo = GizmoPipeline::create(&device, &gizmo_buffers);
        let plate = PlatePipeline::create(&device, &mesh);

        Self {
            device,
            queue,
            mesh,
            outline,
            gizmo,
            gizmo_buffers,
            plate,
            plate_buffers: None,
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
    pub fn upload_scene(&mut self, graph: &SceneGraph) -> Result<()> {
        let gpu = GpuScene::build(&self.device, graph)?;
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
            layout: &self.mesh.models_bind_group_layout,
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
        if self.msaa.as_ref().is_some_and(|m| m.matches(width, height)) {
            return;
        }
        self.msaa = Some(MsaaTargets::create(
            &self.device,
            &self.outline.bind_group_layout,
            width,
            height,
        ));
    }

    fn ensure_targets(&mut self, width: u32, height: u32) {
        if self
            .targets
            .as_ref()
            .is_some_and(|t| t.matches(width, height))
        {
            return;
        }
        self.targets = Some(TargetPool::create(&self.device, width, height));
    }

    /// Lazily create or rebuild the plate's GPU buffers. The cache
    /// key is the `BuildVolume`; a different value triggers a
    /// rebuild so a runtime envelope change reaches the next frame.
    fn ensure_plate_buffers(&mut self, build_volume: BuildVolume) {
        if self
            .plate_buffers
            .as_ref()
            .is_some_and(|b| b.build_volume == build_volume)
        {
            return;
        }
        self.plate_buffers = Some(PlateBuffers::create(&self.device, build_volume));
    }

    pub fn render(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Option<wgpu::Texture> {
        if width == 0 || height == 0 {
            return None;
        }

        let (graph, selection, gizmo) = match scene_render {
            Some(sr) => (Some(sr.graph), sr.selection, sr.gizmo),
            None => (None, None, None),
        };

        self.ensure_msaa(width, height);
        self.ensure_targets(width, height);
        self.ensure_plate_buffers(build_volume);
        let installed = self.scene.as_ref();
        let msaa = self.msaa.as_ref().unwrap();
        let plate_buffers = self
            .plate_buffers
            .as_ref()
            .expect("ensure_plate_buffers populated the cache");

        // Per-frame upload of model matrices when a `GpuScene` is
        // installed AND the caller passed the matching CPU
        // `SceneGraph`. The CPU graph is the source of truth; the GPU
        // uniform buffer is a mirror that lives long enough to be
        // sampled across the next draw. The renderer's `GpuScene`
        // must match the graph's reachable-leaf count 1:1; caller-side
        // invariant checked by debug_assert so test failures surface
        // early.
        let scene_for_draws = if let (Some(inst), Some(graph_data)) = (installed, graph) {
            debug_assert_eq!(
                inst.gpu.object_count(),
                graph_leaf_count(graph_data),
                "GpuScene and SceneGraph have diverged in leaf count"
            );
            let entries = build_model_entries(graph_data, selection, build_volume);
            // Each entry lives at `i * model_stride`. The stride is
            // at least `MODEL_ENTRY_SIZE` and a multiple of the
            // device's uniform offset alignment (typically 256
            // bytes), so consecutive entries are not contiguous
            // and must be written individually.
            for (i, entry) in entries.iter().enumerate() {
                let offset = i as u64 * inst.gpu.model_stride();
                self.queue
                    .write_buffer(inst.gpu.model_buffer(), offset, bytemuck::bytes_of(entry));
            }
            !entries.is_empty()
        } else {
            false
        };

        // Framing AABB drives `compute_view_proj`'s near/far
        // multipliers. With a scene installed, union the scene's
        // world AABB with the plate envelope so the plate stays
        // inside the depth range; without a scene, the envelope
        // alone is the framing AABB.
        let env_min = build_volume.envelope_min_mm();
        let env_max = build_volume.envelope_max_mm();
        let (framing_min, framing_max) = match installed {
            Some(inst) => {
                let (smin, smax) = inst.gpu.aabb();
                (smin.min(env_min), smax.max(env_max))
            }
            None => (env_min, env_max),
        };
        let view_proj = frame::compute_view_proj(
            camera,
            framing_min,
            framing_max,
            width,
            height,
            default_fov_y(),
        );

        // Per-frame envelope-wall warning mask: which side walls (and
        // the top) some leaf breaches. Recomputed each frame from
        // the CPU `SceneGraph`; cheap (a single per-vertex scan) and
        // keeps the breach signal in lockstep with the per-leaf
        // `flags.y` bit. `None` for an empty graph leaves the mask
        // at 0 so all wall fragments are discarded.
        let face_breach_mask = graph.map_or(0, |g| build_envelope_warning_mask(g, build_volume));
        let uniforms = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: frame::LIGHT_DIR,
            env_min: [env_min.x, env_min.y, env_min.z, 0.0],
            env_max: [env_max.x, env_max.y, env_max.z, 0.0],
            face_breach_mask: [face_breach_mask, 0, 0, 0],
        };
        self.queue
            .write_buffer(&self.mesh.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let target = self
            .targets
            .as_mut()
            .expect("ensure_targets populated the pool")
            .acquire();
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
                        load: wgpu::LoadOp::Clear(frame::CLEAR_COLOR),
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

            // Plate solid (sides + bottom): drawn first with
            // depth-write on so the slab's interior occludes
            // content behind it when the camera orbits low.
            // Stencil reference stays at 0 so the outline post-pass
            // leaves the plate untouched.
            pass.set_pipeline(&self.plate.solid_pipeline);
            pass.set_bind_group(
                PlateBindGroup::Uniforms.idx(),
                &self.plate.uniforms_bind_group,
                &[],
            );
            pass.set_vertex_buffer(0, plate_buffers.vertex_buffer.slice(..));
            pass.set_index_buffer(
                plate_buffers.solid_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.set_stencil_reference(0);
            pass.draw_indexed(0..plate_buffers.solid_index_count, 0, 0..1);

            // Plate top: inner grid + outer rounded border at Z = 0.
            // Depth-write off so a model resting at Z = 0 always
            // wins z-fighting against the plate top.
            pass.set_pipeline(&self.plate.top_pipeline);
            pass.set_index_buffer(
                plate_buffers.top_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..plate_buffers.top_index_count, 0, 0..1);

            // Cage: 12 wireframe edges sharing the plate vertex
            // buffer; the line-list index buffer picks the cage
            // corner range.
            pass.set_pipeline(&self.plate.line_pipeline);
            pass.set_index_buffer(
                plate_buffers.cage_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..plate_buffers.cage_index_count, 0, 0..1);

            if scene_for_draws && let Some(inst) = installed {
                pass.set_pipeline(&self.mesh.pipeline);
                pass.set_bind_group(
                    MeshBindGroup::Uniforms.idx(),
                    &self.mesh.uniforms_bind_group,
                    &[],
                );
                // One draw call per object. The models bind group
                // is rebound at each step with the dynamic offset
                // of the matching ModelEntry slot, so the vertex
                // shader's `model_entry` uniform reads that
                // object's matrix. The stencil reference is `1`
                // only for the selected object, so after this pass
                // the stencil holds 1 over its visible silhouette
                // and 0 everywhere else, what the outline
                // post-pass dilates.
                for (i, obj) in inst.gpu.objects().iter().enumerate() {
                    let offset = (i as u64 * inst.gpu.model_stride()) as u32;
                    pass.set_bind_group(
                        MeshBindGroup::Models.idx(),
                        &inst.models_bind_group,
                        &[offset],
                    );
                    pass.set_vertex_buffer(0, obj.vertex_buffer().slice(..));
                    pass.set_index_buffer(obj.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
                    pass.set_stencil_reference(if Some(obj.node_id()) == selection {
                        1
                    } else {
                        0
                    });
                    pass.draw_indexed(0..obj.index_count(), 0, 0..1);
                }
            }

            // Envelope-warning walls: 5 invisible quads coplanar with
            // the build-volume side faces and top. The plate shader
            // discards a wall fragment unless the matching bit of
            // `face_breach_mask` is set; on a set bit it emits the
            // diagonal red stripe pattern. Drawn after the mesh pass
            // so the mesh's depth-write gates wall visibility: stripes
            // appear over the part of the mesh that lies on the far
            // side of the envelope wall from the camera, while
            // closer-to-camera mesh pixels occlude the wall as they
            // should. `top_pipeline` matches the needs here
            // (TriangleList, depth-test Less, depth-write off,
            // cull none, alpha blending). Stencil stays at 0.
            pass.set_pipeline(&self.plate.top_pipeline);
            pass.set_bind_group(
                PlateBindGroup::Uniforms.idx(),
                &self.plate.uniforms_bind_group,
                &[],
            );
            pass.set_vertex_buffer(0, plate_buffers.vertex_buffer.slice(..));
            pass.set_index_buffer(
                plate_buffers.wall_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.set_stencil_reference(0);
            pass.draw_indexed(0..plate_buffers.wall_index_count, 0, 0..1);

            // Plate bottom: drawn last in the main pass with alpha
            // blending and back-face culling. Cage edges and any
            // meshes are already in the framebuffer, so the bottom
            // blends over them when the camera looks up from below;
            // from above the bottom is back-facing and culled by the
            // pipeline, leaving the top face fully opaque. The mesh
            // loop above left the plate vertex buffer unbound, so
            // rebind. Stencil reset to 0 so the outline post-pass
            // leaves the bottom untouched.
            pass.set_pipeline(&self.plate.bottom_pipeline);
            pass.set_bind_group(
                PlateBindGroup::Uniforms.idx(),
                &self.plate.uniforms_bind_group,
                &[],
            );
            pass.set_vertex_buffer(0, plate_buffers.vertex_buffer.slice(..));
            pass.set_index_buffer(
                plate_buffers.bottom_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.set_stencil_reference(0);
            pass.draw_indexed(0..plate_buffers.bottom_index_count, 0, 0..1);
        }
        // Outline post-pass: only run when something is selected
        // and at least one mesh draw wrote stencil = 1 (i.e. a
        // scene is installed). Without a scene, the stencil is all
        // zeroes and the dilate would be a no-op.
        if selection.is_some() && scene_for_draws {
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
            outline_pass.set_pipeline(&self.outline.pipeline);
            outline_pass.set_bind_group(
                OutlineBindGroup::Stencil.idx(),
                &msaa.outline_bind_group,
                &[],
            );
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
                &self.gizmo_buffers.uniforms_buffer,
                0,
                bytemuck::bytes_of(&uniforms),
            );

            // Decide which categories to draw and write the matching
            // entry slots. Other slots are not used this frame, so we
            // skip writing them, the dynamic-offset bind only ever
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
                GizmoVisibility::Idle | GizmoVisibility::DraggingScaleAxis { .. }
            );

            if render_translate {
                let entries = build_translate_entries(&frame);
                self.gizmo_buffers.write_entries(
                    &self.queue,
                    GIZMO_SLOTS.translate.start(),
                    &entries,
                );
            }
            if render_rotate {
                let entries = build_rotate_entries(&frame, camera);
                self.gizmo_buffers
                    .write_entries(&self.queue, GIZMO_SLOTS.rotate.start(), &entries);
            }
            if render_scale {
                let axis_entries = build_scale_axis_entries(&frame);
                self.gizmo_buffers.write_entries(
                    &self.queue,
                    GIZMO_SLOTS.scale_axis.start(),
                    &axis_entries,
                );
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
            gizmo_pass.set_bind_group(
                GizmoBindGroup::Uniforms.idx(),
                &self.gizmo.uniforms_bind_group,
                &[],
            );

            if render_translate {
                // 1-pixel shaft line, three axes.
                gizmo_pass.set_pipeline(&self.gizmo.line_pipeline);
                gizmo_pass
                    .set_vertex_buffer(0, self.gizmo_buffers.shaft_line_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_buffers.shaft_line_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                    let offset = (GIZMO_SLOTS.translate.slot(axis_idx as u64)
                        * self.gizmo_buffers.entry_stride) as u32;
                    gizmo_pass.set_bind_group(
                        GizmoBindGroup::Entries.idx(),
                        &self.gizmo.entries_bind_group,
                        &[offset],
                    );
                    gizmo_pass.draw_indexed(0..self.gizmo_buffers.shaft_line_index_count, 0, 0..1);
                }
                // Solid cone tip, the active translate handle.
                gizmo_pass.set_pipeline(&self.gizmo.triangle_pipeline);
                gizmo_pass
                    .set_vertex_buffer(0, self.gizmo_buffers.cone_tip_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_buffers.cone_tip_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                    let offset = (GIZMO_SLOTS.translate.slot(axis_idx as u64)
                        * self.gizmo_buffers.entry_stride) as u32;
                    gizmo_pass.set_bind_group(
                        GizmoBindGroup::Entries.idx(),
                        &self.gizmo.entries_bind_group,
                        &[offset],
                    );
                    gizmo_pass.draw_indexed(0..self.gizmo_buffers.cone_tip_index_count, 0, 0..1);
                }
            }
            if render_rotate {
                // 1-pixel arc polyline.
                gizmo_pass.set_pipeline(&self.gizmo.line_pipeline);
                gizmo_pass
                    .set_vertex_buffer(0, self.gizmo_buffers.arc_line_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_buffers.arc_line_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                    let offset = (GIZMO_SLOTS.rotate.slot(axis_idx as u64)
                        * self.gizmo_buffers.entry_stride) as u32;
                    gizmo_pass.set_bind_group(
                        GizmoBindGroup::Entries.idx(),
                        &self.gizmo.entries_bind_group,
                        &[offset],
                    );
                    gizmo_pass.draw_indexed(0..self.gizmo_buffers.arc_line_index_count, 0, 0..1);
                }
                // Two-cone spindle at the arc's midpoint, the active
                // rotate handle. Shares the per-axis camera-rolled
                // `axis_rotation` so it lands at the visual centre of
                // the arc.
                gizmo_pass.set_pipeline(&self.gizmo.triangle_pipeline);
                gizmo_pass
                    .set_vertex_buffer(0, self.gizmo_buffers.rotate_cones_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_buffers.rotate_cones_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                    let offset = (GIZMO_SLOTS.rotate.slot(axis_idx as u64)
                        * self.gizmo_buffers.entry_stride) as u32;
                    gizmo_pass.set_bind_group(
                        GizmoBindGroup::Entries.idx(),
                        &self.gizmo.entries_bind_group,
                        &[offset],
                    );
                    gizmo_pass.draw_indexed(
                        0..self.gizmo_buffers.rotate_cones_index_count,
                        0,
                        0..1,
                    );
                }
            }
            if render_scale {
                gizmo_pass.set_pipeline(&self.gizmo.triangle_pipeline);
                gizmo_pass
                    .set_vertex_buffer(0, self.gizmo_buffers.scale_cube_vertex_buffer.slice(..));
                gizmo_pass.set_index_buffer(
                    self.gizmo_buffers.scale_cube_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                    let offset = (GIZMO_SLOTS.scale_axis.slot(axis_idx as u64)
                        * self.gizmo_buffers.entry_stride) as u32;
                    gizmo_pass.set_bind_group(
                        GizmoBindGroup::Entries.idx(),
                        &self.gizmo.entries_bind_group,
                        &[offset],
                    );
                    gizmo_pass.draw_indexed(0..self.gizmo_buffers.scale_cube_index_count, 0, 0..1);
                }
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        Some(target)
    }
}

/// End-to-end GPU tests. They construct a fresh wgpu device with the
/// **default-limits profile** (the same profile Slint uses), so they
/// catch regressions that would only show up at runtime, most
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
    use crate::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME;
    use crate::gizmo::axis::Axis;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene_graph_node::LeafPayload;
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::transform::Transform3D;

    /// Loader-faithful empty graph: identity-Group root, no leaves yet.
    fn graph_with_group_root() -> (SceneGraph, NodeId) {
        let mut g = SceneGraph::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    /// Attaches an inline-mesh leaf under `parent` with the given
    /// local transform and returns the leaf's [`NodeId`].
    fn add_inline_leaf(
        g: &mut SceneGraph,
        parent: NodeId,
        mesh: IndexedMesh,
        transform: Transform3D,
    ) -> NodeId {
        let leaf = g
            .add_leaf(None, transform, LeafPayload::Mesh(mesh))
            .expect("add leaf");
        g.add_child(parent, leaf).expect("attach leaf");
        leaf
    }

    /// Build envelope used by the render tests below. Held as a
    /// module-level constant so each test passes the same value to
    /// `render` without extra boilerplate.
    const TEST_BV: BuildVolume = HARDCODED_DEFAULT_BUILD_VOLUME;

    /// Builds a wgpu (Device, Queue) with the default-limits profile
    /// Slint requests. Returns `None` if no adapter is available, the
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
            // panic in `create_bind_group_layout`, that's the
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

    /// First reachable leaf's [`NodeId`] in pre-order traversal. The
    /// renderer's selection key is the leaf's stable id, so the
    /// render tests resolve "the only leaf" through this helper
    /// instead of fabricating an id.
    fn first_leaf(graph: &SceneGraph) -> NodeId {
        use sc_mesh_core::scene_graph_node::NodeKind;
        let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let node = graph.node(id).expect("walked id should be live");
            match &node.kind {
                NodeKind::Leaf(_) => return id,
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                    for child in children.iter().rev() {
                        stack.push(*child);
                    }
                }
                NodeKind::Instance { .. } => {}
            }
        }
        panic!("test fixture has no leaves");
    }

    fn triangle_graph_with_translation(t: AffineTransform3D) -> SceneGraph {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh, Transform3D::Affine(t));
        g
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
    /// for a non-trivial graph with a non-identity transform.
    #[test]
    fn gpu_scene_build_succeeds_under_default_limits() {
        let Some((device, _queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
        let gpu = GpuScene::build(&device, &graph).expect("GpuScene::build should succeed");
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
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let texture = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: None,
                    gizmo: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
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
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let texture = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
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
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        // Twice, first frame creates the MSAA targets and outline
        // bind group; second exercises the cached path.
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render with selection runs the outline pass");
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
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
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
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
            hover: None,
            drag_visual: None,
        };
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: Some(frame),
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("first gizmo render should succeed");
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: Some(frame),
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("cached gizmo resources survive a second frame");
    }

    /// Idle visibility renders all three handle categories
    /// simultaneously, exercises the translate, rotate, and scale
    /// draws in a single frame.
    #[test]
    fn render_with_idle_gizmo_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
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
            hover: None,
            drag_visual: None,
        };
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: Some(frame),
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
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
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
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
            hover: None,
            drag_visual: None,
        };
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: Some(frame),
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("rotate gizmo render should succeed");
    }

    /// Scale-axis renders the three midpoint cubes, pins the
    /// scale-pass wiring under default-limits.
    #[test]
    fn render_with_scale_gizmo_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue);
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
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
            hover: None,
            drag_visual: None,
        };
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: Some(scale_axis),
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("scale-axis gizmo render should succeed");
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
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        };
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, mesh.clone(), Transform3D::Identity);
        add_inline_leaf(
            &mut graph,
            root,
            mesh.clone(),
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
        );
        add_inline_leaf(
            &mut graph,
            root,
            mesh,
            Transform3D::Affine(AffineTransform3D::from_translation([0.0, 10.0, 0.0])),
        );
        let mut renderer = Renderer::new(device.clone(), queue);
        renderer.upload_scene(&graph).expect("upload");
        let bounds = renderer.scene_bounds().expect("bounds");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);
        let _texture = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: None,
                    gizmo: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render multi-object");
    }

    /// Render with no scene installed: the plate alone should
    /// produce a non-`None` texture. Two consecutive calls so the
    /// second exercises the cached `plate_buffers` path.
    #[test]
    fn render_plate_only_under_default_limits() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, queue);
        let (centre, radius) = TEST_BV.framing(None);
        let camera = OrbitCamera::new(centre, radius);
        let _ = renderer
            .render(None, TEST_BV, &camera, 64, 64)
            .expect("first plate-only render should succeed");
        let _ = renderer
            .render(None, TEST_BV, &camera, 64, 64)
            .expect("second plate-only render should succeed");
    }

    /// A render call against a different `BuildVolume` rebuilds the
    /// cached plate buffers so the new envelope reaches the next
    /// frame. Verified through the public path: render once at the
    /// default envelope, then at a smaller one, and confirm both
    /// frames succeed and the cache key reflects the latest call.
    #[test]
    fn render_plate_buffers_rebuild_on_envelope_change() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, queue);
        let camera = OrbitCamera::new(Vec3::ZERO, 100.0);

        let _ = renderer
            .render(None, TEST_BV, &camera, 64, 64)
            .expect("render at default envelope");
        let cached_after_first = renderer.plate_buffers.as_ref().map(|b| b.build_volume);
        assert_eq!(cached_after_first, Some(TEST_BV));

        let smaller = BuildVolume::new(50.0, 50.0, 50.0);
        let _ = renderer
            .render(None, smaller, &camera, 64, 64)
            .expect("render at smaller envelope");
        let cached_after_second = renderer.plate_buffers.as_ref().map(|b| b.build_volume);
        assert_eq!(cached_after_second, Some(smaller));
    }

    /// Builds a one-triangle graph that pokes one vertex past the
    /// `TEST_BV`'s +X envelope wall at x = +96 mm. Two anchor
    /// vertices stay inside the envelope so the graph's AABB
    /// straddles the wall (and the camera framing centres on it).
    fn x_pos_breaching_graph() -> SceneGraph {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, -1.0, 50.0]),
                Vertex::new([0.0, 1.0, 50.0]),
                Vertex::new([120.0, 0.0, 50.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
        g
    }

    /// Counts pixels whose RGB signature matches the wall warning
    /// stripe. The shader emits `WALL_WARNING_COLOUR = (1.00, 0.15,
    /// 0.15)` with `WALL_STRIPE_ALPHA = 0.55`, so on a typical
    /// background the visible pixel is red-dominant with green and
    /// blue both well below red. The thresholds are loose enough to
    /// match the stripe over any plausible background (white plate,
    /// grey mesh, dark cage) but tight enough that no untinted
    /// surface in the scene clears the bar.
    fn count_warning_pixels(rgba: &[u8]) -> usize {
        rgba.chunks_exact(4)
            .filter(|p| {
                let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
                r > 128 && (r - g) > 40 && (r - b) > 40
            })
            .count()
    }

    /// Copies an `Rgba8Unorm` 2D texture back to the CPU and returns
    /// the pixel bytes in row-major RGBA order. Requires
    /// `wgpu::TextureUsages::COPY_SRC` on the texture (the renderer's
    /// resolved color target has it). `width * 4` must satisfy
    /// `COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes); the test sites
    /// pick widths that already do, so the function asserts rather
    /// than padding silently.
    fn read_target_back(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        const BYTES_PER_PIXEL: u32 = 4;
        let bytes_per_row = width * BYTES_PER_PIXEL;
        assert!(
            bytes_per_row.is_multiple_of(256),
            "test-only helper expects width * 4 to be a 256-byte multiple; got {bytes_per_row}"
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer test readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mesh-viewer test readback encoder"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result)
                .expect("readback map_async receiver dropped");
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll for readback");
        rx.recv()
            .expect("readback map_async sender dropped")
            .expect("readback map_async failed");
        let data = slice.get_mapped_range().to_vec();
        readback.unmap();
        data
    }

    /// End-to-end property test for the +X envelope warning stripe.
    /// Renders the same camera-frame twice, once with a mesh that
    /// stays inside the envelope and once with a mesh that pokes
    /// past +X, reads back both framebuffers, and asserts that the
    /// breach render carries visible stripe pixels (red-dominant,
    /// green / blue subordinate) and the safe render does not.
    /// Property-level: independent of the exact stripe geometry,
    /// driver-specific blending rounding, and viewport jitter.
    #[test]
    fn envelope_breach_x_pos_paints_warning_stripe_pixels() {
        let Some((device, queue)) = make_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), queue.clone());
        // 64 * 4 = 256 bytes / row, which already satisfies
        // `COPY_BYTES_PER_ROW_ALIGNMENT` and matches the size used by
        // the other render tests in this module.
        let width = 64u32;
        let height = 64u32;
        // Centre the orbit on the build-volume centre with a radius
        // that comfortably contains the envelope so the +X wall is
        // in frame regardless of which mesh is installed.
        let camera = OrbitCamera::new(Vec3::new(0.0, 0.0, 100.0), 250.0);

        let safe_graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 50.0]));
        renderer
            .upload_scene(&safe_graph)
            .expect("upload safe graph");
        let safe_texture = renderer
            .render(
                Some(SceneRender {
                    graph: &safe_graph,
                    selection: None,
                    gizmo: None,
                }),
                TEST_BV,
                &camera,
                width,
                height,
            )
            .expect("safe render produces a texture");
        let safe_pixels = read_target_back(&device, &queue, &safe_texture, width, height);
        let safe_warning = count_warning_pixels(&safe_pixels);

        let breach_graph = x_pos_breaching_graph();
        renderer
            .upload_scene(&breach_graph)
            .expect("upload breaching graph");
        let breach_texture = renderer
            .render(
                Some(SceneRender {
                    graph: &breach_graph,
                    selection: None,
                    gizmo: None,
                }),
                TEST_BV,
                &camera,
                width,
                height,
            )
            .expect("breach render produces a texture");
        let breach_pixels = read_target_back(&device, &queue, &breach_texture, width, height);
        let breach_warning = count_warning_pixels(&breach_pixels);

        assert!(
            safe_warning < 5,
            "safe render should be essentially free of warning-colour pixels, got {safe_warning}"
        );
        assert!(
            breach_warning >= 20,
            "breach render should paint visible stripe pixels, got {breach_warning} (safe={safe_warning})"
        );
        assert!(
            breach_warning > safe_warning * 4,
            "breach must dominate safe in warning-colour pixel count: breach={breach_warning}, safe={safe_warning}"
        );
    }
}
