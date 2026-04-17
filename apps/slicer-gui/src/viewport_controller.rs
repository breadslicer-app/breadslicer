/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sc_mesh_core::scene::Scene;
use sc_mesh_formats::LoadedMesh;
use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning};
use slint::ComponentHandle;
use slint::wgpu_28::wgpu;

use crate::MainWindow;
use crate::camera::{AxisView, OrbitCamera};
use crate::error_dialog::{format_anyhow_chain, show_error_dialog, show_error_dialog_deferred};
use crate::i18n::ctr;
use crate::mesh_render::Renderer;

pub type LoadSlot = Arc<Mutex<Option<(u64, PathBuf, anyhow::Result<LoadedMesh>)>>>;

pub struct ViewportController {
    ui: slint::Weak<MainWindow>,
    renderer: RefCell<Option<Renderer>>,
    scene: RefCell<Option<Scene>>,
    saved_camera: RefCell<Option<OrbitCamera>>,
    viewport: Cell<(f32, f32)>,
    dirty: Cell<bool>,
    needs_upload: Cell<bool>,
    load_gen: Cell<u64>,
    load_slot: LoadSlot,
}

impl ViewportController {
    pub fn new(ui: &MainWindow) -> Self {
        Self {
            ui: ui.as_weak(),
            renderer: RefCell::new(None),
            scene: RefCell::new(None),
            saved_camera: RefCell::new(None),
            viewport: Cell::new((0.0, 0.0)),
            dirty: Cell::new(false),
            needs_upload: Cell::new(false),
            load_gen: Cell::new(0),
            load_slot: Arc::new(Mutex::new(None)),
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
        self.mark_dirty();
    }

    pub fn on_rotate(&self, dx: f32, dy: f32) {
        let (_, h) = self.viewport.get();
        if let Some(r) = self.renderer.borrow_mut().as_mut()
            && r.rotate(dx, dy, h)
        {
            self.mark_dirty();
        }
    }

    pub fn on_pan(&self, dx: f32, dy: f32) {
        let (_, h) = self.viewport.get();
        if let Some(r) = self.renderer.borrow_mut().as_mut()
            && r.pan(dx, dy, h)
        {
            self.mark_dirty();
        }
    }

    pub fn on_zoom(&self, delta: f32, cursor_x: f32, cursor_y: f32) {
        let (w, h) = self.viewport.get();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let factor = (-delta * 0.005).exp().clamp(0.1, 10.0);
        if let Some(r) = self.renderer.borrow_mut().as_mut()
            && r.zoom_at(factor, cursor_x, cursor_y, w, h)
        {
            self.mark_dirty();
        }
    }

    pub fn on_frame_view(&self) {
        if let Some(r) = self.renderer.borrow_mut().as_mut()
            && r.frame_view()
        {
            self.mark_dirty();
        }
    }

    pub fn on_axis_view(&self, view: crate::AxisView) {
        let axis = axis_from_ui(view);
        if let Some(r) = self.renderer.borrow_mut().as_mut()
            && r.set_axis_view(axis)
        {
            self.mark_dirty();
        }
    }

    pub fn on_close_scene(&self) {
        // Invalidate any in-flight load so its result is dropped on arrival.
        self.load_gen.set(self.load_gen.get().wrapping_add(1));
        *self.scene.borrow_mut() = None;
        *self.saved_camera.borrow_mut() = None;
        self.needs_upload.set(false);
        self.dirty.set(false);
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.clear_scene();
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_mesh_rendering(slint::Image::default());
        }
    }

    pub fn on_scene_load_finished(&self) {
        let taken = self.load_slot.lock().expect("load slot poisoned").take();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_loading(false);
        }
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
                *self.scene.borrow_mut() = Some(scene);
                *self.saved_camera.borrow_mut() = None;
                self.needs_upload.set(true);
                self.mark_dirty();
            }
            Err(err) => show_error_dialog(
                ctr!("ErrorDialog", "Failed to open 3D model").as_str(),
                Some(ctr!("ErrorDialog", "File: {}", path.display()).to_string()),
                err.to_string(),
                format_anyhow_chain(&err),
            ),
        }
    }

    pub fn handle_rendering_setup(&self, device: wgpu::Device, queue: wgpu::Queue) {
        *self.renderer.borrow_mut() = Some(Renderer::new(device, queue));
        if self.scene.borrow().is_some() {
            self.needs_upload.set(true);
            self.request_redraw();
        }
    }

    // Release GPU-owning state before the graphics context goes away.
    // The Scene (CPU-side) and a camera snapshot are kept so the next
    // RenderingSetup can re-upload and restore the user's view.
    pub fn handle_rendering_teardown(&self) {
        let camera_snapshot = self.renderer.borrow().as_ref().and_then(|r| r.camera());
        if camera_snapshot.is_some() {
            *self.saved_camera.borrow_mut() = camera_snapshot;
        }
        *self.renderer.borrow_mut() = None;
        self.dirty.set(false);
        if self.scene.borrow().is_some() {
            self.needs_upload.set(true);
        }
    }

    pub fn handle_before_rendering(&self) {
        let mut renderer_slot = self.renderer.borrow_mut();
        let Some(renderer) = renderer_slot.as_mut() else {
            return;
        };

        if self.needs_upload.get() {
            self.needs_upload.set(false);
            if let Some(scene_ref) = self.scene.borrow().as_ref() {
                if let Err(err) = renderer.upload_scene(scene_ref) {
                    show_error_dialog_deferred(
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
                    return;
                }
                if let Some(cam) = self.saved_camera.borrow_mut().take() {
                    renderer.set_camera(cam);
                }
                self.dirty.set(true);
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

        let Some(texture) = renderer.render(width, height) else {
            return;
        };
        self.dirty.set(false);
        match slint::Image::try_from(texture) {
            Ok(image) => ui.set_mesh_rendering(image),
            Err(err) => show_error_dialog_deferred(
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
