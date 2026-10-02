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
use crate::gizmo::axis::ALL_AXES;
use crate::slice_cap::CapRaster;

mod device_loss;
mod device_ops;
mod error;
mod frame;
mod gizmo_buffers;
mod gizmo_entries;
#[cfg(test)]
pub(crate) mod mock;
mod pipelines;
mod readback;
mod render_size;
mod renderer_trait;
pub mod scene_upload;
mod targets;
#[cfg(test)]
mod tests_gpu;

pub use device_loss::DeviceLoss;
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
use pipelines::label::LabelPipeline;
use pipelines::mesh::{MeshBindGroup, MeshPipeline};
use pipelines::outline::{OutlineBindGroup, OutlinePipeline};
use pipelines::plate::{FrontTab, PlateBindGroup, PlateBuffers, PlatePipeline};
pub use readback::FramePixels;
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
#[derive(Clone, Copy, Debug)]
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

/// What a frame draws around the models.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FrameContents {
    /// The machine's workspace: the plate, its cage and envelope, and
    /// the slice cap when one is installed.
    Workspace,
    /// The models by themselves.
    ModelsOnly,
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
        eprintln!("wgpu error (uncaptured): {err}");
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
    device_loss: DeviceLoss,
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
    /// The plate-label pipeline, immutable after construction.
    label: LabelPipeline,
    label_state: LabelState,
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

/// State of the plate label's texture, keyed by the word it was last
/// built for. The bind group keeps the texture and its view alive,
/// and the tab it is drawn on is sized by that same word.
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

    /// Returns the tab carrying the label, `None` when there is no
    /// label to carry: the tab exists for the word.
    fn tab(&self) -> Option<FrontTab> {
        match self {
            Self::Shown { tab, .. } => Some(*tab),
            Self::Off | Self::Refused { .. } => None,
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

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("has_scene", &self.scene.is_some())
            .finish_non_exhaustive()
    }
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
        device_loss: DeviceLoss,
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
        let label = LabelPipeline::create(&device, &mesh).map_err(at(RendererStage::Label))?;
        let max_texture_side = device.limits().max_texture_dimension_2d;

        Ok(Self {
            core: RendererCore {
                device,
                queue,
                device_token,
                device_loss,
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
                label,
                label_state: LabelState::Off,
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
        if self.core.device_loss.is_lost() {
            return Err(scene_upload::DeviceLostDuringUpload.into());
        }
        // Released before the build so the old and the new scene never
        // occupy device memory at the same time.
        self.scene = None;
        // An empty model uniform buffer would fail bind-group
        // validation, so a leafless graph gets no scene at all.
        if renderable_leaf_count(graph)? == 0 {
            self.clear_scene();
            return Ok(());
        }
        let gpu = GpuScene::build(
            &self.core.device,
            self.core.device_token,
            graph,
            &self.core.device_loss,
        )?;
        self.attach_scene(gpu);
        Ok(())
    }

    /// Installs a `GpuScene` built on a worker thread, but only if its
    /// `device_token` matches this renderer's and the device is not
    /// lost. Returns `false` otherwise; the caller falls back to the
    /// recovery upload path.
    pub fn install_scene(&mut self, gpu: GpuScene) -> bool {
        if self.core.device_token != gpu.device_token() || self.core.device_loss.is_lost() {
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
    /// key is the `BuildVolume` plus the front tab's footprint; a
    /// different value triggers a rebuild so a runtime envelope
    /// change reaches the next frame. On `Err` the cache is empty, so
    /// the next frame retries. The returned value clones the cached
    /// wgpu handles (reference counted), so no borrow of `self`
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

    /// Refresh the plate label for the word identified by `key`.
    /// `build` produces the coverage raster only when `key` differs
    /// from the current label, so a camera orbit skips the rasterise.
    /// A `build` yielding `None`, or a refused upload, leaves the
    /// plate without a label and therefore without its tab.
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
        let tab = FrontTab::around_label(raster.aspect());
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

    /// Upload a coverage raster as an R8 texture and build the cap's
    /// group-1 bind group.
    fn upload_cap_texture(&self, raster: &CapRaster) -> Result<wgpu::BindGroup, RenderError> {
        scoped(&self.core.device, || {
            self.coverage_bind_group(
                "cap",
                &raster.coverage,
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

    /// Upload a label raster as an R8 texture and build the label's
    /// group-1 bind group.
    fn upload_label_texture(&self, raster: &LabelRaster) -> Result<wgpu::BindGroup, RenderError> {
        scoped(&self.core.device, || {
            self.coverage_bind_group(
                "front label",
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

    /// Upload a single-channel coverage bitmap as an R8 texture and
    /// bind it with `sampler`. Each row is padded to wgpu's copy
    /// alignment, as `write_texture` requires.
    fn coverage_bind_group(
        &self,
        name: &str,
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
            padded[dst..dst + width as usize].copy_from_slice(&coverage[src..src + width as usize]);
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
        self.render_frame(
            scene_render,
            build_volume,
            camera,
            width,
            height,
            FrameContents::Workspace,
        )
    }

    /// Draws the models alone against the clear colour, for a picture
    /// of the print rather than of the workspace: no plate, no
    /// envelope, no slice cap, and none of the editing overlays.
    ///
    /// `camera` frames the whole model, so a caller wanting the
    /// printed object in view builds one from the scene's bounds
    /// rather than passing the viewport's.
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
            selection: None,
            gizmo: None,
            clip_z: None,
        };
        self.render_frame(
            Some(scene_render),
            build_volume,
            camera,
            width,
            height,
            FrameContents::ModelsOnly,
        )
    }

    /// Draw the models alone and copy the frame off the GPU, for a
    /// picture that is not going on screen. `Ok(None)` for a
    /// zero-sized request.
    ///
    /// The frame can come back smaller than asked when the size
    /// exceeds what the device can hold, so the caller checks the
    /// size it got.
    ///
    /// # Errors
    ///
    /// [`RenderError`] when the frame cannot be drawn, or when the
    /// device refuses or never finishes the copy back.
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

    fn render_frame(
        &mut self,
        scene_render: Option<SceneRender<'_>>,
        build_volume: BuildVolume,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
        contents: FrameContents,
    ) -> Result<Option<wgpu::Texture>, RenderError> {
        if self.core.device_loss.is_lost() {
            return Err(RenderError::DeviceLost);
        }
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
        let plate_buffers =
            self.ensure_plate_buffers(build_volume, self.pipelines.label_state.tab())?;
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

            if contents == FrameContents::Workspace {
                self.record_plate_solid(&mut pass, plate_buffers);
                self.record_plate_top(&mut pass, plate_buffers);
                self.record_cage(&mut pass, plate_buffers);
                self.record_front_label(&mut pass, plate_buffers);
            }

            if scene_for_draws && let Some(inst) = installed {
                self.record_mesh_objects(&mut pass, inst, selection);
            }

            if contents == FrameContents::Workspace {
                self.record_cap(&mut pass);
                self.record_envelope_walls(&mut pass, plate_buffers);
                self.record_plate_bottom(&mut pass, plate_buffers);
            }
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

    /// The word on the plate's front tab, over the tab's own top
    /// face. Binds its own vertex buffer at slot 0, so the plate
    /// draws that share the plate's must come before it.
    fn record_front_label(&self, pass: &mut wgpu::RenderPass<'_>, plate_buffers: &PlateBuffers) {
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
        pass.draw(0..6, 0..1);
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
