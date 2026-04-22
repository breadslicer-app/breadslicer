/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sc_mesh_core::scene::Scene;
use slint::ComponentHandle;
use slint::wgpu_28::wgpu;

use crate::MainWindow;
use crate::camera::{AxisView, OrbitCamera};
use crate::error_dialog::DialogManager;
use crate::i18n::ctr;
use crate::mesh_render::{Renderer, default_fov_y};
use crate::scene_loader::{PreparedScene, SceneLoader};

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
    loader: Rc<SceneLoader>,
    // True between a `viewport-resized` event and the debounce timer firing
    // in `on_resize_settled`. While set, `handle_before_rendering` rounds the
    // render target size up to the next `RESIZE_BUCKET` multiple so drag
    // resizes don't churn the MSAA/target allocations on every pointer move.
    resizing: Cell<bool>,
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
        let dialogs = DialogManager::new();
        Self {
            ui: ui.as_weak(),
            renderer: RefCell::new(None),
            scene: RefCell::new(None),
            camera: RefCell::new(None),
            viewport: Cell::new((0.0, 0.0)),
            dirty: Cell::new(false),
            needs_upload: Cell::new(false),
            loader: Rc::new(SceneLoader::new(ui, dialogs.clone())),
            resizing: Cell::new(false),
            dialogs,
        }
    }

    /// Access to the loader FSM. `wiring.rs` drives the async file-load
    /// handshake (begin/end, stamp, slot) against the loader directly
    /// rather than through delegating methods on the controller.
    pub fn loader(&self) -> Rc<SceneLoader> {
        self.loader.clone()
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
        self.loader.invalidate_and_clear();
        *self.scene.borrow_mut() = None;
        *self.camera.borrow_mut() = None;
        self.needs_upload.set(false);
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
        let Some(prepared) = self.loader.take_finished_load() else {
            return;
        };
        let PreparedScene {
            scene,
            mesh_gpu,
            device: built_with,
            warnings: _,
        } = prepared;

        // Try to install the worker-built buffers directly. Falls
        // through to the UI-thread upload path if the renderer is
        // gone (post-teardown) or has rotated to a different device
        // than the worker built against (mid-load device loss).
        let mesh_installed = self
            .renderer
            .borrow_mut()
            .as_mut()
            .map(|r| r.install_mesh(mesh_gpu, &built_with))
            .unwrap_or(false);

        *self.camera.borrow_mut() = None;
        *self.scene.borrow_mut() = Some(scene);
        // If install succeeded, the buffers are already on the GPU; no
        // upload owed. Otherwise, defer to the recovery upload path.
        self.needs_upload.set(!mesh_installed);

        if mesh_installed {
            self.loader.note_upload_success();
            // Frame the camera off the AABB the worker computed.
            let bounds = self
                .renderer
                .borrow()
                .as_ref()
                .and_then(|r| r.scene_bounds());
            if let Some((min, max)) = bounds {
                let center = (min + max) * 0.5;
                let radius = ((max - min).length() * 0.5).max(1e-3);
                *self.camera.borrow_mut() = Some(OrbitCamera::new(center, radius));
            }
            self.dirty.set(true);
        }

        if let Some(ui) = self.ui.upgrade() {
            ui.set_has_scene(true);
        }
        self.mark_dirty();
    }

    pub fn handle_rendering_setup(&self, device: wgpu::Device, queue: wgpu::Queue) {
        self.loader.set_gpu(device.clone(), queue.clone());
        *self.renderer.borrow_mut() = Some(Renderer::new(device, queue));
        self.loader.reset_upload_failures();
        if self.scene.borrow().is_some() {
            self.needs_upload.set(true);
            self.request_redraw();
        }
    }

    // Release GPU-owning state before the graphics context goes away. The
    // scene (CPU-side) and the camera stay in the controller, so the next
    // `RenderingSetup` can re-upload without losing the user's view —
    // `handle_rendering_setup` flips `needs_upload` on its own when a scene
    // is present, so we don't set it here. The loader's gpu slot is
    // cleared too, so a click on Open between teardown and the next
    // setup is bounced by `begin_load` instead of leaving a worker to
    // build buffers against an unowned device.
    pub fn handle_rendering_teardown(&self) {
        self.loader.clear_gpu();
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
                        self.loader.note_upload_success();
                        self.dirty.set(true);
                        // Frame the camera off the AABB the upload just
                        // computed. Skipped if a camera already exists
                        // (e.g. this was a re-upload across a
                        // RenderingTeardown→Setup cycle, and the user's
                        // view should persist).
                        if self.camera.borrow().is_none()
                            && let Some((min, max)) = renderer.scene_bounds()
                        {
                            let center = (min + max) * 0.5;
                            let radius = ((max - min).length() * 0.5).max(1e-3);
                            *self.camera.borrow_mut() = Some(OrbitCamera::new(center, radius));
                        }
                    }
                    Err(err) => {
                        // Leave `needs_upload = true` so the flag continues
                        // to represent "upload owed" — a subsequent renderer
                        // setup or scene reload (both of which reset the
                        // loader's failure counter) will drive a retry
                        // without relying on coincidental side effects.
                        self.loader.note_upload_failure(&err);
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
