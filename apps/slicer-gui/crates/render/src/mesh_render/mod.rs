/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use glam::{Mat4, Vec3};
use sc_scene::Scene;
use sc_scene::node::NodeId;
use slint::wgpu_30::wgpu;

use crate::build_volume::BuildVolume;
use crate::camera::{DEFAULT_DISTANCE_FACTOR, OrbitCamera};
use crate::front_label::LabelRaster;
use crate::slice_cap::CapRaster;

mod device_loss;
mod device_ops;
mod error;
mod frame;
mod gizmo_buffers;
mod gizmo_entries;
mod hull_points;
#[cfg(any(test, feature = "test-support"))]
pub mod mock;
mod overhang_map;
mod overlay;
mod smooth_shading;
mod support_lines;
mod surface_paint;
mod tip_coverage;
pub use smooth_shading::set_smooth_models;
mod pipelines;
mod prepared;
mod readback;
mod render_size;
mod renderer_trait;
pub mod scene_upload;
mod targets;
#[cfg(test)]
mod tests_gpu;
#[cfg(test)]
mod tests_overhang_map;
#[cfg(test)]
mod tests_under_plate;

pub use device_loss::DeviceLoss;
use device_ops::scoped;
pub use error::{
    AllocationRefusal, RenderAllocation, RenderError, RendererInitError, RendererStage,
    StageRefusal,
};
use frame::Uniforms;
use gizmo_buffers::GizmoBuffers;
use gizmo_entries::{
    GIZMO_ENTRY_SLOTS, GizmoDrawEntry, GizmoLayer, GizmoMesh, GizmoUniformsGpu, build_gizmo_draws,
};
pub use gizmo_entries::{GizmoDragVisual, GizmoFrame, GizmoHover, GizmoVisibility};

/// How many draws the gizmo pass records for `frame`, for the cost probe.
#[cfg(feature = "test-support")]
pub fn gizmo_draw_count_for_test(frame: &GizmoFrame) -> usize {
    build_gizmo_draws(frame).len()
}
pub use overhang_map::OverhangMap;
pub use overlay::{Overlay, OverlayMesh, OverlayTint};
use pipelines::cap::CapPipeline;
use pipelines::gizmo::{GizmoBindGroup, GizmoPipeline};
use pipelines::label::LabelPipeline;
use pipelines::mesh::{MeshBindGroup, MeshPipeline};
use pipelines::outline::{OutlineBindGroup, OutlinePipeline};
use pipelines::plate::{FrontTab, PlateBindGroup, PlateBuffers, PlatePipeline};
use pipelines::surface_paint::SurfacePaintPipeline;
pub use prepared::prepare_pipelines;
pub use readback::FramePixels;
use render_size::clamp_render_size;
pub use renderer_trait::MeshRenderer;
pub use scene_upload::GpuScene;
use scene_upload::{
    MODEL_ENTRY_SIZE, WorldBoxes, build_envelope_warning_mask, build_model_entries,
    renderable_leaf_count,
};
pub use support_lines::SupportLines;
pub use surface_paint::{BrushRing, PaintLayer};
use targets::{MsaaTargets, TargetPool};
pub use tip_coverage::SupportTip;

/// Scene-dependent inputs to [`Renderer::render`]. The selection drives the
/// outline post-pass and the gizmo overlay drives the gizmo pass; both are
/// meaningful only when a scene is loaded, so they live alongside the scene
/// reference rather than on [`Renderer::render`]'s top-level signature where
/// the type system would let a caller pass a selection against `scene: None`.
#[derive(Clone, Copy, Debug)]
pub struct SceneRender<'a> {
    pub graph: &'a Scene,
    /// Stable [`NodeId`]s, sorted, whose stencil mask the outline post-pass
    /// dilates: every selected leaf (#13). Empty skips the outline pass; a
    /// `NodeId` that does not name a reachable leaf is ignored.
    pub selection: &'a [NodeId],
    /// Gizmo overlay parameters; the gizmo pass runs only when this is `Some`.
    pub gizmo: Option<GizmoFrame>,
    /// World-space Z (mm) above which mesh fragments are discarded, so the
    /// slice slider can reveal the model layer by layer. `None` renders the
    /// whole mesh.
    pub clip_z: Option<f32>,
}

/// What a frame draws around the models.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FrameContents {
    /// The machine's workspace: the plate, its cage and envelope, and the slice
    /// cap when one is installed.
    Workspace,
    /// The models by themselves.
    ModelsOnly,
}

/// How a frame is dressed: what goes around the models, whether the
/// overhang map goes over them, and how high the rafts reach, for the view
/// from under the plate to hide them (#481; 0 without rafts).
#[derive(Clone, Copy, Debug, PartialEq)]
struct FrameLook {
    contents: FrameContents,
    overhang_map: Option<OverhangMap>,
    raft_top_mm: f32,
    /// From under the plate, the height above which a support mesh fades out
    /// for its lines (#493); `f32::MAX` keeps it.
    supports_shown_up_to_mm: f32,
}

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
// Depth24PlusStencil8 carries an 8-bit stencil alongside depth in a single
// attachment. The selection-outline post-pass writes `1` to stencil for the
// selected object via per-draw stencil reference, then reads that stencil
// aspect in a fullscreen edge-detect. Default-limits supported on every wgpu
// backend Slint may pick.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
// 4x MSAA: each pixel rasterises 4 samples and resolves to 1 colour, costing
// ~4x colour/depth/stencil attachment bandwidth in exchange for smooth mesh
// silhouettes and grid lines. Default-limits supported on every wgpu backend;
// 8x adds little visible benefit on this geometry.
const SAMPLE_COUNT: u32 = 4;
const FIT_FRACTION: f32 = 0.75;

/// The front label's vertices over the tab's top face, and under it.
const LABEL_TOP: std::ops::Range<u32> = 0..6;
const LABEL_UNDERSIDE: std::ops::Range<u32> = 6..12;

/// A `GpuScene` paired with its models bind group. The bind group references
/// the scene's storage buffer by handle, so it must be rebuilt every time the
/// storage buffer is recreated (i.e. on every scene install or upload).
struct InstalledScene {
    gpu: GpuScene,
    models_bind_group: wgpu::BindGroup,
}

/// Identity token used to detect cross-device installation of a `GpuScene`.
/// `wgpu::Device`'s own `PartialEq` proxies to the inner state's
/// value-equality, which compares 2 separately-created devices as equal even
/// when they are distinct GPU resources; the install-time guard cannot use it.
/// Every fresh device pairing must mint a new token via [`DeviceToken::next`];
/// `GpuScene::build` and `Renderer::new` store the same token, and
/// `Renderer::install_scene` rejects any `GpuScene` whose token does not match
/// the renderer's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceToken(u64);

impl DeviceToken {
    /// Mints a token that is process-globally unique for the lifetime of the
    /// process. Wrap raises on overflow at `u64::MAX` mints.
    pub fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Logs to stderr every uncaught wgpu error on `device`. Without this, wgpu's
/// default handler panics on them.
pub fn install_device_error_logger(device: &wgpu::Device) {
    device.on_uncaptured_error(std::sync::Arc::new(|err: wgpu::Error| {
        eprintln!("wgpu error (uncaptured): {err}");
    }));
}

/// GPU device pairing plus the identity token used to detect a cross-device
/// install. Grouped so the [`Renderer`] struct does not expose 3 loose fields
/// whose lifetime is tied together (the device, the queue, and the token are
/// minted together at `RenderingSetup` and dropped together at
/// `RenderingTeardown`).
struct RendererCore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    device_token: DeviceToken,
    device_loss: DeviceLoss,
    /// The device's `max_texture_dimension_2d`.
    max_texture_side: u32,
}

/// Per-pipeline GPU state. The mesh / outline / gizmo / plate pipelines are
/// immutable after construction; `gizmo_buffers` holds the gizmo pass's
/// vertex/index data, also immutable, while `plate_buffers` is cached lazily
/// because the plate geometry depends on the runtime `BuildVolume`.
struct Pipelines {
    mesh: MeshPipeline,
    outline: OutlinePipeline,
    gizmo: GizmoPipeline,
    gizmo_buffers: GizmoBuffers,
    plate: PlatePipeline,
    /// The slice-view cap pipeline, immutable after construction.
    cap: CapPipeline,
    cap_state: CapState,
    /// The plate-label pipeline, immutable after construction.
    label: LabelPipeline,
    label_state: LabelState,
    /// The paint and brush-ring pipelines, immutable after construction.
    surface_paint: SurfacePaintPipeline,
    /// Cached plate geometry for the current `BuildVolume`. `None` until the
    /// first render call; rebuilt when the per-frame `BuildVolume` differs from
    /// the cached one.
    plate_buffers: Option<PlateBuffers>,
}

impl Pipelines {
    /// Builds every pipeline of the viewport against `device`.
    fn create(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, RendererInitError> {
        let at = |stage: RendererStage| move |source| RendererInitError { stage, source };
        let mesh = MeshPipeline::create(device, queue).map_err(at(RendererStage::Mesh))?;
        let outline = OutlinePipeline::create(device).map_err(at(RendererStage::Outline))?;
        let gizmo_buffers =
            GizmoBuffers::create(device, queue).map_err(at(RendererStage::GizmoBuffers))?;
        let gizmo =
            GizmoPipeline::create(device, &gizmo_buffers).map_err(at(RendererStage::Gizmo))?;
        let plate = PlatePipeline::create(device, &mesh).map_err(at(RendererStage::Plate))?;
        let cap = CapPipeline::create(device, &mesh).map_err(at(RendererStage::Cap))?;
        let label = LabelPipeline::create(device, &mesh).map_err(at(RendererStage::Label))?;
        let surface_paint =
            SurfacePaintPipeline::create(device, &mesh).map_err(at(RendererStage::SurfacePaint))?;
        Ok(Self {
            mesh,
            outline,
            gizmo,
            gizmo_buffers,
            plate,
            cap,
            cap_state: CapState::Off,
            label,
            label_state: LabelState::Off,
            surface_paint,
            plate_buffers: None,
        })
    }
}

/// State of the slice-view cap texture, keyed by the layer it was last built
/// for. The bind group keeps the texture and its view alive.
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

/// State of the plate label's texture, keyed by the word it was last built for.
/// The bind group keeps the texture and its view alive, and the tab it is drawn
/// on is sized by that same word.
enum LabelState {
    Off,
    Shown {
        key: u64,
        tab: FrontTab,
        bind_group: wgpu::BindGroup,
    },
    Refused {
        key: u64,
    },
}

impl LabelState {
    fn key(&self) -> Option<u64> {
        match self {
            Self::Off => None,
            Self::Shown { key, .. } | Self::Refused { key } => Some(*key),
        }
    }

    /// Returns the tab carrying the label, `None` when there is no label to
    /// carry: the tab exists for the word.
    fn tab(&self) -> Option<FrontTab> {
        match self {
            Self::Shown { tab, .. } => Some(*tab),
            Self::Off | Self::Refused { .. } => None,
        }
    }
}

/// Viewport-sized GPU allocations that are dropped and recreated when the
/// render-target size changes. Both fields are `None` until the first render
/// call and after [`Renderer::invalidate_sizing`] / [`Renderer::clear_scene`].
struct TargetManager {
    msaa: Option<MsaaTargets>,
    targets: Option<TargetPool>,
}

pub struct Renderer {
    core: RendererCore,
    pipelines: Pipelines,
    scene: Option<InstalledScene>,
    /// The world box of each leaf, for the out-of-volume warning; emptied on
    /// every scene install.
    world_boxes: WorldBoxes,
    /// Buffers of meshes uploaded on a worker for models about to join the
    /// scene (#391), for the next upload to take instead of uploading them on
    /// the window's thread.
    adopted: scene_upload::UploadedMeshes,
    overlay: overlay::OverlaySlot,
    surface_paint: surface_paint::PaintSlot,
    /// The overhang map over the models of a workspace frame, when on.
    overhang_map: Option<OverhangMap>,
    /// The support tips the map counts as holding the faces around them,
    /// as last given, and the side of the cells they are hashed in (0
    /// without one) (#484).
    support_tips: Vec<SupportTip>,
    tip_cell_mm: f32,
    /// How high the rafts of the scene reach, mm: what of a support mesh
    /// the view from under the plate hides (#481).
    raft_top_mm: f32,
    /// From under the plate, the supports as lines (#493): the height above
    /// which a support mesh fades out (`f32::MAX` keeps it), and the lines
    /// drawn in its place.
    supports_shown_up_to_mm: f32,
    support_lines: support_lines::LinesSlot,
    target_manager: TargetManager,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("has_scene", &self.scene.is_some())
            .finish_non_exhaustive()
    }
}

/// Returns the out-of-volume warning mask that a paint of `graph` on
/// `build_volume` shows, in the plate shader's bit layout.
#[cfg(feature = "test-support")]
pub fn envelope_warning_mask_for_test(graph: &Scene, build_volume: BuildVolume) -> u32 {
    build_envelope_warning_mask(graph, build_volume, &mut WorldBoxes::default())
        .expect("the leaf walk succeeds")
}

impl Renderer {
    /// Computes the world boxes of `graph` as a frame does: the model entries
    /// and, with `warnings`, the out-of-volume warnings. `from_scratch` drops
    /// the boxes kept so far first. For the paste timing harness (#353).
    #[cfg(feature = "test-support")]
    pub fn refresh_world_boxes_for_test(
        &mut self,
        graph: &Scene,
        build_volume: BuildVolume,
        from_scratch: bool,
        warnings: bool,
    ) {
        if from_scratch {
            self.world_boxes.clear();
        }
        build_model_entries(graph, &[], build_volume, &mut self.world_boxes, false)
            .expect("entries");
        if warnings {
            build_envelope_warning_mask(graph, build_volume, &mut self.world_boxes).expect("mask");
        }
    }
}

/// Vertical field of view in radians.
///
/// Chosen so that a sphere of the scene's bounding radius fills `FIT_FRACTION`
/// of the viewport height when the camera sits at its initial framing distance
/// (`DEFAULT_DISTANCE_FACTOR * radius`). Being a pure function of those
/// constants, the FOV is viewport-independent, resizing the window rescales the
/// image but never distorts perspective.
pub fn default_fov_y() -> f32 {
    2.0 * (1.0 / (FIT_FRACTION * DEFAULT_DISTANCE_FACTOR)).atan()
}

impl Renderer {
    /// Builds every static GPU resource of the viewport against `device`.
    pub fn new(
        device: wgpu::Device,
        device_token: DeviceToken,
        queue: wgpu::Queue,
        device_loss: DeviceLoss,
    ) -> Result<Self, RendererInitError> {
        // Built ahead on a worker thread when [`prepare_pipelines`] was called
        // for this device; built here otherwise.
        let pipelines = match prepared::take(&device) {
            Some(prepared) => prepared?,
            None => Pipelines::create(&device, &queue)?,
        };
        let max_texture_side = device.limits().max_texture_dimension_2d;

        Ok(Self {
            core: RendererCore {
                device,
                queue,
                device_token,
                device_loss,
                max_texture_side,
            },
            pipelines,
            scene: None,
            world_boxes: WorldBoxes::default(),
            adopted: scene_upload::UploadedMeshes::default(),
            overlay: overlay::OverlaySlot::default(),
            surface_paint: surface_paint::PaintSlot::default(),
            overhang_map: None,
            support_tips: Vec::new(),
            tip_cell_mm: 0.0,
            raft_top_mm: 0.0,
            supports_shown_up_to_mm: f32::MAX,
            support_lines: support_lines::LinesSlot::default(),
            target_manager: TargetManager {
                msaa: None,
                targets: None,
            },
        })
    }

    /// Identity token of the device pairing this renderer was built against.
    /// Callers thread this token through to worker-side `GpuScene::build` so
    /// the eventual `install_scene` can match the worker's snapshot against the
    /// renderer's current device.
    pub fn device_token(&self) -> DeviceToken {
        self.core.device_token
    }

    pub fn clear_scene(&mut self) {
        self.scene = None;
        self.adopted = scene_upload::UploadedMeshes::default();
        self.world_boxes.clear();
        self.target_manager.msaa = None;
        self.target_manager.targets = None;
    }

    /// Drops the viewport-sized allocations (MSAA + target pool) so the next
    /// `render` reallocates them at the current `(width, height)`. Used by
    /// `ViewportController::on_resize_settled` to snap back from bucketed
    /// drag-resize sizing to exact pixel sizing once the user stops resizing.
    pub fn invalidate_sizing(&mut self) {
        self.target_manager.msaa = None;
        self.target_manager.targets = None;
    }

    /// World-space AABB of the currently uploaded scene, captured at install
    /// time via `Scene::aabb`. `None` if nothing has been uploaded yet.
    pub fn scene_bounds(&self) -> Option<(Vec3, Vec3)> {
        self.scene.as_ref().map(|s| s.gpu.aabb())
    }

    /// Replaces the renderer's scene with one built from `graph`, blocking the
    /// caller for the whole build. Afterwards the renderer holds the new scene
    /// on `Ok`, and no scene at all on `Err`. When `graph` has no renderable
    /// leaves ([`render`] then draws the plate alone).
    ///
    /// The meshes the scene before already uploaded keep their buffers, so a
    /// paste, a deletion or an undo uploads only the meshes new to the scene
    /// (#353).
    ///
    /// [`render`]: Self::render
    pub fn upload_scene(&mut self, graph: &Scene) -> Result<()> {
        if self.core.device_loss.is_lost() {
            return Err(scene_upload::DeviceLostDuringUpload.into());
        }
        // Released before the build, but for the buffers of the meshes the new
        // scene still draws, so the old and the new scene never occupy device
        // memory at the same time.
        let mut uploaded = self
            .scene
            .take()
            .map(|installed| installed.gpu.into_uploaded())
            .unwrap_or_default();
        uploaded.absorb(std::mem::take(&mut self.adopted));
        // An empty model uniform buffer would fail bind-group validation, so a
        // leafless graph gets no scene at all.
        let built = renderable_leaf_count(graph).and_then(|count| {
            if count == 0 {
                return Ok(None);
            }
            let mut uploaded = uploaded;
            uploaded.retain_drawn(graph)?;
            GpuScene::build_reusing(
                &self.core.device,
                self.core.device_token,
                graph,
                &self.core.device_loss,
                uploaded,
                |_| {},
            )
            .map(Some)
        });
        match built {
            Ok(Some(gpu)) => {
                self.attach_scene(gpu);
                Ok(())
            }
            Ok(None) => {
                self.clear_scene();
                Ok(())
            }
            Err(err) => {
                // The boxes are kept only across an install that names the
                // meshes they still stand for.
                self.world_boxes.clear();
                Err(err)
            }
        }
    }

    /// Installs a `GpuScene` built on a worker thread, but only if its
    /// `device_token` matches this renderer's and the device is not lost.
    /// Returns `false` otherwise; the caller falls back to the recovery upload
    /// path.
    pub fn install_scene(&mut self, gpu: GpuScene) -> bool {
        if self.core.device_token != gpu.device_token() || self.core.device_loss.is_lost() {
            return false;
        }
        self.attach_scene(gpu);
        true
    }

    /// Keeps the mesh buffers of `gpu`, built on a worker for models about to
    /// join the scene, for the next [`Self::upload_scene`] to take instead of
    /// uploading those meshes again (#391). Returns `false`, keeping nothing,
    /// when `gpu` was built on another device or the device is lost: the
    /// upload then builds them itself.
    pub fn adopt_meshes(&mut self, gpu: GpuScene) -> bool {
        if self.core.device_token != gpu.device_token() || self.core.device_loss.is_lost() {
            return false;
        }
        self.adopted.absorb(gpu.into_uploaded());
        true
    }

    fn attach_scene(&mut self, gpu: GpuScene) {
        // The bind group references a single ModelEntry-sized window into the
        // per-scene uniform buffer; the dynamic offset shifts that window to
        // the right slot per draw call. The window size must match
        // `min_binding_size` declared in the layout.
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
        self.world_boxes.keep_for(gpu.uploaded());
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
        // Freed before the new allocation, not after, so a refusal is not
        // caused by the bundle it replaces.
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

    /// Lazily create or rebuild the plate's GPU buffers. The cache key is the
    /// `BuildVolume` plus the front tab's footprint; a different value triggers
    /// a rebuild so a runtime envelope change reaches the next frame. On `Err`
    /// the cache is empty, so the next frame retries. The returned value clones
    /// the cached wgpu handles (reference counted), so no borrow of `self`
    /// outlives the call.
    fn ensure_plate_buffers(
        &mut self,
        build_volume: BuildVolume,
        front_tab: Option<FrontTab>,
    ) -> Result<PlateBuffers, RenderError> {
        let buffers = match self.pipelines.plate_buffers.take() {
            Some(cached)
                if cached.build_volume == build_volume && cached.front_tab == front_tab =>
            {
                cached
            }
            _ => {
                PlateBuffers::create(&self.core.device, &self.core.queue, build_volume, front_tab)?
            }
        };
        Ok(self.pipelines.plate_buffers.insert(buffers).clone())
    }

    /// Refresh the slice-view cap texture. `key` identifies the layer being
    /// capped; `None` clears the cap. When `key` matches the current cap (shown
    /// or refused) nothing happens, so a camera orbit (same layer) skips the
    /// rasterise. Otherwise `build` produces the coverage raster, which is
    /// uploaded as an R8 texture; a `build` yielding `None` clears the cap.
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
        if self.core.device_loss.is_lost() {
            return Err(RenderError::DeviceLost);
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

    /// Refresh the plate label for the word identified by `key`. `build`
    /// produces the coverage raster only when `key` differs from the current
    /// label, so a camera orbit skips the rasterise. A `build` yielding `None`,
    /// or a refused upload, leaves the plate without a label and therefore
    /// without its tab.
    pub(crate) fn update_front_label(
        &mut self,
        key: u64,
        build: &mut dyn FnMut() -> Option<LabelRaster>,
    ) -> Result<(), RenderError> {
        if self.pipelines.label_state.key() == Some(key) {
            return Ok(());
        }
        if self.core.device_loss.is_lost() {
            return Err(RenderError::DeviceLost);
        }
        let Some(raster) = build() else {
            self.pipelines.label_state = LabelState::Off;
            return Ok(());
        };
        let tab = FrontTab::around_label(raster.aspect(), raster.height_over_word());
        match self.upload_label_texture(&raster) {
            Ok(bind_group) => {
                self.pipelines.label_state = LabelState::Shown {
                    key,
                    tab,
                    bind_group,
                };
                Ok(())
            }
            Err(err) => {
                self.pipelines.label_state = LabelState::Refused { key };
                Err(err)
            }
        }
    }

    /// Upload a coverage raster as an RG8 texture (coverage, highlight) and
    /// build the cap's group-1 bind group.
    fn upload_cap_texture(&self, raster: &CapRaster) -> Result<wgpu::BindGroup, RenderError> {
        let bytes = raster.texture_bytes();
        scoped(&self.core.device, || {
            self.coverage_bind_group(
                "cap",
                wgpu::TextureFormat::Rg8Unorm,
                &bytes,
                raster.width,
                raster.height,
                &self.pipelines.cap.texture_bind_group_layout,
                &self.pipelines.cap.sampler,
            )
        })
        .map_err(|source| RenderError::DeviceRefused {
            what: RenderAllocation::CapTexture,
            source,
        })
    }

    /// Upload a label raster as an RG8 texture (the word, the printer's name)
    /// and build the label's group-1 bind group.
    fn upload_label_texture(&self, raster: &LabelRaster) -> Result<wgpu::BindGroup, RenderError> {
        scoped(&self.core.device, || {
            self.coverage_bind_group(
                "front label",
                wgpu::TextureFormat::Rg8Unorm,
                &raster.coverage,
                raster.width,
                raster.height,
                &self.pipelines.label.texture_bind_group_layout,
                &self.pipelines.label.sampler,
            )
        })
        .map_err(|source| RenderError::DeviceRefused {
            what: RenderAllocation::LabelTexture,
            source,
        })
    }

    /// Upload a coverage bitmap as an 8-bit-per-channel texture of `format`
    /// (R8 or RG8, channels interleaved) and bind it with `sampler`. Each row
    /// is padded to wgpu's copy alignment, as `write_texture` requires.
    #[allow(clippy::too_many_arguments)]
    fn coverage_bind_group(
        &self,
        name: &str,
        format: wgpu::TextureFormat,
        coverage: &[u8],
        width: u32,
        height: u32,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        let texture = self.core.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("mesh-viewer {name} texture")),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let row_bytes = width * format.block_copy_size(None).unwrap_or(1);
        let padded_bpr = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let mut padded = vec![0u8; (padded_bpr * height) as usize];
        for row in 0..height as usize {
            let src = row * row_bytes as usize;
            let dst = row * padded_bpr as usize;
            padded[dst..dst + row_bytes as usize]
                .copy_from_slice(&coverage[src..src + row_bytes as usize]);
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
                label: Some(&format!("mesh-viewer {name} texture bg")),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
    }

    /// Draws one frame into a texture Slint can show. Returns `Ok(None)` for a
    /// zero-sized viewport. A size over the device's texture ceiling is drawn
    /// at the ceiling with the aspect kept, so the texture can come back
    /// smaller than asked.
    pub fn render(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<wgpu::Texture>, RenderError> {
        self.render_frame(
            scene_render,
            build_volume,
            camera,
            width,
            height,
            FrameLook {
                contents: FrameContents::Workspace,
                overhang_map: self.overhang_map,
                raft_top_mm: self.raft_top_mm,
                supports_shown_up_to_mm: self.supports_shown_up_to_mm,
            },
        )
    }

    /// Paints the models of the frames to come by overhang (`None` turns the
    /// map off). A picture of the print ([`Self::render_preview`]) never
    /// carries it.
    pub fn set_overhang_map(&mut self, map: Option<OverhangMap>) {
        self.overhang_map = map;
    }

    /// The support tips the overhang map counts as holding the faces around
    /// them (#484). The same tips as last time upload nothing.
    pub fn set_support_tips(&mut self, tips: &[SupportTip]) {
        if self.support_tips == tips {
            return;
        }
        self.support_tips = tips.to_vec();
        let grid = tip_coverage::CoverageGrid::build(tips);
        self.tip_cell_mm = grid.as_ref().map_or(0.0, |g| g.cell_mm);
        self.pipelines
            .mesh
            .set_tip_coverage(&self.core.device, &self.core.queue, grid.as_ref());
    }

    /// How high the rafts of the scene reach, mm (0 without one): what of
    /// the support meshes fades with the plate when the camera goes under
    /// it (#481).
    pub fn set_raft_top(&mut self, mm: f32) {
        self.raft_top_mm = mm.max(0.0);
    }

    /// From under the plate, the supports drawn as lines (#493): the height
    /// above which the support meshes fade with the plate (`f32::MAX` keeps
    /// them, below 0 fades them whole), and the lines drawn in their place
    /// (`None` none). The same `Arc` as last time uploads nothing.
    pub fn set_support_lines(
        &mut self,
        shown_up_to_mm: f32,
        lines: Option<&std::sync::Arc<SupportLines>>,
    ) {
        self.supports_shown_up_to_mm = shown_up_to_mm;
        self.support_lines.install(
            &self.core.device,
            &self.core.queue,
            &self.pipelines.mesh.models_bind_group_layout,
            lines,
        );
    }

    /// Blocks until the GPU has finished the work submitted so far. A poll
    /// error (a lost device) returns at once; the next frame reports it.
    pub fn wait_for_gpu(&self) {
        let _ = self.core.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Draws the models alone against the clear colour, for a picture of the
    /// print rather than of the workspace: no plate, no envelope, no slice cap,
    /// and none of the editing overlays.
    ///
    /// `camera` frames the whole model, so a caller wanting the printed object
    /// in view builds one from the scene's bounds rather than passing the
    /// viewport's.
    pub fn render_preview(
        &mut self,
        graph: &Scene,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<wgpu::Texture>, RenderError> {
        let scene_render = SceneRender {
            graph,
            selection: &[],
            gizmo: None,
            clip_z: None,
        };
        self.render_frame(
            Some(scene_render),
            build_volume,
            camera,
            width,
            height,
            FrameLook {
                contents: FrameContents::ModelsOnly,
                overhang_map: None,
                raft_top_mm: 0.0,
                supports_shown_up_to_mm: f32::MAX,
            },
        )
    }

    /// Draw the models alone and copy the frame off the GPU, for a picture that
    /// is not going on screen. `Ok(None)` for a zero-sized request.
    ///
    /// The frame can come back smaller than asked when the size exceeds what
    /// the device can hold, so the caller checks the size it got.
    ///
    /// # Errors
    ///
    /// [`RenderError`] when the frame cannot be drawn, or when the device
    /// refuses or never finishes the copy back.
    pub fn preview_pixels(
        &mut self,
        graph: &Scene,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<FramePixels>, RenderError> {
        let Some(texture) = self.render_preview(graph, build_volume, camera, width, height)? else {
            return Ok(None);
        };
        readback::read_target_rgba(&self.core.device, &self.core.queue, &texture).map(Some)
    }

    /// Draws a workspace frame and copies it off the GPU, for a test that
    /// looks at what the viewport shows.
    #[cfg(any(test, feature = "test-support"))]
    pub fn workspace_pixels_for_test(
        &mut self,
        scene_render: SceneRender<'_>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Option<FramePixels>, RenderError> {
        let Some(texture) = self.render(Some(scene_render), build_volume, camera, width, height)?
        else {
            return Ok(None);
        };
        readback::read_target_rgba(&self.core.device, &self.core.queue, &texture).map(Some)
    }

    fn render_frame(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
        look: FrameLook,
    ) -> Result<Option<wgpu::Texture>, RenderError> {
        let FrameLook {
            contents,
            overhang_map,
            raft_top_mm,
            supports_shown_up_to_mm,
        } = look;
        let tip_cell_mm = self.tip_cell_mm;
        if self.core.device_loss.is_lost() {
            return Err(RenderError::DeviceLost);
        }
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let (width, height) = clamp_render_size(width, height, self.core.max_texture_side);

        let (graph, selection, gizmo, clip_z) = match scene_render {
            Some(sr) => (Some(sr.graph), sr.selection, sr.gizmo, sr.clip_z),
            None => (None, &[][..], None, None),
        };

        self.ensure_msaa(width, height)?;
        self.ensure_targets(width, height)?;
        let plate_buffers =
            self.ensure_plate_buffers(build_volume, self.pipelines.label_state.tab())?;
        let plate_buffers = &plate_buffers;
        let installed = self.scene.as_ref();
        let msaa = self.target_manager.msaa.as_ref().unwrap();

        // Per-frame upload of model matrices when a `GpuScene` is installed AND
        // the caller passed the matching CPU `Scene`. The CPU graph is the
        // source of truth; the GPU uniform buffer is a mirror that lives long
        // enough to be sampled across the next draw. The renderer's `GpuScene`
        // must match the graph's reachable-leaf count 1:1; a mismatch means the
        // live graph has drifted from the install-time snapshot, and writing 1
        // entry per live leaf would step past the model buffer's end. Skip the
        // mesh path for this frame and wait for the next install to restore the
        // invariant. A graph that fails the leaf walk (dangling node id /
        // dangling subtree resource id) skips the mesh path on the same
        // grounds: the partial entry list would leave the model buffer's tail
        // slots holding stale matrices that the draw loop would render against
        // the current frame's geometry.
        // Which objects are support meshes, for the cuts their hidden rafts
        // leave (#481).
        let mut support_objects: Vec<bool> = Vec::new();
        let scene_for_draws = if let (Some(inst), Some(graph_data)) = (installed, graph) {
            match build_model_entries(
                graph_data,
                selection,
                build_volume,
                &mut self.world_boxes,
                overhang_map.is_some(),
            ) {
                Ok(entries) if entries.len() == inst.gpu.object_count() => {
                    // Each entry lives at `i * model_stride`. The stride is at
                    // least `MODEL_ENTRY_SIZE` and a multiple of the device's
                    // uniform offset alignment (typically 256 bytes), so
                    // consecutive entries are not contiguous and must be
                    // written individually.
                    support_objects = entries.iter().map(|e| e.is_supports()).collect();
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

        // Framing AABB drives `compute_view_proj`'s near/far multipliers. With
        // a scene installed, union the scene's world AABB with the plate
        // envelope so the plate stays inside the depth range; without a scene,
        // the envelope alone is the framing AABB.
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

        // Per-frame envelope-wall warning mask: which side walls (and the top)
        // some leaf breaches. Recomputed each frame from the CPU `Scene`, from
        // the same cached world boxes as the per-leaf `flags.y` bit above, so
        // the two signals stay in lockstep and no vertex is walked twice. `None`
        // for an empty graph leaves the mask at 0 so all wall fragments are
        // discarded. A leaf-walk failure on the same graph already caused
        // `scene_for_draws` to fall to false above, so falling back to 0 here
        // keeps the wall-warning state consistent with the skipped mesh draws.
        let face_breach_mask = graph
            .and_then(|g| build_envelope_warning_mask(g, build_volume, &mut self.world_boxes).ok())
            .unwrap_or(0);
        // From under the plate, the plate and the rafts fade out (#481); a
        // picture of the print has neither.
        let under_plate = if contents == FrameContents::Workspace {
            crate::under_plate::under_plate_fade(camera)
        } else {
            0.0
        };
        let uniforms = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: frame::pack_light_dir(clip_z),
            env_min: [env_min.x, env_min.y, env_min.z, under_plate],
            env_max: [
                env_max.x,
                env_max.y,
                env_max.z,
                crate::under_plate::hidden_below_mm(raft_top_mm),
            ],
            face_breach_mask: [face_breach_mask, 0, 0, 0],
            overhang: overhang_map.map_or([0.0; 4], |map| {
                let [limit, radians, contact, _] = map.uniform(env_min.z);
                [limit, radians, contact, tip_cell_mm]
            }),
            under_plate: [
                if under_plate > 0.0 {
                    supports_shown_up_to_mm
                } else {
                    f32::MAX
                },
                support_lines::LINE_HALF_WIDTH_PX,
                width as f32,
                height as f32,
            ],
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
        // The last pass that draws into the multisampled target resolves it
        // into `target`: the main pass, then the selection outline, then the
        // gizmo, each when it runs.
        let outline = !selection.is_empty() && scene_for_draws;
        let resolve_main = !outline && gizmo.is_none();
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh-viewer pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa.color_view,
                    resolve_target: resolve_main.then_some(&target_view),
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

            if contents == FrameContents::Workspace {
                self.record_plate_solid(&mut pass, plate_buffers);
                self.record_plate_top(&mut pass, plate_buffers);
                self.record_cage(&mut pass, plate_buffers);
                self.record_front_label(&mut pass, plate_buffers, LABEL_TOP);
            }

            if scene_for_draws && let Some(inst) = installed {
                self.record_mesh_objects(&mut pass, inst, selection);
                if under_plate > 0.0 && raft_top_mm > 0.0 {
                    self.record_support_cuts(&mut pass, inst, selection, &support_objects);
                }
                if under_plate > 0.0 && !self.support_lines.is_empty() {
                    self.support_lines.record(
                        &mut pass,
                        &self.pipelines.mesh.line_pipeline,
                        &self.pipelines.mesh.ball_pipeline,
                        &self.pipelines.mesh.uniforms_bind_group,
                    );
                }
                if contents == FrameContents::Workspace {
                    self.surface_paint
                        .record(&mut pass, &self.pipelines.surface_paint, inst);
                }
            }
            if contents == FrameContents::Workspace {
                self.overlay.record(
                    &mut pass,
                    &self.pipelines.mesh.overlay_pipeline,
                    &self.pipelines.mesh.uniforms_bind_group,
                );
            }

            if contents == FrameContents::Workspace {
                self.record_cap(&mut pass);
                self.record_envelope_walls(&mut pass, plate_buffers);
                self.record_plate_bottom(&mut pass, plate_buffers);
                // Over the translucent bottom, so it reads clear from below.
                self.record_front_label(&mut pass, plate_buffers, LABEL_UNDERSIDE);
            }
        }
        if outline {
            let resolve = gizmo.is_none().then_some(&target_view);
            self.record_outline_post_pass(&mut encoder, resolve, msaa);
        }
        if let Some(frame) = gizmo {
            self.record_gizmo_pass(&mut encoder, &target_view, msaa, &frame, view_proj, camera);
        }
        self.core.queue.submit(std::iter::once(encoder.finish()));

        Ok(Some(target))
    }

    /// Plate solid (sides + bottom): drawn first with depth-write on so the
    /// slab's interior occludes content behind it when the camera orbits low.
    /// Stencil reference stays at 0 so the outline post-pass leaves the plate
    /// untouched.
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

    /// Slice-view cap: a flat quad at the clip height textured with the current
    /// layer's coverage, filling the mesh cut so it reads solid. Drawn only
    /// when a cap texture is set (Slice section with a held slice). The clip
    /// height and build corners come from the shared uniform buffer, so no
    /// per-cap vertex data is bound.
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

    /// Plate top: inner grid + outer rounded border at Z = 0. Depth-write off
    /// so a model resting at Z = 0 always wins z-fighting against the plate
    /// top. Relies on the vertex buffer and uniforms bind group set by
    /// [`Self::record_plate_solid`]; must be called after it within the same
    /// render pass.
    fn record_plate_top(&self, pass: &mut wgpu::RenderPass<'_>, plate_buffers: &PlateBuffers) {
        pass.set_pipeline(&self.pipelines.plate.top_pipeline);
        pass.set_index_buffer(
            plate_buffers.top_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..plate_buffers.top_index_count, 0, 0..1);
    }

    /// Cage: 12 wireframe edges sharing the plate vertex buffer; the line-list
    /// index buffer picks the cage corner range. Relies on the vertex buffer
    /// and uniforms bind group set by [`Self::record_plate_solid`].
    fn record_cage(&self, pass: &mut wgpu::RenderPass<'_>, plate_buffers: &PlateBuffers) {
        pass.set_pipeline(&self.pipelines.plate.line_pipeline);
        pass.set_index_buffer(
            plate_buffers.cage_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..plate_buffers.cage_index_count, 0, 0..1);
    }

    /// The word on the plate's front tab: `vertices` picks the copy over the
    /// tab's top face ([`LABEL_TOP`]) or the one under it
    /// ([`LABEL_UNDERSIDE`]). Binds its own vertex buffer at slot 0, so the
    /// plate draws that share the plate's must come before it, or bind theirs
    /// again.
    fn record_front_label(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        plate_buffers: &PlateBuffers,
        vertices: std::ops::Range<u32>,
    ) {
        let LabelState::Shown { bind_group, .. } = &self.pipelines.label_state else {
            return;
        };
        let Some(quad) = &plate_buffers.label_vertex_buffer else {
            return;
        };
        pass.set_pipeline(&self.pipelines.label.pipeline);
        pass.set_bind_group(0, &self.pipelines.label.uniforms_bind_group, &[]);
        pass.set_bind_group(1, bind_group, &[]);
        pass.set_vertex_buffer(0, quad.slice(..));
        pass.set_stencil_reference(0);
        pass.draw(vertices, 0..1);
    }

    /// 1 draw call per installed scene object. The models bind group is rebound
    /// at each step with the dynamic offset of the matching ModelEntry slot, so
    /// the vertex shader's `model_entry` uniform reads that object's matrix.
    /// The stencil reference is `1` only for the selected object, so after this
    /// pass the stencil holds 1 over its visible silhouette and 0 everywhere
    /// else, what the outline post-pass dilates.
    fn record_mesh_objects(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        inst: &InstalledScene,
        selection: &[NodeId],
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
            pass.set_stencil_reference(if selection.binary_search(&obj.node_id()).is_ok() {
                1
            } else {
                0
            });
            pass.draw_indexed(0..obj.index_count(), 0, 0..1);
        }
    }

    /// The cuts the hidden rafts leave (#481): the support meshes again, with
    /// their back faces only, through the cap pipeline. A back face shows
    /// only where the front faces before it were dropped with the raft, so
    /// this fills each opened shaft with the flat face of its cut. Same
    /// stencil as the object, so a selected set's outline takes its cuts in.
    fn record_support_cuts(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        inst: &InstalledScene,
        selection: &[NodeId],
        support_objects: &[bool],
    ) {
        pass.set_pipeline(&self.pipelines.mesh.cap_pipeline);
        pass.set_bind_group(
            MeshBindGroup::Uniforms.idx(),
            &self.pipelines.mesh.uniforms_bind_group,
            &[],
        );
        for (i, obj) in inst.gpu.objects().iter().enumerate() {
            if !support_objects.get(i).copied().unwrap_or(false) {
                continue;
            }
            let offset = (i as u64 * inst.gpu.model_stride()) as u32;
            pass.set_bind_group(
                MeshBindGroup::Models.idx(),
                &inst.models_bind_group,
                &[offset],
            );
            pass.set_vertex_buffer(0, obj.vertex_buffer().slice(..));
            pass.set_index_buffer(obj.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
            pass.set_stencil_reference(u32::from(selection.binary_search(&obj.node_id()).is_ok()));
            pass.draw_indexed(0..obj.index_count(), 0, 0..1);
        }
    }

    /// Envelope-warning walls: 5 invisible quads coplanar with the build-volume
    /// side faces and top. The plate shader discards a wall fragment unless the
    /// matching bit of `face_breach_mask` is set; on a set bit it emits the
    /// diagonal red stripe pattern. Drawn after the mesh pass so the mesh's
    /// depth-write gates wall visibility: stripes appear over the part of the
    /// mesh that lies on the far side of the envelope wall from the camera,
    /// while closer-to-camera mesh pixels occlude the wall as they should.
    /// `top_pipeline` matches the needs here (TriangleList, depth-test Less,
    /// depth-write off, cull none, alpha blending). Stencil stays at 0.
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

    /// Plate bottom: drawn last in the main pass with alpha blending and
    /// back-face culling. Cage edges and any meshes are already in the
    /// framebuffer, so the bottom blends over them when the camera looks up
    /// from below; from above the bottom is back-facing and culled by the
    /// pipeline, leaving the top face fully opaque. The mesh loop earlier in
    /// the pass left the plate vertex buffer unbound, so rebind here. Stencil
    /// reset to 0 so the outline post-pass leaves the bottom untouched.
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

    /// Outline post-pass: dilate the stencil set by the mesh pass to draw a
    /// 1-pixel halo around the selected leaf. Caller must gate on
    /// `!selection.is_empty() && scene_for_draws`: without a scene the stencil is
    /// all zeroes and the dilate would be a no-op; without a selection the mesh
    /// pass left the stencil at 0 too. Loads the multisampled colour from the
    /// main pass and discards non-edge fragments, so existing pixels survive;
    /// resolves into `resolve` when no gizmo pass follows.
    fn record_outline_post_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        resolve: Option<&wgpu::TextureView>,
        msaa: &MsaaTargets,
    ) {
        let mut outline_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh-viewer outline pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa.color_view,
                resolve_target: resolve,
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

    /// Gizmo pass: drawn last, into the multisampled colour target so the
    /// handles are antialiased, over the mesh and the selection outline, and
    /// resolved into `target_view`. It borrows the main depth attachment with
    /// its depth cleared, so the gizmo is never hidden by the model while the
    /// handles still order among themselves. Uploads the per-frame uniforms,
    /// the draw entries (one upload) and, while rotating, the marks of the
    /// turn, then records the draws in the order `build_gizmo_draws` gives.
    fn record_gizmo_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        msaa: &MsaaTargets,
        frame: &GizmoFrame,
        view_proj: Mat4,
        camera: &OrbitCamera,
    ) {
        let buffers = &self.pipelines.gizmo_buffers;
        let eye = if camera.orthographic {
            let toward = camera.eye() - camera.target;
            [toward.x, toward.y, toward.z, 0.0]
        } else {
            let e = camera.eye();
            [e.x, e.y, e.z, 1.0]
        };
        let uniforms = GizmoUniformsGpu {
            view_proj: view_proj.to_cols_array_2d(),
            pivot_and_scale: [frame.pivot.x, frame.pivot.y, frame.pivot.z, frame.scale],
            eye,
        };
        self.core
            .queue
            .write_buffer(&buffers.uniforms_buffer, 0, bytemuck::bytes_of(&uniforms));

        let draws = build_gizmo_draws(frame);
        let entries: Vec<GizmoDrawEntry> = draws.iter().map(|d| d.entry).collect();
        buffers.write_entries(&self.core.queue, &entries);
        let ring_radius = frame.look.metrics().ring_radius;
        let (wedge, marks) = match frame.drag_visual {
            Some(GizmoDragVisual::Rotate { axis, start, sweep }) => buffers.write_marks(
                &self.core.queue,
                &crate::gizmo_geometry::build_rotate_marks(axis, start, sweep, ring_radius),
            ),
            Some(GizmoDragVisual::ViewRotate {
                right,
                up,
                start,
                sweep,
            }) => buffers.write_marks(
                &self.core.queue,
                &crate::gizmo_geometry::build_ring_marks(right, up, start, sweep, ring_radius),
            ),
            _ => (0..0, 0..0),
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh-viewer gizmo pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa.color_view,
                resolve_target: Some(target_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &msaa.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Discard,
                }),
            }),
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pass.set_bind_group(
            GizmoBindGroup::Uniforms.idx(),
            &self.pipelines.gizmo.uniforms_bind_group,
            &[],
        );
        let mut bound_layer = None;
        let mut bound_mesh = None;
        for (slot, draw) in draws.iter().enumerate().take(GIZMO_ENTRY_SLOTS as usize) {
            if bound_layer != Some(draw.layer) {
                pass.set_pipeline(match draw.layer {
                    GizmoLayer::Halo => &self.pipelines.gizmo.halo_pipeline,
                    GizmoLayer::Core => &self.pipelines.gizmo.core_pipeline,
                    GizmoLayer::Flat => &self.pipelines.gizmo.flat_pipeline,
                });
                bound_layer = Some(draw.layer);
            }
            let offset = (slot as u64 * buffers.entry_stride) as u32;
            pass.set_bind_group(
                GizmoBindGroup::Entries.idx(),
                &self.pipelines.gizmo.entries_bind_group,
                &[offset],
            );
            if let Some(mesh) = buffers.mesh(draw.mesh, frame.look.style) {
                if bound_mesh != Some(draw.mesh) {
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    bound_mesh = Some(draw.mesh);
                }
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                continue;
            }
            let range = if draw.mesh == GizmoMesh::Wedge {
                wedge.clone()
            } else {
                marks.clone()
            };
            if !range.is_empty() {
                pass.set_vertex_buffer(0, buffers.marks_buffer.slice(..));
                bound_mesh = None;
                pass.draw(range, 0..1);
            }
        }
    }
}
