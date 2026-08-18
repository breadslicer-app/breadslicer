/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use glam::{Mat4, Vec3};
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::node::NodeId;
use slint::wgpu_29::wgpu;

use crate::build_volume::BuildVolume;
use crate::camera::{DEFAULT_DISTANCE_FACTOR, OrbitCamera};
use crate::gizmo::axis::ALL_AXES;
use crate::slice_cap::CapRaster;

mod device_ops;
mod error;
mod frame;
mod gizmo_buffers;
mod gizmo_entries;
#[cfg(test)]
pub(crate) mod mock;
mod pipelines;
mod render_size;
mod renderer_trait;
pub mod scene_upload;
mod targets;

use device_ops::scoped;
pub use error::{RenderAllocation, RenderError, RendererInitError, RendererStage};
use frame::Uniforms;
use gizmo_buffers::GizmoBuffers;
use gizmo_entries::{
    GIZMO_SLOTS, GizmoUniformsGpu, build_rotate_entries, build_scale_axis_entries,
    build_translate_entries,
};
pub use gizmo_entries::{GizmoDragVisual, GizmoFrame, GizmoHover, GizmoVisibility};
use pipelines::cap::CapPipeline;
use pipelines::gizmo::{GizmoBindGroup, GizmoPipeline};
use pipelines::mesh::{MeshBindGroup, MeshPipeline};
use pipelines::outline::{OutlineBindGroup, OutlinePipeline};
use pipelines::plate::{PlateBindGroup, PlateBuffers, PlatePipeline};
use render_size::clamp_render_size;
pub(crate) use renderer_trait::MeshRenderer;
pub(crate) use scene_upload::GpuScene;
use scene_upload::{
    MODEL_ENTRY_SIZE, build_envelope_warning_mask, build_model_entries, renderable_leaf_count,
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
    pub graph: &'a Scene,
    /// Stable [`NodeId`] whose stencil mask the outline post-pass
    /// dilates. `None` skips the outline pass; a `NodeId` that does
    /// not name a reachable leaf is treated the same as `None`.
    pub selection: Option<NodeId>,
    /// Gizmo overlay parameters; the gizmo pass runs only when
    /// this is `Some`.
    pub gizmo: Option<GizmoFrame>,
    /// World-space Z (mm) above which mesh fragments are discarded, so
    /// the slice slider can reveal the model layer by layer. `None`
    /// renders the whole mesh.
    pub clip_z: Option<f32>,
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
// 4x MSAA: each pixel rasterises 4 samples and resolves to one colour,
// costing ~4x colour/depth/stencil attachment bandwidth in exchange for
// smooth mesh silhouettes and grid lines. Default-limits supported on
// every wgpu backend; 8x adds little visible benefit on this geometry.
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

/// Identity token used to detect cross-device installation of a
/// `GpuScene`. `wgpu::Device`'s own `PartialEq` proxies to the inner
/// state's value-equality, which compares two separately-created
/// devices as equal even when they are distinct GPU resources; the
/// install-time guard cannot use it. Every fresh device pairing must
/// mint a new token via [`DeviceToken::next`]; `GpuScene::build` and
/// `Renderer::new` store the same token, and
/// `Renderer::install_scene` rejects any `GpuScene` whose token does
/// not match the renderer's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceToken(u64);

impl DeviceToken {
    /// Mints a token that is process-globally unique for the lifetime
    /// of the process. Wrap raises on overflow at `u64::MAX` mints.
    pub fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Logs to stderr every uncaught wgpu error on `device`.
/// Without this, wgpu's default handler panics on them.
pub fn install_device_error_logger(device: &wgpu::Device) {
    device.on_uncaptured_error(std::sync::Arc::new(|err: wgpu::Error| {
        eprintln!("wgpu error (not fatal): {err}");
    }));
}

/// GPU device pairing plus the identity token used to detect a
/// cross-device install. Grouped so the [`Renderer`] struct does not
/// expose three loose fields whose lifetime is tied together (the
/// device, the queue, and the token are minted together at
/// `RenderingSetup` and dropped together at `RenderingTeardown`).
struct RendererCore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    device_token: DeviceToken,
    /// The device's `max_texture_dimension_2d`.
    max_texture_side: u32,
}

/// Per-pipeline GPU state. The mesh / outline / gizmo / plate
/// pipelines are immutable after construction; `gizmo_buffers` holds
/// the gizmo pass's vertex/index data, also immutable, while
/// `plate_buffers` is cached lazily because the plate geometry
/// depends on the runtime `BuildVolume`.
struct Pipelines {
    mesh: MeshPipeline,
    outline: OutlinePipeline,
    gizmo: GizmoPipeline,
    gizmo_buffers: GizmoBuffers,
    plate: PlatePipeline,
    /// The slice-view cap pipeline, immutable after construction.
    cap: CapPipeline,
    cap_state: CapState,
    /// Cached plate geometry for the current `BuildVolume`. `None`
    /// until the first render call; rebuilt when the per-frame
    /// `BuildVolume` differs from the cached one.
    plate_buffers: Option<PlateBuffers>,
}

/// State of the slice-view cap texture, keyed by the layer it was last
/// built for. The bind group keeps the texture and its view alive.
enum CapState {
    Off,
    Shown {
        key: u64,
        bind_group: wgpu::BindGroup,
    },
    Refused {
        key: u64,
    },
}

impl CapState {
    fn key(&self) -> Option<u64> {
        match self {
            Self::Off => None,
            Self::Shown { key, .. } | Self::Refused { key } => Some(*key),
        }
    }
}

/// Viewport-sized GPU allocations that are dropped and recreated when
/// the render-target size changes. Both fields are `None` until the
/// first render call and after [`Renderer::invalidate_sizing`] /
/// [`Renderer::clear_scene`].
struct TargetManager {
    msaa: Option<MsaaTargets>,
    targets: Option<TargetPool>,
}

pub struct Renderer {
    core: RendererCore,
    pipelines: Pipelines,
    scene: Option<InstalledScene>,
    target_manager: TargetManager,
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
    /// Builds every static GPU resource of the viewport against
    /// `device`.
    pub fn new(
        device: wgpu::Device,
        device_token: DeviceToken,
        queue: wgpu::Queue,
    ) -> Result<Self, RendererInitError> {
        let at = |stage: RendererStage| move |source| RendererInitError { stage, source };
        let mesh = MeshPipeline::create(&device).map_err(at(RendererStage::Mesh))?;
        let outline = OutlinePipeline::create(&device).map_err(at(RendererStage::Outline))?;
        let gizmo_buffers =
            GizmoBuffers::create(&device, &queue).map_err(at(RendererStage::GizmoBuffers))?;
        let gizmo =
            GizmoPipeline::create(&device, &gizmo_buffers).map_err(at(RendererStage::Gizmo))?;
        let plate = PlatePipeline::create(&device, &mesh).map_err(at(RendererStage::Plate))?;
        let cap = CapPipeline::create(&device, &mesh).map_err(at(RendererStage::Cap))?;
        let max_texture_side = device.limits().max_texture_dimension_2d;

        Ok(Self {
            core: RendererCore {
                device,
                queue,
                device_token,
                max_texture_side,
            },
            pipelines: Pipelines {
                mesh,
                outline,
                gizmo,
                gizmo_buffers,
                plate,
                cap,
                cap_state: CapState::Off,
                plate_buffers: None,
            },
            scene: None,
            target_manager: TargetManager {
                msaa: None,
                targets: None,
            },
        })
    }

    /// Identity token of the device pairing this renderer was built
    /// against. Callers thread this token through to worker-side
    /// `GpuScene::build` so the eventual `install_scene` can match
    /// the worker's snapshot against the renderer's current device.
    pub fn device_token(&self) -> DeviceToken {
        self.core.device_token
    }

    pub fn clear_scene(&mut self) {
        self.scene = None;
        self.target_manager.msaa = None;
        self.target_manager.targets = None;
    }

    /// Drops the viewport-sized allocations (MSAA + target pool) so the
    /// next `render` reallocates them at the current `(width, height)`.
    /// Used by `ViewportController::on_resize_settled` to snap back from
    /// bucketed drag-resize sizing to exact pixel sizing once the user
    /// stops resizing.
    pub fn invalidate_sizing(&mut self) {
        self.target_manager.msaa = None;
        self.target_manager.targets = None;
    }

    /// World-space AABB of the currently uploaded scene, captured at
    /// install time via `Scene::aabb`. `None` if nothing has been
    /// uploaded yet.
    pub fn scene_bounds(&self) -> Option<(Vec3, Vec3)> {
        self.scene.as_ref().map(|s| s.gpu.aabb())
    }

    /// Replaces the renderer's scene with one built from `graph`,
    /// blocking the caller for the whole build. Afterwards the renderer
    /// holds the new scene on `Ok`, and no scene at all on `Err`. When
    /// `graph` has no renderable leaves ([`render`] then draws the plate
    /// alone).
    ///
    /// [`render`]: Self::render
    pub fn upload_scene(&mut self, graph: &Scene) -> Result<()> {
        // Released before the build so the old and the new scene never
        // occupy device memory at the same time.
        self.scene = None;
        // An empty model uniform buffer would fail bind-group
        // validation, so a leafless graph gets no scene at all.
        if renderable_leaf_count(graph)? == 0 {
            self.clear_scene();
            return Ok(());
        }
        let gpu = GpuScene::build(&self.core.device, self.core.device_token, graph)?;
        self.attach_scene(gpu);
        Ok(())
    }

    /// Installs a `GpuScene` built on a worker thread, but only if its
    /// `device_token` matches this renderer's. Returns `false` if the
    /// device has rotated since the worker started (mid-load device
    /// loss); the caller falls back to the recovery upload path.
    pub fn install_scene(&mut self, gpu: GpuScene) -> bool {
        if self.core.device_token != gpu.device_token() {
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
        let models_bind_group = self
            .core
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mesh-viewer models bg"),
                layout: &self.pipelines.mesh.models_bind_group_layout,
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

    fn ensure_msaa(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        if self
            .target_manager
            .msaa
            .as_ref()
            .is_some_and(|m| m.matches(width, height))
        {
            return Ok(());
        }
        // Freed before the new allocation, not after, so a refusal is
        // not caused by the bundle it replaces.
        self.target_manager.msaa = None;
        self.target_manager.msaa = Some(MsaaTargets::create(
            &self.core.device,
            &self.pipelines.outline.bind_group_layout,
            width,
            height,
        )?);
        Ok(())
    }

    fn ensure_targets(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        if self
            .target_manager
            .targets
            .as_ref()
            .is_some_and(|t| t.matches(width, height))
        {
            return Ok(());
        }
        self.target_manager.targets = None;
        self.target_manager.targets = Some(TargetPool::create(&self.core.device, width, height)?);
        Ok(())
    }

    /// Lazily create or rebuild the plate's GPU buffers. The cache
    /// key is the `BuildVolume`; a different value triggers a
    /// rebuild so a runtime envelope change reaches the next frame.
    /// On `Err` the cache is empty, so the next frame retries. The
    /// returned value clones the cached wgpu handles (reference
    /// counted), so no borrow of `self` outlives the call.
    fn ensure_plate_buffers(
        &mut self,
        build_volume: BuildVolume,
    ) -> Result<PlateBuffers, RenderError> {
        let buffers = match self.pipelines.plate_buffers.take() {
            Some(cached) if cached.build_volume == build_volume => cached,
            _ => PlateBuffers::create(&self.core.device, &self.core.queue, build_volume)?,
        };
        Ok(self.pipelines.plate_buffers.insert(buffers).clone())
    }

    /// Refresh the slice-view cap texture. `key` identifies the layer
    /// being capped; `None` clears the cap. When `key` matches the
    /// current cap (shown or refused) nothing happens, so a camera
    /// orbit (same layer) skips the rasterise. Otherwise `build`
    /// produces the coverage raster, which is uploaded as an R8
    /// texture; a `build` yielding `None` clears the cap.
    pub(crate) fn update_cap(
        &mut self,
        key: Option<u64>,
        build: &mut dyn FnMut() -> Option<CapRaster>,
    ) -> Result<(), RenderError> {
        let Some(key) = key else {
            self.pipelines.cap_state = CapState::Off;
            return Ok(());
        };
        if self.pipelines.cap_state.key() == Some(key) {
            return Ok(());
        }
        let Some(raster) = build() else {
            self.pipelines.cap_state = CapState::Off;
            return Ok(());
        };
        match self.upload_cap_texture(&raster) {
            Ok(bind_group) => {
                self.pipelines.cap_state = CapState::Shown { key, bind_group };
                Ok(())
            }
            Err(err) => {
                self.pipelines.cap_state = CapState::Refused { key };
                Err(err)
            }
        }
    }

    /// Upload a coverage raster as an R8 texture and build the cap's
    /// group-1 bind group. Each row is padded to wgpu's copy alignment,
    /// as `write_texture` requires.
    fn upload_cap_texture(&self, raster: &CapRaster) -> Result<wgpu::BindGroup, RenderError> {
        scoped(&self.core.device, || {
            self.upload_cap_texture_unscoped(raster)
        })
        .map_err(|source| RenderError::DeviceRefused {
            what: RenderAllocation::CapTexture,
            source,
        })
    }

    fn upload_cap_texture_unscoped(&self, raster: &CapRaster) -> wgpu::BindGroup {
        let width = raster.width;
        let height = raster.height;
        let texture = self.core.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mesh-viewer cap texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let padded_bpr =
            width.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let mut padded = vec![0u8; (padded_bpr * height) as usize];
        for row in 0..height as usize {
            let src = row * width as usize;
            let dst = row * padded_bpr as usize;
            padded[dst..dst + width as usize]
                .copy_from_slice(&raster.coverage[src..src + width as usize]);
        }
        self.core.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &padded,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bpr),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.core
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mesh-viewer cap texture bg"),
                layout: &self.pipelines.cap.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.pipelines.cap.sampler),
                    },
                ],
            })
    }

    /// Draws one frame into a texture Slint can show. Returns `Ok(None)`
    /// for a zero-sized viewport. A size over the device's texture
    /// ceiling is drawn at the ceiling with the aspect kept, so the
    /// texture can come back smaller than asked.
    pub fn render(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<wgpu::Texture>, RenderError> {
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let (width, height) = clamp_render_size(width, height, self.core.max_texture_side);

        let (graph, selection, gizmo, clip_z) = match scene_render {
            Some(sr) => (Some(sr.graph), sr.selection, sr.gizmo, sr.clip_z),
            None => (None, None, None, None),
        };

        self.ensure_msaa(width, height)?;
        self.ensure_targets(width, height)?;
        let plate_buffers = self.ensure_plate_buffers(build_volume)?;
        let plate_buffers = &plate_buffers;
        let installed = self.scene.as_ref();
        let msaa = self.target_manager.msaa.as_ref().unwrap();

        // Per-frame upload of model matrices when a `GpuScene` is
        // installed AND the caller passed the matching CPU
        // `Scene`. The CPU graph is the source of truth; the GPU
        // uniform buffer is a mirror that lives long enough to be
        // sampled across the next draw. The renderer's `GpuScene`
        // must match the graph's reachable-leaf count 1:1; a
        // mismatch means the live graph has drifted from the
        // install-time snapshot, and writing one entry per live leaf
        // would step past the model buffer's end. Skip the mesh
        // path for this frame and wait for the next install to
        // restore the invariant. A graph that fails the leaf walk
        // (dangling node id / dangling subtree resource id) skips
        // the mesh path on the same grounds: the partial entry list
        // would leave the model buffer's tail slots holding stale
        // matrices that the draw loop would render against the
        // current frame's geometry.
        let scene_for_draws = if let (Some(inst), Some(graph_data)) = (installed, graph) {
            match build_model_entries(graph_data, selection, build_volume) {
                Ok(entries) if entries.len() == inst.gpu.object_count() => {
                    // Each entry lives at `i * model_stride`. The stride is
                    // at least `MODEL_ENTRY_SIZE` and a multiple of the
                    // device's uniform offset alignment (typically 256
                    // bytes), so consecutive entries are not contiguous
                    // and must be written individually.
                    for (i, entry) in entries.iter().enumerate() {
                        let offset = i as u64 * inst.gpu.model_stride();
                        self.core.queue.write_buffer(
                            inst.gpu.model_buffer(),
                            offset,
                            bytemuck::bytes_of(entry),
                        );
                    }
                    !entries.is_empty()
                }
                _ => false,
            }
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
        // the CPU `Scene`; cheap (a single per-vertex scan) and
        // keeps the breach signal in lockstep with the per-leaf
        // `flags.y` bit. `None` for an empty graph leaves the mask
        // at 0 so all wall fragments are discarded. A leaf-walk
        // failure on the same graph already caused `scene_for_draws`
        // to fall to false above, so falling back to 0 here keeps
        // the wall-warning state consistent with the skipped mesh
        // draws.
        let face_breach_mask = graph
            .and_then(|g| build_envelope_warning_mask(g, build_volume).ok())
            .unwrap_or(0);
        let uniforms = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: frame::pack_light_dir(clip_z),
            env_min: [env_min.x, env_min.y, env_min.z, 0.0],
            env_max: [env_max.x, env_max.y, env_max.z, 0.0],
            face_breach_mask: [face_breach_mask, 0, 0, 0],
        };
        self.core.queue.write_buffer(
            &self.pipelines.mesh.uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        let target = self
            .target_manager
            .targets
            .as_mut()
            .expect("ensure_targets populated the pool")
            .acquire();
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.core
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

            self.record_plate_solid(&mut pass, plate_buffers);
            self.record_plate_top(&mut pass, plate_buffers);
            self.record_cage(&mut pass, plate_buffers);

            if scene_for_draws && let Some(inst) = installed {
                self.record_mesh_objects(&mut pass, inst, selection);
            }

            self.record_cap(&mut pass);

            self.record_envelope_walls(&mut pass, plate_buffers);
            self.record_plate_bottom(&mut pass, plate_buffers);
        }
        if selection.is_some() && scene_for_draws {
            self.record_outline_post_pass(&mut encoder, &target_view, msaa);
        }
        if let Some(frame) = gizmo {
            self.record_gizmo_pass(&mut encoder, &target_view, msaa, &frame, view_proj, camera);
        }
        self.core.queue.submit(std::iter::once(encoder.finish()));

        Ok(Some(target))
    }

    /// Plate solid (sides + bottom): drawn first with depth-write on
    /// so the slab's interior occludes content behind it when the
    /// camera orbits low. Stencil reference stays at 0 so the outline
    /// post-pass leaves the plate untouched.
    fn record_plate_solid(&self, pass: &mut wgpu::RenderPass<'_>, plate_buffers: &PlateBuffers) {
        pass.set_pipeline(&self.pipelines.plate.solid_pipeline);
        pass.set_bind_group(
            PlateBindGroup::Uniforms.idx(),
            &self.pipelines.plate.uniforms_bind_group,
            &[],
        );
        pass.set_vertex_buffer(0, plate_buffers.vertex_buffer.slice(..));
        pass.set_index_buffer(
            plate_buffers.solid_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.set_stencil_reference(0);
        pass.draw_indexed(0..plate_buffers.solid_index_count, 0, 0..1);
    }

    /// Slice-view cap: a flat quad at the clip height textured with the
    /// current layer's coverage, filling the mesh cut so it reads solid.
    /// Drawn only when a cap texture is set (Slice section with a held
    /// slice). The clip height and build corners come from the shared
    /// uniform buffer, so no per-cap vertex data is bound.
    fn record_cap(&self, pass: &mut wgpu::RenderPass<'_>) {
        let CapState::Shown { bind_group, .. } = &self.pipelines.cap_state else {
            return;
        };
        pass.set_pipeline(&self.pipelines.cap.pipeline);
        pass.set_bind_group(0, &self.pipelines.cap.uniforms_bind_group, &[]);
        pass.set_bind_group(1, bind_group, &[]);
        pass.set_stencil_reference(0);
        pass.draw(0..6, 0..1);
    }

    /// Plate top: inner grid + outer rounded border at Z = 0.
    /// Depth-write off so a model resting at Z = 0 always wins
    /// z-fighting against the plate top. Relies on the vertex buffer
    /// and uniforms bind group set by [`Self::record_plate_solid`];
    /// must be called after it within the same render pass.
    fn record_plate_top(&self, pass: &mut wgpu::RenderPass<'_>, plate_buffers: &PlateBuffers) {
        pass.set_pipeline(&self.pipelines.plate.top_pipeline);
        pass.set_index_buffer(
            plate_buffers.top_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..plate_buffers.top_index_count, 0, 0..1);
    }

    /// Cage: 12 wireframe edges sharing the plate vertex buffer;
    /// the line-list index buffer picks the cage corner range.
    /// Relies on the vertex buffer and uniforms bind group set by
    /// [`Self::record_plate_solid`].
    fn record_cage(&self, pass: &mut wgpu::RenderPass<'_>, plate_buffers: &PlateBuffers) {
        pass.set_pipeline(&self.pipelines.plate.line_pipeline);
        pass.set_index_buffer(
            plate_buffers.cage_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..plate_buffers.cage_index_count, 0, 0..1);
    }

    /// One draw call per installed scene object. The models bind
    /// group is rebound at each step with the dynamic offset of the
    /// matching ModelEntry slot, so the vertex shader's `model_entry`
    /// uniform reads that object's matrix. The stencil reference is
    /// `1` only for the selected object, so after this pass the
    /// stencil holds 1 over its visible silhouette and 0 everywhere
    /// else, what the outline post-pass dilates.
    fn record_mesh_objects(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        inst: &InstalledScene,
        selection: Option<NodeId>,
    ) {
        pass.set_pipeline(&self.pipelines.mesh.pipeline);
        pass.set_bind_group(
            MeshBindGroup::Uniforms.idx(),
            &self.pipelines.mesh.uniforms_bind_group,
            &[],
        );
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

    /// Envelope-warning walls: 5 invisible quads coplanar with the
    /// build-volume side faces and top. The plate shader discards a
    /// wall fragment unless the matching bit of `face_breach_mask` is
    /// set; on a set bit it emits the diagonal red stripe pattern.
    /// Drawn after the mesh pass so the mesh's depth-write gates wall
    /// visibility: stripes appear over the part of the mesh that lies
    /// on the far side of the envelope wall from the camera, while
    /// closer-to-camera mesh pixels occlude the wall as they should.
    /// `top_pipeline` matches the needs here (TriangleList, depth-test
    /// Less, depth-write off, cull none, alpha blending). Stencil
    /// stays at 0.
    fn record_envelope_walls(&self, pass: &mut wgpu::RenderPass<'_>, plate_buffers: &PlateBuffers) {
        pass.set_pipeline(&self.pipelines.plate.top_pipeline);
        pass.set_bind_group(
            PlateBindGroup::Uniforms.idx(),
            &self.pipelines.plate.uniforms_bind_group,
            &[],
        );
        pass.set_vertex_buffer(0, plate_buffers.vertex_buffer.slice(..));
        pass.set_index_buffer(
            plate_buffers.wall_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.set_stencil_reference(0);
        pass.draw_indexed(0..plate_buffers.wall_index_count, 0, 0..1);
    }

    /// Plate bottom: drawn last in the main pass with alpha blending
    /// and back-face culling. Cage edges and any meshes are already
    /// in the framebuffer, so the bottom blends over them when the
    /// camera looks up from below; from above the bottom is
    /// back-facing and culled by the pipeline, leaving the top face
    /// fully opaque. The mesh loop earlier in the pass left the plate
    /// vertex buffer unbound, so rebind here. Stencil reset to 0 so
    /// the outline post-pass leaves the bottom untouched.
    fn record_plate_bottom(&self, pass: &mut wgpu::RenderPass<'_>, plate_buffers: &PlateBuffers) {
        pass.set_pipeline(&self.pipelines.plate.bottom_pipeline);
        pass.set_bind_group(
            PlateBindGroup::Uniforms.idx(),
            &self.pipelines.plate.uniforms_bind_group,
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

    /// Outline post-pass: dilate the stencil set by the mesh pass to
    /// draw a one-pixel halo around the selected leaf. Caller must
    /// gate on `selection.is_some() && scene_for_draws`: without a
    /// scene the stencil is all zeroes and the dilate would be a
    /// no-op; without a selection the mesh pass left the stencil at
    /// 0 too. Loads the resolved colour from the main pass and
    /// discards non-edge fragments, so existing pixels survive.
    fn record_outline_post_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        msaa: &MsaaTargets,
    ) {
        let mut outline_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh-viewer outline pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                ops: wgpu::Operations {
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
        outline_pass.set_pipeline(&self.pipelines.outline.pipeline);
        outline_pass.set_bind_group(
            OutlineBindGroup::Stencil.idx(),
            &msaa.outline_bind_group,
            &[],
        );
        outline_pass.draw(0..3, 0..1);
    }

    /// Gizmo pass: drawn last so it sits on top of the mesh + the
    /// outline. Depth is cleared at pass start so the gizmo is never
    /// occluded by the mesh; depth-test stays enabled within the pass
    /// so the three axes order correctly where they overlap near the
    /// pivot. Uploads the per-frame gizmo uniforms and the active
    /// entry slots, then records draw calls for whichever of
    /// translate / rotate / scale handles are visible this frame.
    fn record_gizmo_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        msaa: &MsaaTargets,
        frame: &GizmoFrame,
        view_proj: Mat4,
        camera: &OrbitCamera,
    ) {
        let uniforms = GizmoUniformsGpu {
            view_proj: view_proj.to_cols_array_2d(),
            pivot_and_scale: [frame.pivot.x, frame.pivot.y, frame.pivot.z, frame.scale],
        };
        self.core.queue.write_buffer(
            &self.pipelines.gizmo_buffers.uniforms_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        // Decide which categories to draw and write the matching
        // entry slots. Other slots are not used this frame, so we
        // skip writing them, the dynamic-offset bind only ever points
        // at slots that received fresh data.
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
            let entries = build_translate_entries(frame);
            self.pipelines.gizmo_buffers.write_entries(
                &self.core.queue,
                GIZMO_SLOTS.translate.start(),
                &entries,
            );
        }
        if render_rotate {
            let entries = build_rotate_entries(frame, camera);
            self.pipelines.gizmo_buffers.write_entries(
                &self.core.queue,
                GIZMO_SLOTS.rotate.start(),
                &entries,
            );
        }
        if render_scale {
            let axis_entries = build_scale_axis_entries(frame);
            self.pipelines.gizmo_buffers.write_entries(
                &self.core.queue,
                GIZMO_SLOTS.scale_axis.start(),
                &axis_entries,
            );
        }

        let mut gizmo_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh-viewer gizmo pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
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
            &self.pipelines.gizmo.uniforms_bind_group,
            &[],
        );

        if render_translate {
            // 1-pixel shaft line, three axes.
            gizmo_pass.set_pipeline(&self.pipelines.gizmo.line_pipeline);
            gizmo_pass.set_vertex_buffer(
                0,
                self.pipelines
                    .gizmo_buffers
                    .shaft_line_vertex_buffer
                    .slice(..),
            );
            gizmo_pass.set_index_buffer(
                self.pipelines
                    .gizmo_buffers
                    .shaft_line_index_buffer
                    .slice(..),
                wgpu::IndexFormat::Uint32,
            );
            for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                let offset = (GIZMO_SLOTS.translate.slot(axis_idx as u64)
                    * self.pipelines.gizmo_buffers.entry_stride)
                    as u32;
                gizmo_pass.set_bind_group(
                    GizmoBindGroup::Entries.idx(),
                    &self.pipelines.gizmo.entries_bind_group,
                    &[offset],
                );
                gizmo_pass.draw_indexed(
                    0..self.pipelines.gizmo_buffers.shaft_line_index_count,
                    0,
                    0..1,
                );
            }
            // Solid cone tip, the active translate handle.
            gizmo_pass.set_pipeline(&self.pipelines.gizmo.triangle_pipeline);
            gizmo_pass.set_vertex_buffer(
                0,
                self.pipelines
                    .gizmo_buffers
                    .cone_tip_vertex_buffer
                    .slice(..),
            );
            gizmo_pass.set_index_buffer(
                self.pipelines.gizmo_buffers.cone_tip_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                let offset = (GIZMO_SLOTS.translate.slot(axis_idx as u64)
                    * self.pipelines.gizmo_buffers.entry_stride)
                    as u32;
                gizmo_pass.set_bind_group(
                    GizmoBindGroup::Entries.idx(),
                    &self.pipelines.gizmo.entries_bind_group,
                    &[offset],
                );
                gizmo_pass.draw_indexed(
                    0..self.pipelines.gizmo_buffers.cone_tip_index_count,
                    0,
                    0..1,
                );
            }
        }
        if render_rotate {
            // 1-pixel arc polyline.
            gizmo_pass.set_pipeline(&self.pipelines.gizmo.line_pipeline);
            gizmo_pass.set_vertex_buffer(
                0,
                self.pipelines
                    .gizmo_buffers
                    .arc_line_vertex_buffer
                    .slice(..),
            );
            gizmo_pass.set_index_buffer(
                self.pipelines.gizmo_buffers.arc_line_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                let offset = (GIZMO_SLOTS.rotate.slot(axis_idx as u64)
                    * self.pipelines.gizmo_buffers.entry_stride)
                    as u32;
                gizmo_pass.set_bind_group(
                    GizmoBindGroup::Entries.idx(),
                    &self.pipelines.gizmo.entries_bind_group,
                    &[offset],
                );
                gizmo_pass.draw_indexed(
                    0..self.pipelines.gizmo_buffers.arc_line_index_count,
                    0,
                    0..1,
                );
            }
            // Two-cone spindle at the arc's midpoint, the active
            // rotate handle. Shares the per-axis camera-rolled
            // `axis_rotation` so it lands at the visual centre of the
            // arc.
            gizmo_pass.set_pipeline(&self.pipelines.gizmo.triangle_pipeline);
            gizmo_pass.set_vertex_buffer(
                0,
                self.pipelines
                    .gizmo_buffers
                    .rotate_cones_vertex_buffer
                    .slice(..),
            );
            gizmo_pass.set_index_buffer(
                self.pipelines
                    .gizmo_buffers
                    .rotate_cones_index_buffer
                    .slice(..),
                wgpu::IndexFormat::Uint32,
            );
            for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                let offset = (GIZMO_SLOTS.rotate.slot(axis_idx as u64)
                    * self.pipelines.gizmo_buffers.entry_stride)
                    as u32;
                gizmo_pass.set_bind_group(
                    GizmoBindGroup::Entries.idx(),
                    &self.pipelines.gizmo.entries_bind_group,
                    &[offset],
                );
                gizmo_pass.draw_indexed(
                    0..self.pipelines.gizmo_buffers.rotate_cones_index_count,
                    0,
                    0..1,
                );
            }
        }
        if render_scale {
            gizmo_pass.set_pipeline(&self.pipelines.gizmo.triangle_pipeline);
            gizmo_pass.set_vertex_buffer(
                0,
                self.pipelines
                    .gizmo_buffers
                    .scale_cube_vertex_buffer
                    .slice(..),
            );
            gizmo_pass.set_index_buffer(
                self.pipelines
                    .gizmo_buffers
                    .scale_cube_index_buffer
                    .slice(..),
                wgpu::IndexFormat::Uint32,
            );
            for (axis_idx, _axis) in ALL_AXES.iter().enumerate() {
                let offset = (GIZMO_SLOTS.scale_axis.slot(axis_idx as u64)
                    * self.pipelines.gizmo_buffers.entry_stride)
                    as u32;
                gizmo_pass.set_bind_group(
                    GizmoBindGroup::Entries.idx(),
                    &self.pipelines.gizmo.entries_bind_group,
                    &[offset],
                );
                gizmo_pass.draw_indexed(
                    0..self.pipelines.gizmo_buffers.scale_cube_index_count,
                    0,
                    0..1,
                );
            }
        }
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
    use std::sync::Arc;

    use super::DeviceToken;
    use super::*;
    use crate::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME;
    use crate::gizmo::axis::Axis;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::node::LeafPayload;
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::transform::Transform3D;

    /// Loader-faithful empty graph: identity-Group root, no leaves yet.
    fn graph_with_group_root() -> (Scene, NodeId) {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    /// Attaches an inline-mesh leaf under `parent` with the given
    /// local transform and returns the leaf's [`NodeId`].
    fn add_inline_leaf(
        g: &mut Scene,
        parent: NodeId,
        mesh: IndexedMesh,
        transform: Transform3D,
    ) -> NodeId {
        let leaf = g
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("add leaf");
        g.add_child(parent, leaf).expect("attach leaf");
        leaf
    }

    /// Build envelope used by the render tests below. Held as a
    /// module-level constant so each test passes the same value to
    /// `render` without extra boilerplate.
    const TEST_BV: BuildVolume = HARDCODED_DEFAULT_BUILD_VOLUME;

    /// `max_buffer_size` of the small-buffer test device.
    const SMALL_BUFFER_LIMIT: u64 = 1024;
    /// `max_buffer_size` of the tiny-buffer test device: below every
    /// static buffer the renderer uploads, so each of them is refused.
    const TINY_BUFFER_LIMIT: u64 = 64;
    /// `max_texture_dimension_2d` of the small-texture test device.
    const SMALL_TEXTURE_LIMIT: u32 = 256;

    #[derive(Clone)]
    struct TestDevices {
        default_limits: (wgpu::Device, wgpu::Queue),
        small_buffers: (wgpu::Device, wgpu::Queue),
        tiny_buffers: (wgpu::Device, wgpu::Queue),
        small_textures: (wgpu::Device, wgpu::Queue),
        /// `max_bind_groups: 1`: every pipeline with 2 bind groups is
        /// refused at its layout.
        one_bind_group: (wgpu::Device, wgpu::Queue),
    }

    /// Builds the test devices once per process, on one adapter, one
    /// request after the other. Returns `None` if no adapter is
    /// available, the caller soft-skips so CI without a GPU isn't
    /// blocked.
    ///
    /// One shared adapter across every GPU test, rather than one per
    /// test: concurrent `wgpu::Instance::new` plus `request_adapter`
    /// against the same Vulkan ICD raced and crashed RADV reliably at
    /// `--test-threads=2`, intermittently at higher parallelism. wgpu's
    /// Device and Queue are `Send + Sync` and `Arc`-shaped, so cloning
    /// them out is cheap and concurrent test access is fine.
    fn shared_test_devices() -> Option<TestDevices> {
        static DEVICES: std::sync::OnceLock<Option<TestDevices>> = std::sync::OnceLock::new();
        DEVICES
            .get_or_init(|| {
                let instance =
                    wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
                let adapter = pollster::block_on(
                    instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
                )
                .ok()?;
                let request = |label: &'static str, required_limits: wgpu::Limits| {
                    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                        label: Some(label),
                        required_features: wgpu::Features::empty(),
                        required_limits,
                        memory_hints: wgpu::MemoryHints::default(),
                        experimental_features: wgpu::ExperimentalFeatures::default(),
                        trace: wgpu::Trace::Off,
                    }))
                    .ok()
                };
                // Default limits are the base profile `run()` asks for.
                let default_limits = request("mesh-viewer test device", wgpu::Limits::default())?;
                let small_buffers = request(
                    "mesh-viewer small-buffer test device",
                    wgpu::Limits {
                        max_buffer_size: SMALL_BUFFER_LIMIT,
                        ..wgpu::Limits::default()
                    },
                )?;
                let tiny_buffers = request(
                    "mesh-viewer tiny-buffer test device",
                    wgpu::Limits {
                        max_buffer_size: TINY_BUFFER_LIMIT,
                        ..wgpu::Limits::default()
                    },
                )?;
                let small_textures = request(
                    "mesh-viewer small-texture test device",
                    wgpu::Limits {
                        max_texture_dimension_2d: SMALL_TEXTURE_LIMIT,
                        ..wgpu::Limits::default()
                    },
                )?;
                let one_bind_group = request(
                    "mesh-viewer one-bind-group test device",
                    wgpu::Limits {
                        max_bind_groups: 1,
                        ..wgpu::Limits::default()
                    },
                )?;
                Some(TestDevices {
                    default_limits,
                    small_buffers,
                    tiny_buffers,
                    small_textures,
                    one_bind_group,
                })
            })
            .clone()
    }

    /// The default-limits test device.
    fn shared_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        shared_test_devices().map(|d| d.default_limits)
    }

    /// The test device whose `max_buffer_size` is [`SMALL_BUFFER_LIMIT`].
    fn shared_small_buffer_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        shared_test_devices().map(|d| d.small_buffers)
    }

    /// The test device whose `max_buffer_size` is [`TINY_BUFFER_LIMIT`].
    fn shared_tiny_buffer_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        shared_test_devices().map(|d| d.tiny_buffers)
    }

    /// The test device whose `max_texture_dimension_2d` is
    /// [`SMALL_TEXTURE_LIMIT`].
    fn shared_small_texture_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        shared_test_devices().map(|d| d.small_textures)
    }

    /// The test device whose `max_bind_groups` is 1.
    fn shared_one_bind_group_test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        shared_test_devices().map(|d| d.one_bind_group)
    }

    /// Copies `buffer` back to the CPU (it must carry `COPY_SRC`).
    fn read_buffer_back(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
    ) -> Vec<u8> {
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer test buffer readback"),
            size: buffer.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mesh-viewer test buffer readback encoder"),
        });
        encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, buffer.size());
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

    /// First reachable leaf's [`NodeId`] in pre-order traversal. The
    /// renderer's selection key is the leaf's stable id, so the
    /// render tests resolve "the only leaf" through this helper
    /// instead of fabricating an id.
    fn first_leaf(graph: &Scene) -> NodeId {
        use sc_mesh_core::scene::node::NodeKind;
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
                NodeKind::Reference { .. } => {}
            }
        }
        panic!("test fixture has no leaves");
    }

    fn triangle_graph_with_translation(t: AffineTransform3D) -> Scene {
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
    fn renderer_construction_under_default_limits_succeeds() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let _renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
    }

    #[test]
    fn renderer_new_names_the_mesh_stage_on_a_one_bind_group_device() {
        let Some((device, queue)) = shared_one_bind_group_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let Err(err) = Renderer::new(device, DeviceToken::next(), queue) else {
            panic!("a 2-bind-group pipeline must be refused on a 1-bind-group device");
        };
        assert_eq!(err.stage, RendererStage::Mesh);
        let text = err.to_string();
        assert!(text.contains("mesh pipeline"), "{text}");
        assert!(
            text.contains("bind"),
            "the cause names the bind group limit: {text}"
        );
    }

    #[test]
    fn two_group_pipelines_are_refused_on_a_one_bind_group_device_and_one_group_ones_are_not() {
        let Some((device, queue)) = shared_one_bind_group_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        assert!(MeshPipeline::create(&device).is_err());
        assert!(OutlinePipeline::create(&device).is_ok());
        let gizmo_buffers =
            GizmoBuffers::create(&device, &queue).expect("buffers need no bind groups");
        assert!(GizmoPipeline::create(&device, &gizmo_buffers).is_err());
    }

    /// `GpuScene::build` succeeds against the default-limits device
    /// for a non-trivial graph with a non-identity transform.
    #[test]
    fn gpu_scene_build_succeeds_under_default_limits() {
        let Some((device, _queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
        let gpu = GpuScene::build(&device, DeviceToken::next(), &graph)
            .expect("GpuScene::build should succeed");
        // Stride must be at least one ModelEntry and a multiple of the
        // device's reported alignment.
        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        assert!(gpu.model_stride() >= MODEL_ENTRY_SIZE);
        assert_eq!(gpu.model_stride() % alignment, 0);
    }

    /// `build_with_progress` ticks once per object per pass with
    /// triangle counts: the single-triangle graph yields one flatten
    /// tick and one buffer tick, ending at the doubled total.
    #[test]
    fn gpu_scene_build_reports_two_pass_triangle_progress() {
        let Some((device, _queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
        let mut ticks: Vec<scene_upload::UploadProgress> = Vec::new();
        GpuScene::build_with_progress(&device, DeviceToken::next(), &graph, |p| ticks.push(p))
            .expect("GpuScene::build_with_progress should succeed");
        let expected: Vec<scene_upload::UploadProgress> = [(1, 2), (2, 2)]
            .into_iter()
            .map(
                |(processed_triangles, total_triangles)| scene_upload::UploadProgress {
                    processed_triangles,
                    total_triangles,
                },
            )
            .collect();
        assert_eq!(ticks, expected);
    }

    /// One leaf whose mesh has `face_count` faces over 3 shared
    /// vertices: an index buffer of `12 * face_count` bytes next to a
    /// 36-byte vertex buffer.
    fn graph_with_one_leaf_of_faces(face_count: usize) -> Scene {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: (0..face_count)
                .map(|_| IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 2]))
                .collect(),
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
        g
    }

    /// A leaf whose index buffer is over the device's `max_buffer_size`
    /// is refused with an error, not a panic.
    #[test]
    fn gpu_scene_build_refuses_a_leaf_above_the_device_buffer_limit() {
        let Some((device, _queue)) = shared_small_buffer_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        // 100 faces: 1200 index bytes against a 1024-byte limit.
        let graph = graph_with_one_leaf_of_faces(100);
        let err = GpuScene::build(&device, DeviceToken::next(), &graph)
            .err()
            .expect("a leaf above the limit is refused");
        let refusal = err
            .downcast_ref::<scene_upload::LeafBufferExceedsDeviceLimit>()
            .expect("the refusal is typed");
        assert_eq!(refusal.buffer_kind, scene_upload::LeafBufferKind::Index);
        assert_eq!(refusal.num_elements, 100);
        assert_eq!(refusal.num_bytes, 1200);
        assert_eq!(refusal.max_buffer_size, SMALL_BUFFER_LIMIT);
    }

    /// A leaf whose index buffer is exactly at the limit builds.
    #[test]
    fn gpu_scene_build_accepts_a_leaf_at_the_device_buffer_limit() {
        let Some((device, _queue)) = shared_small_buffer_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        // 85 faces: 1020 index bytes, padded to 1020 (already 4-aligned).
        let graph = graph_with_one_leaf_of_faces(85);
        GpuScene::build(&device, DeviceToken::next(), &graph)
            .expect("a leaf at the limit should build");
    }

    /// Full end-to-end render: build the renderer, install a scene
    /// with a non-identity transform, drive a render at a tiny size.
    /// The original storage-buffer-in-vertex bug surfaced as a runtime
    /// panic during `RenderingSetup`'s `Renderer::new`; this test
    /// reproduces that exact code path against the same limits and
    /// would have caught the regression before runtime.
    #[test]
    fn render_succeeds_under_default_limits() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("render should produce a texture");
        assert_eq!(texture.size().width, 64);
        assert_eq!(texture.size().height, 64);

        // A second upload replaces the held scene and renders again.
        let graph2 =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 7.0, 0.0]));
        renderer
            .upload_scene(&graph2)
            .expect("a second upload_scene under default limits");
        let bounds2 = renderer
            .scene_bounds()
            .expect("scene bounds follow the new scene");
        assert_ne!(bounds2, bounds);
        renderer
            .render(
                Some(SceneRender {
                    graph: &graph2,
                    selection: None,
                    gizmo: None,
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("render after the second upload should produce a texture");
    }

    /// A buffer the device refuses comes back as `Err`, with the wgpu
    /// error, instead of a panic.
    #[test]
    fn create_buffer_reporting_returns_the_devices_refusal_without_panicking() {
        let Some((device, _queue)) = shared_small_buffer_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let err = device_ops::create_buffer_reporting(
            &device,
            &wgpu::BufferDescriptor {
                label: Some("over the small limit"),
                size: SMALL_BUFFER_LIMIT + 8,
                usage: wgpu::BufferUsages::INDEX,
                mapped_at_creation: false,
            },
        )
        .expect_err("a buffer above max_buffer_size is refused");
        assert!(matches!(err, wgpu::Error::Validation { .. }));
        assert!(err.to_string().contains("maximum buffer size"), "{err}");
    }

    /// Within the limit the helper hands back a buffer that can be
    /// mapped, written and unmapped like any other.
    #[test]
    fn create_buffer_reporting_yields_a_usable_buffer_within_the_limit() {
        let Some((device, _queue)) = shared_small_buffer_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let buffer = device_ops::create_buffer_reporting(
            &device,
            &wgpu::BufferDescriptor {
                label: Some("at the small limit"),
                size: SMALL_BUFFER_LIMIT,
                usage: wgpu::BufferUsages::INDEX,
                mapped_at_creation: true,
            },
        )
        .expect("a buffer at max_buffer_size is accepted");
        buffer
            .get_mapped_range_mut(..)
            .copy_from_slice(&[7u8; SMALL_BUFFER_LIMIT as usize]);
        buffer.unmap();
        assert_eq!(buffer.size(), SMALL_BUFFER_LIMIT);
    }

    /// The gizmo geometry is uploaded without mapping: a device that
    /// refuses the buffers answers with `Err`, not a panic.
    #[test]
    fn gizmo_buffers_are_refused_typed_on_a_tiny_buffer_device() {
        let Some((device, queue)) = shared_tiny_buffer_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let err = GizmoBuffers::create(&device, &queue)
            .err()
            .expect("the tiny-buffer device refuses the gizmo geometry");
        assert!(matches!(err, wgpu::Error::Validation { .. }), "{err}");
        assert!(err.to_string().contains("maximum buffer size"), "{err}");
    }

    /// Same for the plate geometry, which is uploaded at render time.
    #[test]
    fn plate_buffers_are_refused_typed_on_a_tiny_buffer_device() {
        let Some((device, queue)) = shared_tiny_buffer_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let err = PlateBuffers::create(&device, &queue, TEST_BV)
            .err()
            .expect("the tiny-buffer device refuses the plate geometry");
        assert!(
            matches!(
                err,
                RenderError::DeviceRefused {
                    what: RenderAllocation::PlateGeometry,
                    ..
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("maximum buffer size"), "{err}");
    }

    /// `upload_static_buffer` fills the buffer through the queue: the
    /// bytes read back are the bytes handed in.
    #[test]
    fn upload_static_buffer_round_trips_contents() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let contents: Vec<u8> = (0u8..48).collect();
        let buffer = device_ops::upload_static_buffer(
            &device,
            &queue,
            "round-trip",
            &contents,
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_SRC,
        )
        .expect("a 48-byte buffer under default limits");
        assert_eq!(buffer.size(), 48);
        assert_eq!(read_buffer_back(&device, &queue, &buffer), contents);
    }

    /// With the error logger installed, a device error nothing scopes
    /// is logged; the calling thread survives it.
    #[test]
    fn after_the_error_logger_is_installed_an_unscoped_device_error_no_longer_panics() {
        let Some((device, _queue)) = shared_small_buffer_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        install_device_error_logger(&device);
        let worker = std::thread::spawn(move || {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("unscoped, over the small limit"),
                size: SMALL_BUFFER_LIMIT + 8,
                usage: wgpu::BufferUsages::INDEX,
                mapped_at_creation: false,
            });
            buffer.size()
        });
        assert_eq!(
            worker
                .join()
                .expect("the device error must not end the thread"),
            SMALL_BUFFER_LIMIT + 8
        );
    }

    /// Error scopes are per thread: 2 threads scoping refused
    /// allocations at the same time each get their own refusal back
    /// and neither trips wgpu's scope-order check.
    #[test]
    fn concurrent_scoped_allocations_on_2_threads_each_get_their_own_refusal() {
        let Some((device, _queue)) = shared_small_buffer_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let start = Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let device = device.clone();
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    (0..500).all(|_| {
                        device_ops::create_buffer_reporting(
                            &device,
                            &wgpu::BufferDescriptor {
                                label: Some("concurrently over the small limit"),
                                size: SMALL_BUFFER_LIMIT + 8,
                                usage: wgpu::BufferUsages::INDEX,
                                mapped_at_creation: false,
                            },
                        )
                        .is_err()
                    })
                })
            })
            .collect();
        for worker in workers {
            assert!(
                worker
                    .join()
                    .expect("scoped allocation must not panic on a worker thread"),
                "every refused allocation must come back as Err"
            );
        }
    }

    /// The slice-view cap renders without crashing: once `update_cap`
    /// installs a coverage texture, a render with a clip height set still
    /// produces a texture, so the cap pipeline and its draw are exercised.
    #[test]
    fn render_with_a_cap_produces_a_texture() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
            .expect("upload_scene under default limits");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);

        let mut build = || {
            Some(CapRaster {
                coverage: vec![255u8; 4 * 4],
                width: 4,
                height: 4,
            })
        };
        renderer
            .update_cap(Some(1), &mut build)
            .expect("a 4 x 4 cap texture is not refused");

        let texture = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: None,
                    gizmo: None,
                    clip_z: Some(1.0),
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("render should produce a texture with a cap");
        assert_eq!(texture.size().width, 64);
        assert_eq!(texture.size().height, 64);
    }

    /// A shared (Device, Queue) pinned to the DX12 backend, built lazily.
    /// The default-backend `shared_test_device` may resolve to Vulkan on
    /// Windows when a Vulkan ICD is present, so it does not guarantee the
    /// DX12 path is exercised. `None` when no DX12 adapter is available.
    #[cfg(windows)]
    fn shared_dx12_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        static DEVICE: std::sync::OnceLock<Option<(wgpu::Device, wgpu::Queue)>> =
            std::sync::OnceLock::new();
        DEVICE
            .get_or_init(|| {
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::DX12,
                    ..wgpu::InstanceDescriptor::new_without_display_handle()
                });
                let adapter = pollster::block_on(
                    instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
                )
                .ok()?;
                let (device, queue) =
                    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                        label: Some("mesh-viewer DX12 test device"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::default(),
                        memory_hints: wgpu::MemoryHints::default(),
                        experimental_features: wgpu::ExperimentalFeatures::default(),
                        trace: wgpu::Trace::Off,
                    }))
                    .ok()?;
                Some((device, queue))
            })
            .clone()
    }

    /// Windows production pins the wgpu backend to DX12 (see the GUI's
    /// shared-device setup): Slint's Skia renderer rejects a non-DX12
    /// adapter on Windows. The other render tests build a default-backend
    /// device that may be Vulkan even on Windows, so this test pins DX12
    /// explicitly and drives a full end-to-end render, proving shader
    /// compilation, bind-group validation, and a draw call all work on
    /// the backend Windows actually ships. Soft-skips with no DX12
    /// adapter.
    #[cfg(windows)]
    #[test]
    fn render_succeeds_on_dx12_backend() {
        let Some((device, queue)) = shared_dx12_device() else {
            eprintln!("skipping: no DX12 wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
            .expect("upload_scene on the DX12 device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("render on DX12 should produce a texture");
        assert_eq!(texture.size().width, 64);
        assert_eq!(texture.size().height, 64);
    }

    /// Identical to `render_succeeds_under_default_limits` but with a
    /// non-`None` selection so the `flags.x` lane carries a 1 through
    /// the per-frame uniform write. Catches any default-limits
    /// regression in the selection-flag path before runtime.
    #[test]
    fn render_with_selection_under_default_limits() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
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
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("render with selection runs the outline pass");
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: None,
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
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
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("first gizmo render should succeed");
        let _ = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: Some(first_leaf(&graph)),
                    gizmo: Some(frame),
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("cached gizmo resources survive a second frame");
    }

    /// Idle visibility renders all three handle categories
    /// simultaneously, exercises the translate, rotate, and scale
    /// draws in a single frame.
    #[test]
    fn render_with_idle_gizmo_under_default_limits() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("idle gizmo render should succeed");
    }

    /// Rotate-drag visibility renders only the three rotate arcs.
    /// Pins the rotate-arc VBO/IBO + per-axis camera-roll wiring
    /// under default-limits.
    #[test]
    fn render_with_rotate_gizmo_under_default_limits() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("rotate gizmo render should succeed");
    }

    /// Scale-axis renders the three midpoint cubes, pins the
    /// scale-pass wiring under default-limits.
    #[test]
    fn render_with_scale_gizmo_under_default_limits() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("scale-axis gizmo render should succeed");
    }

    /// Multi-object scene drives one draw call per object; the
    /// dynamic-offset bind path must keep `model_entry` pointing at
    /// the right slot for each draw. Pure smoke test (no read-back),
    /// but it exercises the full draw loop with N > 1.
    #[test]
    fn render_multi_object_scene_under_default_limits() {
        let Some((device, queue)) = shared_test_device() else {
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
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("render multi-object");
    }

    /// Render with no scene installed: the plate alone should
    /// produce a non-`None` texture. Two consecutive calls so the
    /// second exercises the cached `plate_buffers` path.
    #[test]
    fn render_plate_only_under_default_limits() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let (centre, radius) = TEST_BV.framing(None);
        let camera = OrbitCamera::new(centre, radius);
        let _ = renderer
            .render(None, TEST_BV, &camera, 64, 64)
            .expect("render is not refused")
            .expect("first plate-only render should succeed");
        let _ = renderer
            .render(None, TEST_BV, &camera, 64, 64)
            .expect("render is not refused")
            .expect("second plate-only render should succeed");
    }

    #[test]
    fn render_clamps_the_target_to_the_device_texture_ceiling() {
        let Some((device, queue)) = shared_small_texture_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the small-texture test device");
        let (centre, radius) = TEST_BV.framing(None);
        let camera = OrbitCamera::new(centre, radius);
        let texture = renderer
            .render(None, TEST_BV, &camera, 300, 150)
            .expect("render is not refused")
            .expect("a viewport wider than the ceiling still renders");
        assert_eq!(
            (texture.size().width, texture.size().height),
            (SMALL_TEXTURE_LIMIT, SMALL_TEXTURE_LIMIT / 2),
            "the target is clamped to the ceiling with the aspect kept"
        );
    }

    #[test]
    fn msaa_targets_are_refused_typed_on_a_small_texture_device() {
        let Some((device, queue)) = shared_small_texture_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the small-texture test device");
        let over = SMALL_TEXTURE_LIMIT + 44;
        let refusal = MsaaTargets::create(
            &device,
            &renderer.pipelines.outline.bind_group_layout,
            over,
            over,
        )
        .err()
        .expect("attachments over the texture ceiling are refused");
        assert!(matches!(
            refusal,
            RenderError::DeviceRefused {
                what: RenderAllocation::MsaaTargets,
                ..
            }
        ));
    }

    #[test]
    fn colour_targets_are_refused_typed_on_a_small_texture_device() {
        let Some((device, _queue)) = shared_small_texture_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let over = SMALL_TEXTURE_LIMIT + 44;
        let refusal = TargetPool::create(&device, over, over)
            .err()
            .expect("colour targets over the texture ceiling are refused");
        assert!(matches!(
            refusal,
            RenderError::DeviceRefused {
                what: RenderAllocation::ColourTargets,
                ..
            }
        ));
    }

    #[test]
    fn update_cap_reports_a_refused_texture_and_leaves_the_cap_off() {
        let Some((device, queue)) = shared_small_texture_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the small-texture test device");
        let side = SMALL_TEXTURE_LIMIT * 2;
        let builds = std::cell::Cell::new(0);
        let mut build = || {
            builds.set(builds.get() + 1);
            Some(CapRaster {
                coverage: vec![255u8; (side * side) as usize],
                width: side,
                height: side,
            })
        };

        let refusal = renderer
            .update_cap(Some(1), &mut build)
            .expect_err("a cap texture over the ceiling is refused");
        assert!(matches!(
            refusal,
            RenderError::DeviceRefused {
                what: RenderAllocation::CapTexture,
                ..
            }
        ));
        assert_eq!(builds.get(), 1);

        // The same key is not retried; a new key is.
        renderer
            .update_cap(Some(1), &mut build)
            .expect("a remembered refusal is not an error");
        assert_eq!(builds.get(), 1);
        let _ = renderer.update_cap(Some(2), &mut build);
        assert_eq!(builds.get(), 2);

        let (centre, radius) = TEST_BV.framing(None);
        let camera = OrbitCamera::new(centre, radius);
        renderer
            .render(None, TEST_BV, &camera, 64, 64)
            .expect("the frame renders without the cap")
            .expect("a 64 x 64 frame produces a texture");
    }

    /// A render call against a different `BuildVolume` rebuilds the
    /// cached plate buffers so the new envelope reaches the next
    /// frame. Verified through the public path: render once at the
    /// default envelope, then at a smaller one, and confirm both
    /// frames succeed and the cache key reflects the latest call.
    #[test]
    fn render_plate_buffers_rebuild_on_envelope_change() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let camera = OrbitCamera::new(Vec3::ZERO, 100.0);

        let _ = renderer
            .render(None, TEST_BV, &camera, 64, 64)
            .expect("render is not refused")
            .expect("render at default envelope");
        let cached_after_first = renderer
            .pipelines
            .plate_buffers
            .as_ref()
            .map(|b| b.build_volume);
        assert_eq!(cached_after_first, Some(TEST_BV));

        let smaller = BuildVolume::new(50.0, 50.0, 50.0);
        let _ = renderer
            .render(None, smaller, &camera, 64, 64)
            .expect("render is not refused")
            .expect("render at smaller envelope");
        let cached_after_second = renderer
            .pipelines
            .plate_buffers
            .as_ref()
            .map(|b| b.build_volume);
        assert_eq!(cached_after_second, Some(smaller));
    }

    /// Builds a one-triangle graph that pokes one vertex past the
    /// `TEST_BV`'s +X envelope wall at x = +96 mm. Two anchor
    /// vertices stay inside the envelope so the graph's AABB
    /// straddles the wall (and the camera framing centres on it).
    fn x_pos_breaching_graph() -> Scene {
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
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue.clone())
            .expect("Renderer::new under the test device");
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                width,
                height,
            )
            .expect("render is not refused")
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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                width,
                height,
            )
            .expect("render is not refused")
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

    /// `install_scene` with a `GpuScene` built against the same device
    /// as the renderer must report success and leave the renderer with
    /// a populated scene (observable through `scene_bounds`).
    #[test]
    fn install_scene_with_matching_device_succeeds() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        // Renderer and GpuScene must share the same `device_token`
        // for `install_scene` to accept the worker-built scene;
        // mismatched tokens are the device-rotation rejection path
        // covered by the `install_scene_with_mismatched_device_returns_false`
        // integration test.
        let token = DeviceToken::next();
        let mut renderer = Renderer::new(device.clone(), token, queue)
            .expect("Renderer::new under the test device");
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        let gpu = GpuScene::build(&device, token, &graph).expect("gpu scene build");
        let installed = renderer.install_scene(gpu);
        assert!(installed);
        assert!(renderer.scene_bounds().is_some());
    }

    /// A live `Scene` that grew an extra leaf after `upload_scene` must
    /// not crash `render`: the per-leaf model uploads target a
    /// `model_buffer` sized at install time, so writing one entry per
    /// live leaf would OOB the buffer (the wgpu runtime would panic on
    /// the trailing `queue.write_buffer`). The renderer must detect the
    /// leaf-count drift and skip the per-frame mesh path until the next
    /// install brings the GpuScene back in sync with the graph.
    #[test]
    fn render_against_grown_graph_does_not_panic() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device.clone(), DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let mut graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
            .expect("upload single-leaf graph");

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
        let root = graph.roots()[0];
        add_inline_leaf(
            &mut graph,
            root,
            mesh,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
        );

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
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("render against the live mutated graph still produces a texture");
        assert_eq!(texture.size().width, 64);
    }

    /// `clear_scene` drops the installed scene; subsequent
    /// `scene_bounds` reads return `None`.
    #[test]
    fn clear_scene_drops_the_installed_scene() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer.upload_scene(&graph).expect("upload");
        assert!(renderer.scene_bounds().is_some());
        renderer.clear_scene();
        assert!(renderer.scene_bounds().is_none());
    }

    /// A graph whose last leaf was deleted must upload cleanly. There
    /// is nothing to mirror, so the renderer ends up holding no scene
    /// rather than reporting a failure the viewport would have to
    /// recover from.
    #[test]
    fn upload_scene_on_a_leafless_graph_succeeds_and_installs_nothing() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let (graph, _root) = graph_with_group_root();
        renderer
            .upload_scene(&graph)
            .expect("a leafless graph uploads as an empty mirror");
        assert!(renderer.scene_bounds().is_none());
    }

    /// The reported scenario end to end: a one-object scene, the
    /// object deleted, then the upload the deletion triggers. The
    /// render that follows must still produce a texture, which is what
    /// proves the empty case raises no wgpu validation error and that
    /// the viewport keeps painting the plate.
    #[test]
    fn render_after_deleting_the_last_leaf_still_produces_a_texture() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let mut graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        renderer
            .upload_scene(&graph)
            .expect("upload one-leaf graph");
        let bounds = renderer.scene_bounds().expect("scene bounds populated");
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
        let camera = OrbitCamera::new(center, radius);

        let leaf = first_leaf(&graph);
        graph.remove_leaf(leaf).expect("delete the only object");
        renderer
            .upload_scene(&graph)
            .expect("the post-delete upload succeeds");
        assert!(renderer.scene_bounds().is_none());

        let texture = renderer
            .render(
                Some(SceneRender {
                    graph: &graph,
                    selection: None,
                    gizmo: None,
                    clip_z: None,
                }),
                TEST_BV,
                &camera,
                64,
                64,
            )
            .expect("render is not refused")
            .expect("render after the delete still produces a texture");
        assert_eq!(texture.size().width, 64);
        assert_eq!(texture.size().height, 64);
    }

    /// Re-uploading a leaf again after the scene went empty installs a
    /// fresh mirror, so the empty case is a transition the renderer can
    /// come back from rather than a terminal state.
    #[test]
    fn upload_after_an_empty_upload_installs_a_scene_again() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let (mut graph, root) = graph_with_group_root();
        renderer.upload_scene(&graph).expect("empty upload");
        assert!(renderer.scene_bounds().is_none());

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
        add_inline_leaf(&mut graph, root, mesh, Transform3D::Identity);
        renderer.upload_scene(&graph).expect("upload after refill");
        assert!(renderer.scene_bounds().is_some());
    }

    /// `invalidate_sizing` drops only the viewport-sized allocations;
    /// the installed scene survives (still reports bounds).
    #[test]
    fn invalidate_sizing_keeps_the_installed_scene() {
        let Some((device, queue)) = shared_test_device() else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        let mut renderer = Renderer::new(device, DeviceToken::next(), queue)
            .expect("Renderer::new under the test device");
        let graph =
            triangle_graph_with_translation(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        renderer.upload_scene(&graph).expect("upload");
        let bounds_before = renderer.scene_bounds().expect("bounds populated");
        renderer.invalidate_sizing();
        let bounds_after = renderer.scene_bounds().expect("bounds preserved");
        assert_eq!(bounds_before, bounds_after);
    }
}
