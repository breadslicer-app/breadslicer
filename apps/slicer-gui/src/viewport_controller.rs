/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use glam::Vec3;
use sc_mesh_core::scene::Scene;
use sc_mesh_formats::LoadedMesh;
use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning};
use slint::ComponentHandle;
use slint::wgpu_28::wgpu;

use crate::MainWindow;
use crate::camera::{AxisView, OrbitCamera};
use crate::error_dialog::{DialogManager, format_anyhow_chain};
use crate::i18n::ctr;
use crate::mesh_render::{Renderer, default_fov_y};

pub type LoadSlot = Arc<Mutex<Option<(u64, PathBuf, anyhow::Result<LoadedMesh>)>>>;

pub struct ViewportController {
    ui: slint::Weak<MainWindow>,
    renderer: RefCell<Option<Renderer>>,
    scene: RefCell<Option<Scene>>,
    // The camera lives next to the scene (not inside the GPU renderer), so it
    // survives device-loss / RenderingTeardown→Setup cycles for free and can
    // be manipulated without a GPU present.
    camera: RefCell<Option<OrbitCamera>>,
    viewport: Cell<(f32, f32)>,
    dirty: Cell<bool>,
    needs_upload: Cell<bool>,
    load_gen: Cell<u64>,
    load_slot: LoadSlot,
    // Number of load operations that have begun but not yet called
    // `end_load`. `loading` on the UI stays true while this is > 0, so a
    // stale worker that finishes after a Close cannot prematurely re-enable
    // Open (the case behind B1). In practice the count is always 0 or 1,
    // because the open gate rejects a new load while any is in flight.
    inflight: Cell<u32>,
    // True between a `viewport-resized` event and the debounce timer firing
    // in `on_resize_settled`. While set, `handle_before_rendering` rounds the
    // render target size up to the next `RESIZE_BUCKET` multiple so drag
    // resizes don't churn the MSAA/target allocations on every pointer move.
    resizing: Cell<bool>,
    // Consecutive upload failures for the currently-loaded scene. `needs_upload`
    // stays true after a failure (so a new renderer or a fresh scene can still
    // drive a retry); this counter is what gates dialog-spam — the error dialog
    // only fires on the transition from 0 → 1 failures. Reset on successful
    // upload, scene load/close, and on `RenderingSetup`.
    upload_failures: Cell<u32>,
    dialogs: Rc<DialogManager>,
}

// Granularity for the allocation bucketing applied during active resize.
// A smaller value reduces the aspect distortion that `image-fit: fill` would
// apply while dragging (texture sized at bucket, stretched to logical); a
// larger value reduces reallocation rate. 64 px keeps the worst-case stretch
// under ~3% on a 1080p window, which is barely perceptible during a drag.
const RESIZE_BUCKET: u32 = 64;

// How long `viewport-resized` must stay quiet before we snap back to exact
// pixel sizing.
pub(crate) const RESIZE_SETTLE: core::time::Duration = core::time::Duration::from_millis(200);

impl ViewportController {
    pub fn new(ui: &MainWindow) -> Self {
        Self {
            ui: ui.as_weak(),
            renderer: RefCell::new(None),
            scene: RefCell::new(None),
            camera: RefCell::new(None),
            viewport: Cell::new((0.0, 0.0)),
            dirty: Cell::new(false),
            needs_upload: Cell::new(false),
            load_gen: Cell::new(0),
            load_slot: Arc::new(Mutex::new(None)),
            inflight: Cell::new(0),
            resizing: Cell::new(false),
            upload_failures: Cell::new(0),
            dialogs: DialogManager::new(),
        }
    }

    // Exposed so the file-loader worker thread (which can't hold an Rc) can
    // deposit results and tag them with a generation stamp.
    pub fn load_slot(&self) -> LoadSlot {
        self.load_slot.clone()
    }

    pub fn next_load_stamp(&self) -> u64 {
        let stamp = self.load_gen.get().wrapping_add(1);
        self.load_gen.set(stamp);
        stamp
    }

    /// Starts a new load. Returns `false` if one is already in flight and
    /// the caller must abort. Pair every successful call with exactly one
    /// `end_load` (invoked from `on_scene_load_finished`, or manually on
    /// paths that bail before a worker gets spawned).
    pub fn begin_load(&self) -> bool {
        if self.inflight.get() > 0 {
            return false;
        }
        self.inflight.set(self.inflight.get() + 1);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_loading(true);
        }
        true
    }

    pub fn end_load(&self) {
        let remaining = self.inflight.get().saturating_sub(1);
        self.inflight.set(remaining);
        if remaining == 0
            && let Some(ui) = self.ui.upgrade()
        {
            ui.set_loading(false);
        }
    }

    fn request_redraw(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.window().request_redraw();
        }
    }

    fn mark_dirty(&self) {
        self.dirty.set(true);
        self.request_redraw();
    }

    pub fn on_viewport_resized(&self, w: f32, h: f32) {
        self.viewport.set((w, h));
        self.resizing.set(true);
        self.mark_dirty();
    }

    /// Called once `viewport-resized` has stayed quiet for `RESIZE_SETTLE`.
    /// Drops the bucketed MSAA / target allocations so the next frame
    /// reallocates at the exact logical size, eliminating the `image-fit:
    /// fill` stretch that persisted during the drag.
    pub fn on_resize_settled(&self) {
        self.resizing.set(false);
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.invalidate_sizing();
        }
        self.mark_dirty();
    }

    pub fn on_rotate(&self, dx: f32, dy: f32) {
        let (_, h) = self.viewport.get();
        if let Some(cam) = self.camera.borrow_mut().as_mut() {
            cam.orbit(dx, dy, h);
            self.mark_dirty();
        }
    }

    pub fn on_pan(&self, dx: f32, dy: f32) {
        let (_, h) = self.viewport.get();
        if let Some(cam) = self.camera.borrow_mut().as_mut() {
            cam.pan(dx, dy, default_fov_y(), h);
            self.mark_dirty();
        }
    }

    pub fn on_zoom(&self, delta: f32, cursor_x: f32, cursor_y: f32) {
        let (w, h) = self.viewport.get();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let factor = (-delta * 0.005).exp().clamp(0.1, 10.0);
        if let Some(cam) = self.camera.borrow_mut().as_mut() {
            cam.zoom_at(factor, cursor_x, cursor_y, default_fov_y(), w, h);
            self.mark_dirty();
        }
    }

    pub fn on_frame_view(&self) {
        if let Some(cam) = self.camera.borrow_mut().as_mut() {
            cam.frame_scene();
            self.mark_dirty();
        }
    }

    pub fn on_axis_view(&self, view: crate::AxisView) {
        let axis = axis_from_ui(view);
        if let Some(cam) = self.camera.borrow_mut().as_mut() {
            cam.set_axis_view(axis);
            self.mark_dirty();
        }
    }

    pub fn on_close_scene(&self) {
        // Invalidate any in-flight load so its result is dropped on arrival.
        self.load_gen.set(self.load_gen.get().wrapping_add(1));
        // Drop any already-deposited result now rather than waiting for the
        // queued `scene_load_finished` handler to drain it. Frees the
        // `LoadedMesh` (which can be large) immediately.
        *self
            .load_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self.scene.borrow_mut() = None;
        *self.camera.borrow_mut() = None;
        self.needs_upload.set(false);
        self.upload_failures.set(0);
        self.dirty.set(false);
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.clear_scene();
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_mesh_rendering(slint::Image::default());
            ui.set_has_scene(false);
        }
    }

    pub fn on_scene_load_finished(&self) {
        let taken = self
            .load_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.end_load();
        let Some((stamp, path, result)) = taken else {
            return;
        };
        if stamp != self.load_gen.get() {
            // The user cancelled (Close) or something else bumped the
            // generation while this worker was parsing. Drop the result.
            return;
        }
        match result {
            Ok(LoadedMesh { scene, warnings }) => {
                log_load_warnings(&path, &warnings);
                // Build a fresh camera from the scene's bounds. If the scene
                // has no geometry, leave camera = None; the subsequent GPU
                // upload will surface the empty-scene error.
                *self.camera.borrow_mut() = camera_for_scene(&scene);
                *self.scene.borrow_mut() = Some(scene);
                self.needs_upload.set(true);
                self.upload_failures.set(0);
                if let Some(ui) = self.ui.upgrade() {
                    ui.set_has_scene(true);
                }
                self.mark_dirty();
            }
            Err(err) => self.dialogs.show_error(
                ctr!("ErrorDialog", "Failed to open 3D model").as_str(),
                Some(ctr!("ErrorDialog", "File: {}", path.display()).to_string()),
                err.to_string(),
                format_anyhow_chain(&err),
            ),
        }
    }

    pub fn handle_rendering_setup(&self, device: wgpu::Device, queue: wgpu::Queue) {
        *self.renderer.borrow_mut() = Some(Renderer::new(device, queue));
        self.upload_failures.set(0);
        if self.scene.borrow().is_some() {
            self.needs_upload.set(true);
            self.request_redraw();
        }
    }

    // Release GPU-owning state before the graphics context goes away. The
    // scene (CPU-side) and the camera stay in the controller, so the next
    // `RenderingSetup` can re-upload without losing the user's view —
    // `handle_rendering_setup` flips `needs_upload` on its own when a scene
    // is present, so we don't set it here.
    pub fn handle_rendering_teardown(&self) {
        *self.renderer.borrow_mut() = None;
        self.dirty.set(false);
    }

    pub fn handle_before_rendering(&self) {
        let mut renderer_slot = self.renderer.borrow_mut();
        let Some(renderer) = renderer_slot.as_mut() else {
            return;
        };

        if self.needs_upload.get() {
            if let Some(scene_ref) = self.scene.borrow().as_ref() {
                match renderer.upload_scene(scene_ref) {
                    Ok(()) => {
                        self.needs_upload.set(false);
                        self.upload_failures.set(0);
                        self.dirty.set(true);
                    }
                    Err(err) => {
                        // Leave `needs_upload = true` so the flag continues
                        // to represent "upload owed" — a subsequent renderer
                        // setup or scene reload (both of which reset
                        // `upload_failures`) will drive a retry without
                        // relying on coincidental side effects. The counter
                        // gates dialog-spam: the error fires only on the
                        // 0 → 1 transition.
                        let prior = self.upload_failures.get();
                        self.upload_failures.set(prior.saturating_add(1));
                        if prior == 0 {
                            self.dialogs.show_error_deferred(
                                ctr!("ErrorDialog", "Failed to load the 3D model").to_string(),
                                Some(
                                    ctr!(
                                        "ErrorDialog",
                                        "The mesh could not be prepared for rendering."
                                    )
                                    .to_string(),
                                ),
                                err.to_string(),
                                format_anyhow_chain(&err),
                            );
                        }
                        return;
                    }
                }
            } else {
                // Flag set but the scene was cleared before we got to it.
                self.needs_upload.set(false);
            }
        }

        if !self.dirty.get() {
            return;
        }

        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (w_logical, h_logical) = self.viewport.get();
        let scale = ui.window().scale_factor();
        let width = (w_logical * scale).round() as u32;
        let height = (h_logical * scale).round() as u32;
        if width == 0 || height == 0 {
            return;
        }

        let (eff_w, eff_h) = if self.resizing.get() {
            (
                width.div_ceil(RESIZE_BUCKET) * RESIZE_BUCKET,
                height.div_ceil(RESIZE_BUCKET) * RESIZE_BUCKET,
            )
        } else {
            (width, height)
        };

        let camera_slot = self.camera.borrow();
        let Some(camera) = camera_slot.as_ref() else {
            return;
        };

        let Some(texture) = renderer.render(camera, eff_w, eff_h) else {
            return;
        };
        self.dirty.set(false);
        match slint::Image::try_from(texture) {
            Ok(image) => ui.set_mesh_rendering(image),
            Err(err) => self.dialogs.show_error_deferred(
                ctr!("ErrorDialog", "Failed to display the 3D model").to_string(),
                Some(
                    ctr!(
                        "ErrorDialog",
                        "The rendered texture could not be handed to the UI."
                    )
                    .to_string(),
                ),
                err.to_string(),
                format!("{err:?}"),
            ),
        }
    }
}

// Builds an `OrbitCamera` framed on the scene's bounding box. Returns `None`
// for empty scenes or scenes with dangling `Resource` references; the later
// GPU upload surfaces the underlying problem with a translated error dialog.
fn camera_for_scene(scene: &Scene) -> Option<OrbitCamera> {
    let (min, max) = scene.aabb().ok()??;
    let min = Vec3::from(min.0);
    let max = Vec3::from(max.0);
    let center = (min + max) * 0.5;
    let radius = ((max - min).length() * 0.5).max(1e-3);
    Some(OrbitCamera::new(center, radius))
}

// Load-time warnings (currently OBJ-only) are surfaced to stderr until the
// UI grows a place to show them. Unifying logging is tracked as ARCH6.
fn log_load_warnings(path: &std::path::Path, warnings: &[LoadMeshWarning]) {
    for warning in warnings {
        match warning {
            LoadMeshWarning::Obj(w) => {
                eprintln!("WARNING [{}]: {}", path.display(), describe_obj_warning(w));
            }
        }
    }
}

fn describe_obj_warning(warning: &LoadObjWarning) -> String {
    match warning {
        LoadObjWarning::NoVertices => "OBJ file has no vertices".to_string(),
        LoadObjWarning::NoFaces => "OBJ file has no faces".to_string(),
        LoadObjWarning::SkippedTextureCoordinates { line } => {
            format!("line {line}: skipped texture coordinates")
        }
        LoadObjWarning::SkippedVertexNormal { line } => {
            format!("line {line}: skipped vertex normal")
        }
        LoadObjWarning::SkippedGroup { line, name } => {
            format!("line {line}: skipped group '{name}'")
        }
        LoadObjWarning::SkippedSmoothingGroup { line } => {
            format!("line {line}: skipped smoothing group")
        }
        LoadObjWarning::SkippedMaterialReference { line, material } => {
            format!("line {line}: skipped material reference '{material}'")
        }
        LoadObjWarning::SkippedMaterialLibrary { line, path } => {
            format!("line {line}: skipped material library '{path}'")
        }
        LoadObjWarning::SkippedComment { line } => format!("line {line}: skipped comment"),
        LoadObjWarning::SharedVertices {
            object_name,
            shared_indices,
        } => match object_name {
            Some(name) => format!(
                "object '{name}' shares global vertex indices with previously loaded objects: {shared_indices:?}"
            ),
            None => format!(
                "an unnamed object shares global vertex indices with previously loaded objects: {shared_indices:?}"
            ),
        },
    }
}

fn axis_from_ui(view: crate::AxisView) -> AxisView {
    match view {
        crate::AxisView::Front => AxisView::Front,
        crate::AxisView::Back => AxisView::Back,
        crate::AxisView::Right => AxisView::Right,
        crate::AxisView::Left => AxisView::Left,
        crate::AxisView::Top => AxisView::Top,
        crate::AxisView::Bottom => AxisView::Bottom,
    }
}
