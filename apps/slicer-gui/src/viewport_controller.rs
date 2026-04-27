/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glam::Vec3;
use sc_mesh_core::scene::{Scene, Transform3D};
use sc_mesh_core::transform::AffineTransform3D;
use slint::ComponentHandle;
use slint::wgpu_28::wgpu;
use slint::{ModelRc, SharedString, StandardListViewItem, VecModel};

use crate::MainWindow;
use crate::camera::{AxisView, OrbitCamera};
use crate::error_dialog::DialogManager;
use crate::gizmo::{self, Axis};
use crate::i18n::ctr;
use crate::mesh_render::{GizmoFrame, Renderer, default_fov_y};
use crate::picking;
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
    // Index into `scene.objects` of the single currently-selected
    // object, or `None` if nothing is selected. The renderer reads
    // this when writing per-frame model entries (driving the
    // selection tint and the outline pass); the translate gizmo will
    // attach to it once wired.
    selection: Cell<Option<usize>>,
    // Logical-pixel position at which the left button was last
    // pressed. `Some` between `on_pointer_down` and `on_pointer_up`;
    // `on_pointer_up` reads it to decide whether the gesture was a
    // click (small movement → run picker) or a drag (large movement →
    // either consumed by the gizmo FSM or dropped on the floor).
    press_origin: Cell<Option<(f32, f32)>>,
    // Translate-gizmo drag state. Idle when no drag is in progress.
    gizmo: Cell<GizmoState>,
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

// Maximum logical-pixel distance between a left-button press and its
// release for the gesture to count as a click rather than a drag.
// 4 px is roughly two-finger-on-trackpad jitter; tighter than this and
// users with imprecise pointers fail to select; looser and stationary
// clicks bleed into starts-of-drags.
const CLICK_DRAG_THRESHOLD: f32 = 4.0;

/// Translate-gizmo input FSM. The `Idle` variant is the resting state
/// (no axis is being dragged). `Drag` captures the axis, the object's
/// transform at gesture start, the world-space pivot and anchor — the
/// running translation is composed each frame as
/// `Transform3D::Affine(delta) ∘ original`, where `delta` is the
/// world-space difference between the current ray's closest point on
/// the axis and the captured anchor. Composing onto `original` rather
/// than the running transform keeps the gesture numerically clean —
/// no float-error accumulation across hundreds of move events.
#[derive(Copy, Clone, Debug)]
enum GizmoState {
    Idle,
    Drag {
        axis: Axis,
        original: Transform3D,
        pivot: Vec3,
        anchor: Vec3,
        object_idx: usize,
    },
}

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
            selection: Cell::new(None),
            press_origin: Cell::new(None),
            gizmo: Cell::new(GizmoState::Idle),
            loader: Rc::new(SceneLoader::new(ui, dialogs.clone())),
            resizing: Cell::new(false),
            dialogs,
        }
    }

    /// Constructor for selection-FSM unit tests. The stored `Weak` is
    /// dangling, so every `ui.upgrade()` returns `None` and the
    /// UI-mirroring side effects (`set_selected_object_index`,
    /// `set_object_list`, `request_redraw`) silently skip — leaving the
    /// in-Rust selection cell, scene cell, and dirty flag as the only
    /// observable state. This matches `SceneLoader::new_for_test`.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        let dialogs = DialogManager::new();
        Self {
            ui: slint::Weak::default(),
            renderer: RefCell::new(None),
            scene: RefCell::new(None),
            camera: RefCell::new(None),
            viewport: Cell::new((0.0, 0.0)),
            dirty: Cell::new(false),
            needs_upload: Cell::new(false),
            selection: Cell::new(None),
            press_origin: Cell::new(None),
            gizmo: Cell::new(GizmoState::Idle),
            loader: Rc::new(SceneLoader::new_for_test(dialogs.clone())),
            resizing: Cell::new(false),
            dialogs,
        }
    }

    #[cfg(test)]
    pub(crate) fn install_scene_for_test(&self, scene: Scene) {
        *self.scene.borrow_mut() = Some(scene);
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
        self.selection.set(None);
        // The drag's captured `object_idx` becomes meaningless once
        // the scene is gone; clear FSM state without trying to
        // revert (the object no longer exists).
        self.gizmo.set(GizmoState::Idle);
        self.press_origin.set(None);
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.clear_scene();
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_mesh_rendering(slint::Image::default());
            ui.set_has_scene(false);
            push_object_list(&ui, Vec::new());
            ui.set_selected_object_index(-1);
        }
    }

    pub fn on_scene_load_finished(&self) {
        let Some(prepared) = self.loader.take_finished_load() else {
            return;
        };
        let PreparedScene {
            scene,
            gpu_scene,
            device: built_with,
            warnings: _,
        } = prepared;

        // Try to install the worker-built buffers directly. Falls
        // through to the UI-thread upload path if the renderer is
        // gone (post-teardown) or has rotated to a different device
        // than the worker built against (mid-load device loss).
        let scene_installed = self
            .renderer
            .borrow_mut()
            .as_mut()
            .map(|r| r.install_scene(gpu_scene, &built_with))
            .unwrap_or(false);

        *self.camera.borrow_mut() = None;
        *self.scene.borrow_mut() = Some(scene);
        // A new scene has its own object indices; any prior selection
        // referred to the previous scene and is no longer meaningful.
        self.selection.set(None);
        // If install succeeded, the buffers are already on the GPU; no
        // upload owed. Otherwise, defer to the recovery upload path.
        self.needs_upload.set(!scene_installed);

        if scene_installed {
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
            let names = self
                .scene
                .borrow()
                .as_ref()
                .map(build_object_list_names)
                .unwrap_or_default();
            push_object_list(&ui, names);
            ui.set_selected_object_index(-1);
        }
        self.mark_dirty();
    }

    /// Selection, in the controller's authoritative form. Tests use
    /// this to assert FSM transitions.
    #[cfg(test)]
    pub(crate) fn selection(&self) -> Option<usize> {
        self.selection.get()
    }

    /// Slint-side selection-changed callback. Out-of-bounds indices map
    /// to `None` (defensive — the UI shouldn't normally hand us one,
    /// but a stale callback can race with a scene reload). The Cell
    /// guard short-circuits the round-trip so the mirror-back to the UI
    /// doesn't fire `selection-changed` again.
    pub fn on_select_by_index(&self, idx: usize) {
        let scene_len = self
            .scene
            .borrow()
            .as_ref()
            .map(|s| s.objects.len())
            .unwrap_or(0);
        let new_sel = if idx < scene_len { Some(idx) } else { None };
        if self.selection.get() == new_sel {
            return;
        }
        self.selection.set(new_sel);
        self.mirror_selection_to_ui();
        self.mark_dirty();
    }

    /// Esc / "click on empty space" path. Always sets selection to
    /// `None` and mirrors back. The dirty-flag fires a redraw so the
    /// renderer drops the highlight tint and outline. If a gizmo
    /// drag is in progress, it is aborted and the affected object's
    /// transform is reverted to its captured original — leaving an
    /// object half-translated would be confusing.
    pub fn on_clear_selection(&self) {
        self.abort_gizmo_drag();
        if self.selection.get().is_none() {
            return;
        }
        self.selection.set(None);
        self.mirror_selection_to_ui();
        self.mark_dirty();
    }

    /// Left button pressed at logical coords `(x, y)`. Records the
    /// press position so `on_pointer_up` can disambiguate click from
    /// drag. If a gizmo handle is selected and the press hits it,
    /// the FSM also enters `Drag`, capturing the original transform
    /// and the world-space anchor on the axis.
    pub fn on_pointer_down(&self, x: f32, y: f32) {
        self.press_origin.set(Some((x, y)));
        self.try_start_gizmo_drag(x, y);
    }

    /// Pointer moved while the left button is held and no camera
    /// gesture is active. Drives the gizmo drag — recomputes the
    /// closest point on the axis from the current cursor ray and
    /// composes the resulting world-delta as a translation onto the
    /// captured original transform.
    pub fn on_pointer_move(&self, x: f32, y: f32) {
        let GizmoState::Drag {
            axis,
            original,
            pivot,
            anchor,
            object_idx,
        } = self.gizmo.get()
        else {
            return;
        };
        let (w, h) = self.viewport.get();
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        let new_anchor = {
            let camera_slot = self.camera.borrow();
            let Some(camera) = camera_slot.as_ref() else {
                return;
            };
            let ray = picking::ray_from_pixel(camera, default_fov_y(), w, h, x, y);
            gizmo::closest_point_on_axis(&ray, pivot, axis)
        };
        let delta = new_anchor - anchor;
        let translation =
            AffineTransform3D::from_translation([delta.x as f64, delta.y as f64, delta.z as f64]);
        let new_transform = Transform3D::Affine(translation).compose(&original);

        let mut scene_slot = self.scene.borrow_mut();
        let Some(scene) = scene_slot.as_mut() else {
            return;
        };
        if let Some(obj) = scene.objects.get_mut(object_idx) {
            obj.transform = new_transform;
        }
        drop(scene_slot);
        self.mark_dirty();
    }

    /// Left button released at logical coords `(x, y)`. If a drag was
    /// in progress, the final composed transform stands and the FSM
    /// returns to `Idle`. Otherwise the press → release is treated as
    /// a click iff its movement stays within `CLICK_DRAG_THRESHOLD`,
    /// dispatching through the picker in that case.
    pub fn on_pointer_up(&self, x: f32, y: f32) {
        let press = self.press_origin.take();
        if matches!(self.gizmo.get(), GizmoState::Drag { .. }) {
            self.gizmo.set(GizmoState::Idle);
            self.mark_dirty();
            return;
        }
        let Some((px, py)) = press else {
            // Up without a matching down — defensive, can happen if a
            // press was captured by another control before reaching us.
            return;
        };
        let dx = x - px;
        let dy = y - py;
        if dx * dx + dy * dy <= CLICK_DRAG_THRESHOLD * CLICK_DRAG_THRESHOLD {
            self.run_pick_at(x, y);
        }
    }

    /// Hit-tests the gizmo against the current camera + cursor ray
    /// and, if a handle is grabbed, transitions the FSM into `Drag`.
    /// Bails on missing camera / scene / no selection / zero
    /// viewport, in which case the press behaves like a plain click
    /// candidate.
    fn try_start_gizmo_drag(&self, x: f32, y: f32) {
        let Some(sel) = self.selection.get() else {
            return;
        };
        let (w, h) = self.viewport.get();
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        let camera_slot = self.camera.borrow();
        let Some(camera) = camera_slot.as_ref() else {
            return;
        };
        let scene_slot = self.scene.borrow();
        let Some(scene) = scene_slot.as_ref() else {
            return;
        };
        let Some(pivot) = gizmo::pivot_for_object(scene, sel) else {
            return;
        };
        let fov_y = default_fov_y();
        let scale = gizmo::gizmo_scale(camera, fov_y, h);
        let radius = gizmo::hit_radius_world(camera, fov_y, h);
        let ray = picking::ray_from_pixel(camera, fov_y, w, h, x, y);
        let Some(axis) = gizmo::hit_test(&ray, pivot, scale, radius) else {
            return;
        };
        let Some(obj) = scene.objects.get(sel) else {
            return;
        };
        let original = obj.transform;
        let anchor = gizmo::closest_point_on_axis(&ray, pivot, axis);
        drop(scene_slot);
        drop(camera_slot);
        self.gizmo.set(GizmoState::Drag {
            axis,
            original,
            pivot,
            anchor,
            object_idx: sel,
        });
    }

    /// Aborts any in-progress drag and reverts the affected object's
    /// transform to its captured original. Called from
    /// `on_clear_selection` and `on_close_scene` so a drag never
    /// outlives its valid context.
    fn abort_gizmo_drag(&self) {
        let GizmoState::Drag {
            original,
            object_idx,
            ..
        } = self.gizmo.get()
        else {
            return;
        };
        self.gizmo.set(GizmoState::Idle);
        self.press_origin.set(None);
        let mut scene_slot = self.scene.borrow_mut();
        if let Some(scene) = scene_slot.as_mut()
            && let Some(obj) = scene.objects.get_mut(object_idx)
        {
            obj.transform = original;
        }
        drop(scene_slot);
        self.mark_dirty();
    }

    /// Picker dispatch: build the ray at `(x, y)`, run `pick_object`,
    /// update the selection. Bails on missing camera / scene / zero
    /// viewport.
    fn run_pick_at(&self, x: f32, y: f32) {
        let (w, h) = self.viewport.get();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let pick = {
            let camera_slot = self.camera.borrow();
            let Some(camera) = camera_slot.as_ref() else {
                return;
            };
            let scene_slot = self.scene.borrow();
            let Some(scene) = scene_slot.as_ref() else {
                return;
            };
            let ray = picking::ray_from_pixel(camera, default_fov_y(), w, h, x, y);
            picking::pick_object(&ray, scene)
            // borrows drop here so the selection setters can re-borrow
        };
        match pick {
            Some(idx) => self.on_select_by_index(idx),
            None => self.on_clear_selection(),
        }
    }

    fn mirror_selection_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let idx = match self.selection.get() {
                Some(i) => i as i32,
                None => -1,
            };
            ui.set_selected_object_index(idx);
        }
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
        let scene_slot = self.scene.borrow();
        let Some(scene_for_render) = scene_slot.as_ref() else {
            // Scene was cleared between the upload check and now.
            return;
        };

        let gizmo_frame = self.selection.get().and_then(|sel| {
            build_gizmo_frame(scene_for_render, camera, sel, h_logical, self.gizmo.get())
        });

        let Some(texture) = renderer.render(
            scene_for_render,
            camera,
            self.selection.get(),
            gizmo_frame,
            eff_w,
            eff_h,
        ) else {
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

/// Builds the per-frame `GizmoFrame` for the renderer when a single
/// object is selected. Returns `None` if the object has no geometry
/// (no pivot to attach to). The locked-axes mask uses the camera's
/// forward direction as a single representative ray — for a small
/// gizmo near the screen centre, all rays through the gizmo region
/// are within a degree or so of the camera forward, so a per-frame
/// "is this axis grabbable from this view" check is uniform across
/// the handle.
fn build_gizmo_frame(
    scene: &Scene,
    camera: &OrbitCamera,
    selection: usize,
    viewport_h_logical: f32,
    state: GizmoState,
) -> Option<GizmoFrame> {
    let pivot = gizmo::pivot_for_object(scene, selection)?;
    let fov_y = default_fov_y();
    let scale = gizmo::gizmo_scale(camera, fov_y, viewport_h_logical);
    let active_axis = match state {
        GizmoState::Drag { axis, .. } => Some(axis),
        GizmoState::Idle => None,
    };
    let forward = (camera.target - camera.eye()).normalize_or_zero();
    let view_ray = picking::Ray {
        origin: camera.eye(),
        direction: forward,
    };
    let locked = [
        !gizmo::axis_pickable(&view_ray, Axis::X),
        !gizmo::axis_pickable(&view_ray, Axis::Y),
        !gizmo::axis_pickable(&view_ray, Axis::Z),
    ];
    Some(GizmoFrame {
        pivot,
        scale,
        active_axis,
        locked,
    })
}

/// Display names for the object-list panel. Falls back to a localised
/// `Object N` (1-indexed) when an object has no `SceneMetadata.name`,
/// then disambiguates duplicates with a `(2)` / `(3)` suffix on every
/// repeat (the first occurrence stays bare). O(N²) on object count;
/// fine for slicer-typical scenes (tens of objects). Pure helper so it
/// can be unit-tested without a `MainWindow`.
fn build_object_list_names(scene: &Scene) -> Vec<String> {
    let raw: Vec<String> = scene
        .objects
        .iter()
        .enumerate()
        .map(|(i, obj)| {
            obj.meta
                .as_ref()
                .and_then(|m| m.name.clone())
                .unwrap_or_else(|| ctr!("ObjectList", "Object {}", i + 1).to_string())
        })
        .collect();

    raw.iter()
        .enumerate()
        .map(|(i, name)| {
            let total = raw.iter().filter(|n| n.as_str() == name.as_str()).count();
            if total < 2 {
                name.clone()
            } else {
                let nth = raw
                    .iter()
                    .take(i + 1)
                    .filter(|n| n.as_str() == name.as_str())
                    .count();
                if nth == 1 {
                    name.clone()
                } else {
                    format!("{name} ({nth})")
                }
            }
        })
        .collect()
}

/// Pushes a fresh `[StandardListViewItem]` model to the window. A
/// fresh `VecModel` per call is the simplest way to keep the UI in
/// sync — the list rebuilds rarely (only on scene load/close), so the
/// extra alloc is irrelevant next to mesh I/O.
fn push_object_list(ui: &MainWindow, names: Vec<String>) {
    let items: Vec<StandardListViewItem> = names
        .into_iter()
        .map(|n| StandardListViewItem::from(SharedString::from(n)))
        .collect();
    ui.set_object_list(ModelRc::new(VecModel::from(items)));
}

#[cfg(test)]
mod build_object_list_names_tests {
    use super::*;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::{Scene, SceneMetadata, SceneObject, SceneObjectContent, Transform3D};

    fn empty_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![],
        }
    }

    fn named(name: &str) -> SceneObject {
        SceneObject {
            meta: Some(SceneMetadata {
                name: Some(name.to_string()),
                comment: None,
            }),
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(empty_mesh()),
        }
    }

    fn anonymous() -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(empty_mesh()),
        }
    }

    #[test]
    fn anonymous_objects_get_one_indexed_fallback_names() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![anonymous(), anonymous(), anonymous()],
        };
        let names = build_object_list_names(&scene);
        // Translation pipeline returns "Object {N}" verbatim when no .po
        // catalogue is loaded (the test runtime), so we can compare
        // strings directly.
        assert_eq!(names, vec!["Object 1", "Object 2", "Object 3"]);
    }

    #[test]
    fn named_objects_keep_their_names() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Cap"), named("Pin")],
        };
        let names = build_object_list_names(&scene);
        assert_eq!(names, vec!["Body", "Cap", "Pin"]);
    }

    #[test]
    fn duplicate_names_are_disambiguated_only_after_the_first() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Body"), named("Body")],
        };
        let names = build_object_list_names(&scene);
        assert_eq!(names, vec!["Body", "Body (2)", "Body (3)"]);
    }

    #[test]
    fn unique_names_are_left_alone_even_when_others_collide() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Body"), named("Pin")],
        };
        let names = build_object_list_names(&scene);
        assert_eq!(names, vec!["Body", "Body (2)", "Pin"]);
    }

    #[test]
    fn fallback_names_can_collide_with_explicit_names() {
        // A metadata-named "Object 2" at index 0 collides with the
        // anonymous fallback for the object at index 1 (whose 1-indexed
        // position is also 2). The dedup pass treats them uniformly —
        // both are "Object 2", so the second instance gets a "(2)"
        // suffix; the third anonymous, at index 2, falls back to the
        // unique "Object 3".
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Object 2"), anonymous(), anonymous()],
        };
        let names = build_object_list_names(&scene);
        assert_eq!(names, vec!["Object 2", "Object 2 (2)", "Object 3"]);
    }

    #[test]
    fn empty_scene_yields_empty_name_list() {
        let scene = Scene::default();
        let names = build_object_list_names(&scene);
        assert!(names.is_empty());
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};

    fn empty_obj() -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(IndexedMesh {
                meta: None,
                vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
                faces: vec![],
            }),
        }
    }

    fn scene_with_n_objects(n: usize) -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: (0..n).map(|_| empty_obj()).collect(),
        }
    }

    #[test]
    fn fresh_controller_has_no_selection() {
        let ctrl = ViewportController::new_for_test();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn select_by_index_records_in_bounds_indices() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(3));
        ctrl.on_select_by_index(1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_select_by_index(2);
        assert_eq!(ctrl.selection(), Some(2));
    }

    #[test]
    fn select_by_index_clamps_out_of_bounds_to_none() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(2));
        // Pre-seed a valid selection so we can observe the reset.
        ctrl.on_select_by_index(0);
        ctrl.on_select_by_index(99);
        assert_eq!(
            ctrl.selection(),
            None,
            "an out-of-bounds index must collapse to None, not silently keep the prior selection"
        );
    }

    #[test]
    fn select_by_index_with_no_scene_is_a_no_op() {
        let ctrl = ViewportController::new_for_test();
        // No `install_scene_for_test`; the scene cell holds None.
        ctrl.on_select_by_index(0);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn clear_selection_resets_to_none() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(2));
        ctrl.on_select_by_index(1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn close_scene_clears_selection() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(3));
        ctrl.on_select_by_index(2);
        assert_eq!(ctrl.selection(), Some(2));
        ctrl.on_close_scene();
        assert_eq!(ctrl.selection(), None);
        // The scene cell is cleared too — a follow-up select must not
        // resurrect a prior index against a missing scene.
        ctrl.on_select_by_index(2);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn select_then_select_same_index_is_idempotent() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(2));
        ctrl.on_select_by_index(0);
        ctrl.dirty.set(false);
        ctrl.on_select_by_index(0);
        assert!(
            !ctrl.dirty.get(),
            "re-selecting the current index should not re-mark the controller dirty"
        );
    }

    #[test]
    fn viewport_click_with_no_camera_is_a_no_op() {
        // The picker requires a camera. If a click runs before the
        // first `RenderingSetup` (or after a teardown that dropped
        // the camera), we expect no panic and no selection change.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(1));
        // No camera installed.
        ctrl.on_pointer_down(50.0, 50.0);
        ctrl.on_pointer_up(50.0, 50.0);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn viewport_click_with_zero_viewport_is_a_no_op() {
        // Width/height of zero means we have no projection to build a
        // ray from. Bail rather than divide-by-zeroing.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(1));
        // viewport defaults to (0.0, 0.0)
        ctrl.on_pointer_down(0.0, 0.0);
        ctrl.on_pointer_up(0.0, 0.0);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn pointer_up_without_matching_down_is_ignored() {
        // Defensive: if Slint somehow sends an up without a down (a
        // press captured elsewhere, or our state out of sync), the
        // controller must not run the picker against an unset press.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(1));
        ctrl.on_pointer_up(50.0, 50.0);
        assert_eq!(ctrl.selection(), None);
    }
}

#[cfg(test)]
mod viewport_click_tests {
    use super::*;
    use crate::camera::OrbitCamera;
    use float_cmp::assert_approx_eq;
    use glam::Vec3;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;

    fn front_facing_triangle_at(translation: [f32; 3]) -> SceneObject {
        // CCW from +Z → front face faces +Z.
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([-0.5, -0.5, 0.0]),
                Vertex::new([0.5, -0.5, 0.0]),
                Vertex::new([0.0, 0.5, 0.0]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 1, 2],
            }],
        };
        SceneObject {
            meta: None,
            transform: Transform3D::Affine(AffineTransform3D::from_translation([
                translation[0] as f64,
                translation[1] as f64,
                translation[2] as f64,
            ])),
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    /// Camera looking at a target along `-Z` from `+Z` so the centre
    /// of the viewport projects onto the world-space line `x = tx,
    /// y = ty, z ∈ R`. Picking a triangle on that line via the centre
    /// pixel is the cleanest way to exercise the ray pipeline
    /// end-to-end.
    fn camera_looking_down_minus_z_at(target: Vec3, distance: f32) -> OrbitCamera {
        let mut cam = OrbitCamera::new(target, distance);
        cam.set_axis_view(crate::camera::AxisView::Top);
        cam
    }

    #[test]
    fn click_centre_picks_the_triangle_directly_under_the_cursor() {
        let ctrl = ViewportController::new_for_test();
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                front_facing_triangle_at([0.0, 0.0, 0.0]),
                front_facing_triangle_at([10.0, 0.0, 0.0]),
            ],
        };
        ctrl.install_scene_for_test(scene);

        // Camera framed on the first triangle, looking down -Z.
        // Centre pixel of the viewport casts a ray straight at it.
        *ctrl.camera.borrow_mut() = Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        ctrl.viewport.set((200.0, 100.0));

        ctrl.on_pointer_down(100.0, 50.0);
        ctrl.on_pointer_up(100.0, 50.0);
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn click_in_empty_space_clears_the_selection() {
        let ctrl = ViewportController::new_for_test();
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![front_facing_triangle_at([0.0, 0.0, 0.0])],
        };
        ctrl.install_scene_for_test(scene);
        *ctrl.camera.borrow_mut() = Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        ctrl.viewport.set((200.0, 100.0));

        // Pre-seed a selection so we can observe the clear.
        ctrl.on_select_by_index(0);
        assert_eq!(ctrl.selection(), Some(0));

        // Click at the corner — no triangle there.
        ctrl.on_pointer_down(0.0, 0.0);
        ctrl.on_pointer_up(0.0, 0.0);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn drag_does_not_run_the_picker() {
        // A press → release with movement larger than the click
        // threshold is a drag, which is currently dropped on the
        // floor (the gizmo FSM will consume it once wired). It must
        // not hand off to the picker — that would let large pointer
        // moves change the selection.
        let ctrl = ViewportController::new_for_test();
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![front_facing_triangle_at([0.0, 0.0, 0.0])],
        };
        ctrl.install_scene_for_test(scene);
        *ctrl.camera.borrow_mut() = Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        ctrl.viewport.set((200.0, 100.0));

        // Press over the triangle, release 50 px away → drag.
        ctrl.on_pointer_down(100.0, 50.0);
        ctrl.on_pointer_up(150.0, 50.0);
        assert_eq!(ctrl.selection(), None);
    }

    /// Camera looking along +Y from `(0, -3, 0)` toward the origin
    /// with up = +Z. The world axes project as: +X = screen-right,
    /// +Z = screen-up, +Y = into the screen. With the gizmo pivot
    /// at the origin, the X handle lies along the centre horizontal
    /// of the viewport — clicks at `(centre_x + Δ, centre_y)` land on
    /// the X handle.
    fn front_camera() -> OrbitCamera {
        let mut c = OrbitCamera::new(Vec3::ZERO, 1.0);
        c.set_axis_view(crate::camera::AxisView::Front);
        c
    }

    fn install_single_object_at_origin(ctrl: &ViewportController) {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![front_facing_triangle_at([0.0, 0.0, 0.0])],
        };
        ctrl.install_scene_for_test(scene);
        *ctrl.camera.borrow_mut() = Some(front_camera());
        ctrl.viewport.set((200.0, 100.0));
        ctrl.on_select_by_index(0);
    }

    fn read_translation(ctrl: &ViewportController) -> Vec3 {
        let scene_slot = ctrl.scene.borrow();
        let scene = scene_slot.as_ref().unwrap();
        match scene.objects[0].transform {
            Transform3D::Identity => Vec3::ZERO,
            Transform3D::Affine(a) => Vec3::new(
                a.translation[0] as f32,
                a.translation[1] as f32,
                a.translation[2] as f32,
            ),
        }
    }

    #[test]
    fn pointer_down_on_x_handle_enters_drag_state() {
        // The X handle in the front-camera setup lies along world +X
        // (which projects to screen-right at centre-y). A press at
        // (150, 50) puts the cursor over the handle (the gizmo
        // shaft extends from the origin to ~world X = 2 at the
        // current camera distance and viewport size).
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(150.0, 50.0);
        match ctrl.gizmo.get() {
            GizmoState::Drag { axis, .. } => assert_eq!(axis, Axis::X),
            other => panic!("expected Drag(X), got {other:?}"),
        }
    }

    #[test]
    fn pointer_down_off_handle_does_not_enter_drag_state() {
        // A press far from any handle lands as a plain click
        // candidate — the FSM stays Idle.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(0.0, 0.0);
        assert!(matches!(ctrl.gizmo.get(), GizmoState::Idle));
    }

    #[test]
    fn drag_translates_object_along_world_axis() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        // Compute the expected translation from the press / release
        // rays and the closest-point-on-axis formula. The expected
        // delta is what the renderer-agnostic helpers return for
        // these two cursor positions; the controller's job is to
        // apply that as a translation onto the object's transform.
        let press = (150.0_f32, 50.0_f32);
        let release = (175.0_f32, 50.0_f32);
        let cam = ctrl.camera.borrow().as_ref().unwrap().clone();
        let fov_y = default_fov_y();
        let press_ray = picking::ray_from_pixel(&cam, fov_y, 200.0, 100.0, press.0, press.1);
        let release_ray = picking::ray_from_pixel(&cam, fov_y, 200.0, 100.0, release.0, release.1);
        let press_anchor = gizmo::closest_point_on_axis(&press_ray, Vec3::ZERO, Axis::X);
        let release_anchor = gizmo::closest_point_on_axis(&release_ray, Vec3::ZERO, Axis::X);
        let expected = release_anchor - press_anchor;

        ctrl.on_pointer_down(press.0, press.1);
        // Drag must have started — otherwise the move is a no-op.
        assert!(matches!(ctrl.gizmo.get(), GizmoState::Drag { .. }));
        ctrl.on_pointer_move(release.0, release.1);

        let actual = read_translation(&ctrl);
        assert_approx_eq!(f32, actual.x, expected.x, epsilon = 1e-4);
        assert_approx_eq!(f32, actual.y, expected.y, epsilon = 1e-4);
        assert_approx_eq!(f32, actual.z, expected.z, epsilon = 1e-4);
    }

    #[test]
    fn second_drag_move_recomposes_from_original_not_running_transform() {
        // Two pointer-moves during the same drag: the final transform
        // must reflect the absolute cursor position relative to the
        // press, *not* the cumulative sum of per-frame deltas. The
        // distinction matters when Phase D adds rotation/scale —
        // pinning it here means a bug in that direction trips this
        // test loudly.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        let press = (150.0_f32, 50.0_f32);
        let mid = (160.0_f32, 50.0_f32);
        let release = (175.0_f32, 50.0_f32);
        let cam = ctrl.camera.borrow().as_ref().unwrap().clone();
        let fov_y = default_fov_y();
        let press_anchor = gizmo::closest_point_on_axis(
            &picking::ray_from_pixel(&cam, fov_y, 200.0, 100.0, press.0, press.1),
            Vec3::ZERO,
            Axis::X,
        );
        let release_anchor = gizmo::closest_point_on_axis(
            &picking::ray_from_pixel(&cam, fov_y, 200.0, 100.0, release.0, release.1),
            Vec3::ZERO,
            Axis::X,
        );
        let expected = release_anchor - press_anchor;

        ctrl.on_pointer_down(press.0, press.1);
        ctrl.on_pointer_move(mid.0, mid.1);
        ctrl.on_pointer_move(release.0, release.1);

        let actual = read_translation(&ctrl);
        assert_approx_eq!(f32, actual.x, expected.x, epsilon = 1e-4);
    }

    #[test]
    fn pointer_up_after_drag_clears_state_and_keeps_translation() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(150.0, 50.0);
        ctrl.on_pointer_move(175.0, 50.0);
        let mid_drag = read_translation(&ctrl);
        ctrl.on_pointer_up(175.0, 50.0);

        // Drag state cleared.
        assert!(matches!(ctrl.gizmo.get(), GizmoState::Idle));
        // Translation persists — the user committed the drag.
        let after = read_translation(&ctrl);
        assert_approx_eq!(f32, after.x, mid_drag.x, epsilon = 1e-6);
    }

    #[test]
    fn esc_mid_drag_reverts_transform() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(150.0, 50.0);
        ctrl.on_pointer_move(175.0, 50.0);
        // Object has moved.
        assert!(read_translation(&ctrl).x.abs() > 1e-4);

        // Esc / clear-selection during a drag aborts and reverts.
        ctrl.on_clear_selection();
        let reverted = read_translation(&ctrl);
        assert_approx_eq!(f32, reverted.x, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, reverted.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, reverted.z, 0.0, epsilon = 1e-6);
        assert!(matches!(ctrl.gizmo.get(), GizmoState::Idle));
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn close_scene_mid_drag_does_not_panic() {
        // The drag captures `object_idx`, which becomes invalid when
        // the scene is dropped. `on_close_scene` must clear FSM
        // state without trying to revert through the missing scene.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(150.0, 50.0);
        ctrl.on_pointer_move(175.0, 50.0);
        ctrl.on_close_scene();

        assert!(matches!(ctrl.gizmo.get(), GizmoState::Idle));
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn jitter_within_threshold_still_counts_as_a_click() {
        // A press → release that moves only a few subpixels
        // (trackpad jitter, debounced finger lift) must still be
        // treated as a click. Threshold is 4 logical pixels.
        let ctrl = ViewportController::new_for_test();
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![front_facing_triangle_at([0.0, 0.0, 0.0])],
        };
        ctrl.install_scene_for_test(scene);
        *ctrl.camera.borrow_mut() = Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        ctrl.viewport.set((200.0, 100.0));

        ctrl.on_pointer_down(100.0, 50.0);
        ctrl.on_pointer_up(101.5, 51.5);
        assert_eq!(ctrl.selection(), Some(0));
    }
}
