/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sc_mesh_core::scene::Scene;
use slint::ComponentHandle;
use slint::wgpu_28::wgpu;

use super::gizmo_drag::{DragOp, GizmoState};
use super::gizmo_fsm::{
    GizmoFsm, HoverCtx as GizmoHoverCtx, MoveCtx as GizmoMoveCtx, StartCtx as GizmoStartCtx,
};
use super::input_state::{ClickOrDrag, InputState};
#[cfg(test)]
use super::keyboard_drive::ARROW_X_POS;
use super::keyboard_drive::{IntegrationCtx, KbdFocus, KeyboardDriveState};
use super::object_list::{build_object_list_names, push_object_list};
use super::render_scheduler;
use super::selection_state::SelectionState;
use super::types::{FrameState, ObjectIdx, ScreenPoint, Selection, ViewportSize};
use crate::MainWindow;
use crate::PointerModifiers;
use crate::camera::{AxisView, OrbitCamera};
use crate::error_dialog::DialogManager;
use crate::gizmo;
use crate::i18n::ctr;
use crate::mesh_render::{GizmoDragVisual, GizmoVisibility, Renderer, default_fov_y};
use crate::picking;
use crate::scene_document::SceneDocument;
use crate::scene_loader::{PreparedScene, SceneLoader};

#[cfg(test)]
use crate::gizmo::axis::Axis;

pub struct ViewportController {
    ui: slint::Weak<MainWindow>,
    // Document-level model. The controller mirrors its own `scene` cell
    // here on every install / clear so consumers that observe the
    // document already see live state. A subsequent checkpoint
    // migrates reads to the document and drops the local cell.
    document: Rc<SceneDocument>,
    renderer: RefCell<Option<Renderer>>,
    scene: RefCell<Option<Scene>>,
    // The camera lives next to the scene (not inside the GPU renderer), so it
    // survives device-loss / RenderingTeardown→Setup cycles for free and can
    // be manipulated without a GPU present.
    camera: RefCell<Option<OrbitCamera>>,
    viewport: Cell<Option<ViewportSize>>,
    // Per-frame work owed: a redraw (something visual changed) and/or
    // a GPU upload (CPU-side scene differs from the renderer's
    // buffers). Both can be set independently — a successful upload
    // also sets the redraw flag so the just-uploaded scene is shown,
    // and a redraw can be flagged alone (e.g. camera moved without a
    // scene change).
    frame: Cell<FrameState>,
    // Index into `scene.objects` of the single currently-selected
    // object, or `None` if nothing is selected. The renderer reads
    // this when writing per-frame model entries (driving the
    // selection tint and the outline pass); the translate gizmo will
    // attach to it once wired.
    selection: SelectionState,
    // Logical-pixel position at which the left button was last
    // pressed. `Some` between `on_pointer_down` and `on_pointer_up`;
    // `on_pointer_up` reads it to decide whether the gesture was a
    // click (small movement → run picker) or a drag (large movement →
    // either consumed by the gizmo FSM or dropped on the floor).
    input_state: InputState,
    // Translate-gizmo drag state. Idle when no drag is in progress.
    gizmo_fsm: GizmoFsm,
    // Keyboard-driven gizmo state: focus (`T`/`R`/`S` modes), held-
    // arrow bitset (driven by `gizmo-arrow-pressed/released`), and
    // the per-frame integrator's tick clock. Sampled in
    // `handle_before_rendering`.
    kbd_drive: KeyboardDriveState,
    loader: Rc<SceneLoader>,
    // True between a `viewport-resized` event and the debounce timer firing
    // in `on_resize_settled`. While set, `handle_before_rendering` rounds the
    // render target size up to the next `RESIZE_BUCKET` multiple so drag
    // resizes don't churn the MSAA/target allocations on every pointer move.
    resizing: Cell<bool>,
    dialogs: Rc<DialogManager>,
}

impl ViewportController {
    pub fn new(ui: &MainWindow, document: Rc<SceneDocument>) -> Self {
        let dialogs = DialogManager::new();
        Self {
            ui: ui.as_weak(),
            document,
            renderer: RefCell::new(None),
            scene: RefCell::new(None),
            camera: RefCell::new(None),
            viewport: Cell::new(None),
            frame: Cell::new(FrameState::default()),
            selection: SelectionState::new(),
            input_state: InputState::new(),
            gizmo_fsm: GizmoFsm::new(),
            kbd_drive: KeyboardDriveState::new(),
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
            document: SceneDocument::new(),
            renderer: RefCell::new(None),
            scene: RefCell::new(None),
            camera: RefCell::new(None),
            viewport: Cell::new(None),
            frame: Cell::new(FrameState::default()),
            selection: SelectionState::new(),
            input_state: InputState::new(),
            gizmo_fsm: GizmoFsm::new(),
            kbd_drive: KeyboardDriveState::new(),
            loader: Rc::new(SceneLoader::new_for_test(dialogs.clone())),
            resizing: Cell::new(false),
            dialogs,
        }
    }

    /// Asserts the controller's `scene` cell and the document's `scene`
    /// cell agree on presence. Catches a missed mirror at any other
    /// scene-touching method's entry. Compiled out in release builds.
    fn debug_assert_scene_mirror_consistent(&self) {
        debug_assert_eq!(
            self.scene.borrow().is_some(),
            self.document.is_loaded(),
            "controller and document scene presence must agree"
        );
    }

    #[cfg(test)]
    pub(crate) fn install_scene_for_test(&self, scene: Scene) {
        self.debug_assert_scene_mirror_consistent();
        self.document.install_scene(scene.clone());
        *self.scene.borrow_mut() = Some(scene);
        self.debug_assert_scene_mirror_consistent();
    }

    #[cfg(test)]
    pub(crate) fn clear_frame_state_for_test(&self) {
        self.frame.set(FrameState::default());
    }

    #[cfg(test)]
    pub(crate) fn redraw_owed_for_test(&self) -> bool {
        self.frame.get().redraw_owed()
    }

    #[cfg(test)]
    pub(crate) fn set_viewport_for_test(&self, w: f32, h: f32) {
        self.viewport.set(ViewportSize::new(w, h));
    }

    #[cfg(test)]
    pub(crate) fn viewport_for_test(&self) -> Option<(f32, f32)> {
        self.viewport.get().map(|vp| (vp.width(), vp.height()))
    }

    /// Access to the loader FSM. `wiring.rs` drives the async file-load
    /// handshake (begin/end, stamp, slot) against the loader directly
    /// rather than through delegating methods on the controller.
    pub fn loader(&self) -> Rc<SceneLoader> {
        self.loader.clone()
    }

    fn request_redraw(&self) {
        render_scheduler::request_redraw(&self.ui);
    }

    fn mark_dirty(&self) {
        render_scheduler::mark_dirty(&self.frame, &self.ui);
    }

    pub fn on_viewport_resized(&self, w: f32, h: f32) {
        render_scheduler::on_viewport_resized(
            &self.viewport,
            &self.resizing,
            &self.frame,
            &self.ui,
            w,
            h,
        );
    }

    /// Called once `viewport-resized` has stayed quiet for `RESIZE_SETTLE`.
    /// Drops the bucketed MSAA / target allocations so the next frame
    /// reallocates at the exact logical size, eliminating the `image-fit:
    /// fill` stretch that persisted during the drag.
    pub fn on_resize_settled(&self) {
        render_scheduler::on_resize_settled(&self.resizing, &self.frame, &self.ui, || {
            if let Some(r) = self.renderer.borrow_mut().as_mut() {
                r.invalidate_sizing();
            }
        });
    }

    pub fn on_rotate(&self, dx: f32, dy: f32) {
        let Some(vp) = self.viewport.get() else {
            return;
        };
        if let Some(cam) = self.camera.borrow_mut().as_mut() {
            cam.orbit(dx, dy, vp.height());
            self.mark_dirty();
        }
    }

    pub fn on_pan(&self, dx: f32, dy: f32) {
        let Some(vp) = self.viewport.get() else {
            return;
        };
        if let Some(cam) = self.camera.borrow_mut().as_mut() {
            cam.pan(dx, dy, default_fov_y(), vp.height());
            self.mark_dirty();
        }
    }

    pub fn on_zoom(&self, delta: f32, cursor_x: f32, cursor_y: f32) {
        let Some(vp) = self.viewport.get() else {
            return;
        };
        let factor = (-delta * 0.005).exp().clamp(0.1, 10.0);
        if let Some(cam) = self.camera.borrow_mut().as_mut() {
            cam.zoom_at(
                factor,
                cursor_x,
                cursor_y,
                default_fov_y(),
                vp.width(),
                vp.height(),
            );
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
        self.debug_assert_scene_mirror_consistent();
        self.loader.invalidate_and_clear();
        self.document.clear_scene();
        *self.scene.borrow_mut() = None;
        *self.camera.borrow_mut() = None;
        self.frame.set(FrameState::default());
        self.selection.set(Selection::None);
        // The drag's captured `object_idx` becomes meaningless once
        // the scene is gone; clear FSM state without trying to
        // revert (the object no longer exists).
        self.gizmo_fsm.set_idle();
        self.gizmo_fsm.clear_hover();
        self.input_state.set_idle();
        // Keyboard focus is anchored on the (now-missing) selection;
        // reset it together with the held-arrow bitset so a stuck
        // key can't drive a phantom integrator after scene close.
        self.kbd_drive.reset();
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.clear_scene();
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_mesh_rendering(slint::Image::default());
            ui.set_has_scene(false);
            push_object_list(&ui, Vec::new());
            ui.set_selected_object_index(-1);
            ui.set_gizmo_keyboard_focus_active(false);
        }
    }

    pub fn on_scene_load_finished(&self) {
        self.debug_assert_scene_mirror_consistent();
        let Some(prepared) = self.loader.take_finished_load() else {
            return;
        };
        let PreparedScene {
            scene,
            gpu_scene,
            device: built_with,
            warnings: _,
        } = prepared;
        self.document.install_scene(scene.clone());

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
        self.selection.set(Selection::None);
        // If install succeeded, the buffers are already on the GPU; no
        // upload owed. Otherwise, defer to the recovery upload path.
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
            self.frame.update(FrameState::with_redraw_owed);
        } else {
            self.frame.update(FrameState::with_upload_owed);
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
        self.selection.primary_idx()
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
        let new_sel = if idx < scene_len {
            Selection::One(ObjectIdx(idx))
        } else {
            Selection::None
        };
        if self.selection.current() == new_sel {
            return;
        }
        self.selection.set(new_sel);
        self.mirror_selection_to_ui();
        self.mark_dirty();
    }

    /// Tab / Shift+Tab selection cycling.
    ///
    /// - `delta == 1` advances forward (Tab); `delta == -1` advances
    ///   backward (Shift+Tab). Other deltas are clamped to ±1 so a
    ///   stale Slint binding can't drive larger jumps.
    /// - With no selection, either direction selects
    ///   `scene.objects[0]` — the same key thus serves "select the
    ///   first thing" and "advance to the next".
    /// - With a selection, the index wraps modulo `len`.
    /// - On an empty / missing scene, nothing happens.
    ///
    /// A side-effect of changing the selection through this path is
    /// that any in-progress mouse drag aborts (because the abort runs
    /// out of the same `on_clear_selection` cascade — but we don't
    /// invoke it here, since Tab on a different object should pick
    /// up the new selection cleanly without touching the running
    /// transform). The current implementation does not abort an
    /// in-progress drag on Tab; if that turns out to be wrong, the
    /// fix is to call `abort_gizmo_drag` here.
    pub fn on_selection_cycle(&self, delta: i32) {
        let scene_len = self
            .scene
            .borrow()
            .as_ref()
            .map(|s| s.objects.len())
            .unwrap_or(0);
        if scene_len == 0 {
            return;
        }
        let step = delta.signum();
        let next = match self.selection.current() {
            Selection::None => 0,
            Selection::One(i) => {
                let len_i = scene_len as i32;
                let cur = i.as_usize() as i32;
                ((cur + step).rem_euclid(len_i)) as usize
            }
        };
        self.on_select_by_index(next);
    }

    /// Esc / "click on empty space" path. Cascades through the
    /// gesture stack one level per call:
    ///
    /// 1. If a gizmo drag is in progress → abort it (transform reverts
    ///    to the captured original), keep selection / focus.
    /// 2. Else if keyboard gizmo focus is active → drop focus only,
    ///    keep selection.
    /// 3. Else → clear selection (Phase B behaviour).
    ///
    /// "Click on empty space" enters at level 3 (the picker has
    /// already dispatched here from `on_pointer_up` after determining
    /// no object was hit); Esc enters at level 1 and naturally
    /// cascades through repeated presses.
    pub fn on_clear_selection(&self) {
        if matches!(self.gizmo_fsm.current(), GizmoState::Drag { .. }) {
            self.abort_gizmo_drag();
            return;
        }
        if self.kbd_drive.current_focus() != KbdFocus::None {
            self.set_keyboard_focus(KbdFocus::None);
            return;
        }
        if matches!(self.selection.current(), Selection::None) {
            return;
        }
        self.selection.set(Selection::None);
        // Selection going away kills the gizmo, so any cached hover
        // tint targets a handle that no longer renders.
        self.gizmo_fsm.clear_hover();
        self.mirror_selection_to_ui();
        self.mark_dirty();
    }

    /// `T`/`R`/`S` keyboard handlers. Each requires a selection to
    /// exist; without one, the press is a no-op (the user has to
    /// pick something to focus on first). Pressing the same focus
    /// type twice is a no-op so the integrator's tick clock isn't
    /// reset by autorepeat.
    pub fn on_gizmo_focus_translate(&self) {
        self.set_keyboard_focus(KbdFocus::Translate);
    }

    pub fn on_gizmo_focus_rotate(&self) {
        self.set_keyboard_focus(KbdFocus::Rotate);
    }

    pub fn on_gizmo_focus_scale(&self) {
        self.set_keyboard_focus(KbdFocus::Scale);
    }

    /// Internal: writes the focus cell, mirrors to the UI property,
    /// and resets the held-arrow bitset on focus change so a held key
    /// from one mode doesn't bleed into another.
    fn set_keyboard_focus(&self, focus: KbdFocus) {
        let has_selection = !matches!(self.selection.current(), Selection::None);
        if !self.kbd_drive.try_set_focus(focus, has_selection) {
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_gizmo_keyboard_focus_active(focus != KbdFocus::None);
        }
        self.mark_dirty();
    }

    /// Held-key handler for the arrow / PageUp / PageDown bindings
    /// while keyboard gizmo focus is active. `code` is the
    /// `ARROW_*` index defined above; out-of-range codes are
    /// ignored. Marks the controller dirty so the next render-tick
    /// runs the integrator.
    pub fn on_gizmo_arrow_pressed(&self, code: i32) {
        if self.kbd_drive.current_focus() == KbdFocus::None {
            return;
        }
        if self.kbd_drive.update_arrow(code, true) {
            self.mark_dirty();
        }
    }

    /// Counterpart to `on_gizmo_arrow_pressed`. Released codes are
    /// always processed even with focus dropped — a release that
    /// arrives just after the focus was cleared (race with Esc
    /// → focus drop) still has to clear its bit so the bitset
    /// doesn't accumulate stuck flags.
    pub fn on_gizmo_arrow_released(&self, code: i32) {
        if self.kbd_drive.update_arrow(code, false) {
            self.mark_dirty();
        }
    }

    /// Left button pressed at logical coords `(x, y)`. Records the
    /// press position so `on_pointer_up` can disambiguate click from
    /// drag. If a gizmo handle is selected and the press hits it,
    /// the FSM also enters `Drag`, capturing the original transform
    /// and the world-space anchor on the axis. The modifier state
    /// at press feeds the `ScaleAxis` proportional flag — pressing
    /// the per-axis cube with Shift already held starts the drag in
    /// uniform-scale mode.
    pub fn on_pointer_down(&self, x: f32, y: f32, modifiers: PointerModifiers) {
        self.input_state.set_pressed(ScreenPoint { x, y });
        self.try_start_gizmo_drag(x, y, modifiers);
    }

    /// Pointer moved while the left button is held and no camera
    /// gesture is active. Drives the gizmo drag — branches on the
    /// active `DragOp`, recomputes the per-op delta from the current
    /// cursor ray, and composes the resulting transform onto the
    /// captured original. If a `ScaleAxis` drag is live and the live
    /// Shift state has flipped since the last frame, re-anchors the
    /// drag (current transform → new `original`, current cursor →
    /// new anchors) so the per-axis ↔ uniform formula switch is
    /// visually snap-free.
    pub fn on_pointer_move(&self, x: f32, y: f32, modifiers: PointerModifiers) {
        let ctx = GizmoMoveCtx {
            scene: &self.scene,
            camera: &self.camera,
            viewport: self.viewport.get(),
            cursor: ScreenPoint { x, y },
            modifiers,
        };
        let mut dirty = self.gizmo_fsm.on_move(ctx);
        // No-op while a drag is in progress (the FSM short-circuits
        // hover updates internally), so this just covers the hover-
        // tint update on idle pointer moves.
        let hover_ctx = GizmoHoverCtx {
            scene: &self.scene,
            camera: &self.camera,
            viewport: self.viewport.get(),
            selection_idx: self.selection.primary_idx(),
            cursor: ScreenPoint { x, y },
        };
        dirty |= self.gizmo_fsm.update_hover(hover_ctx);
        if dirty {
            self.mark_dirty();
        }
    }

    /// Left button released at logical coords `(x, y)`. If a drag was
    /// in progress, the final composed transform stands and the FSM
    /// returns to `Idle`. Otherwise the press → release is treated as
    /// a click iff its movement stays within `CLICK_DRAG_THRESHOLD`,
    /// dispatching through the picker in that case. Modifier state
    /// at release is currently unused; threaded through for symmetry
    /// with the down/move callbacks and so future operations
    /// (snapping, etc.) can read it without another signature change.
    pub fn on_pointer_up(&self, x: f32, y: f32, _modifiers: PointerModifiers) {
        let outcome = self.input_state.click_or_drag(ScreenPoint { x, y });
        if matches!(self.gizmo_fsm.current(), GizmoState::Drag { .. }) {
            self.gizmo_fsm.set_idle();
            self.mark_dirty();
            return;
        }
        if outcome == ClickOrDrag::Click {
            self.run_pick_at(x, y);
        }
    }

    /// Runs the four gizmo hit tests against the current camera +
    /// cursor ray and, if a handle is grabbed, transitions the FSM
    /// into `Drag`. The front-most candidate (smallest positive `t`
    /// along the press ray) wins across translate cones, rotate
    /// arcs, scale midpoint cubes, and the centre cube. Bails on
    /// missing camera / scene / no selection / zero viewport, in
    /// which case the press behaves like a plain click candidate.
    ///
    /// `modifiers.shift` at press time captures into the resulting
    /// `ScaleAxis.proportional` flag — a press on the per-axis cube
    /// with Shift held starts uniformly; subsequent toggles
    /// re-anchor through the move path.
    fn try_start_gizmo_drag(&self, x: f32, y: f32, modifiers: PointerModifiers) {
        let ctx = GizmoStartCtx {
            scene: &self.scene,
            camera: &self.camera,
            viewport: self.viewport.get(),
            selection_idx: self.selection.primary_idx(),
            press: ScreenPoint { x, y },
            modifiers,
        };
        self.gizmo_fsm.try_start_drag(ctx);
    }

    /// Aborts any in-progress drag and reverts the affected object's
    /// transform to its captured original. Called from
    /// `on_clear_selection` and `on_close_scene` so a drag never
    /// outlives its valid context.
    fn abort_gizmo_drag(&self) {
        if self.gizmo_fsm.abort(&self.scene) {
            self.input_state.set_idle();
            self.mark_dirty();
        }
    }

    /// Per-frame integrator: if a keyboard gizmo focus is active and
    /// any arrow key is held, applies one tick of motion to the
    /// selected object. Bails (without resetting the tick clock) on
    /// missing camera / scene / selection / zero viewport so a hot
    /// re-init doesn't hand back time the user didn't drive.
    ///
    /// Marks the controller dirty when a tick fires so the next
    /// `handle_before_rendering` runs again — the integrator
    /// self-pumps as long as the user holds keys, and quiesces when
    /// they release.
    fn integrate_keyboard_arrows(&self) {
        let ctx = IntegrationCtx {
            scene: &self.scene,
            camera: &self.camera,
            viewport: self.viewport.get(),
            selection_idx: self.selection.primary_idx(),
        };
        if self.kbd_drive.integrate(ctx) {
            self.mark_dirty();
        }
    }

    /// Picker dispatch: build the ray at `(x, y)`, run `pick_object`,
    /// update the selection. Bails on missing camera / scene / zero
    /// viewport.
    fn run_pick_at(&self, x: f32, y: f32) {
        let Some(vp) = self.viewport.get() else {
            return;
        };
        let pick = {
            let camera_slot = self.camera.borrow();
            let Some(camera) = camera_slot.as_ref() else {
                return;
            };
            let scene_slot = self.scene.borrow();
            let Some(scene) = scene_slot.as_ref() else {
                return;
            };
            let ray =
                picking::ray_from_pixel(camera, default_fov_y(), vp.width(), vp.height(), x, y);
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
            let idx = match self.selection.current() {
                Selection::One(i) => i.as_usize() as i32,
                Selection::None => -1,
            };
            ui.set_selected_object_index(idx);
        }
    }

    pub fn handle_rendering_setup(&self, device: wgpu::Device, queue: wgpu::Queue) {
        self.loader.set_gpu(device.clone(), queue.clone());
        *self.renderer.borrow_mut() = Some(Renderer::new(device, queue));
        self.loader.reset_upload_failures();
        if self.scene.borrow().is_some() {
            self.frame.update(FrameState::with_upload_owed);
            self.request_redraw();
        }
    }

    // Release GPU-owning state before the graphics context goes away. The
    // scene (CPU-side) and the camera stay in the controller, so the next
    // `RenderingSetup` can re-upload without losing the user's view —
    // `handle_rendering_setup` flips the upload flag on its own when a
    // scene is present, so we don't set it here. The loader's gpu slot
    // is cleared too, so a click on Open between teardown and the next
    // setup is bounced by `begin_load` instead of leaving a worker to
    // build buffers against an unowned device.
    pub fn handle_rendering_teardown(&self) {
        self.loader.clear_gpu();
        *self.renderer.borrow_mut() = None;
        self.frame.update(FrameState::after_render);
    }

    pub fn handle_before_rendering(&self) {
        // Step the keyboard-focus integrator before borrowing the
        // renderer — the integrator mutates the scene's transform
        // and marks dirty, which is exactly the input the renderer
        // pass below needs to read in this same frame.
        self.integrate_keyboard_arrows();

        let mut renderer_slot = self.renderer.borrow_mut();
        let Some(renderer) = renderer_slot.as_mut() else {
            return;
        };

        if self.frame.get().upload_owed() {
            if let Some(scene_ref) = self.scene.borrow().as_ref() {
                match renderer.upload_scene(scene_ref) {
                    Ok(()) => {
                        self.frame.update(FrameState::after_upload_success);
                        self.loader.note_upload_success();
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
                        // Leave the upload flag set so it continues to
                        // represent "upload owed" — a subsequent renderer
                        // setup or scene reload (both of which reset the
                        // loader's failure counter) will drive a retry
                        // without relying on coincidental side effects.
                        self.loader.note_upload_failure(&err);
                        return;
                    }
                }
            } else {
                // Flag set but the scene was cleared before we got to it.
                self.frame.update(FrameState::upload_cleared);
            }
        }

        if !self.frame.get().redraw_owed() {
            return;
        }

        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(vp) = self.viewport.get() else {
            return;
        };
        let h_logical = vp.height();
        let scale = ui.window().scale_factor();
        let width = (vp.width() * scale).round() as u32;
        let height = (h_logical * scale).round() as u32;
        if width == 0 || height == 0 {
            return;
        }

        let (eff_w, eff_h) = render_scheduler::bucketed_size(width, height, self.resizing.get());

        let camera_slot = self.camera.borrow();
        let Some(camera) = camera_slot.as_ref() else {
            return;
        };
        let scene_slot = self.scene.borrow();
        let Some(scene_for_render) = scene_slot.as_ref() else {
            // Scene was cleared between the upload check and now.
            return;
        };

        let selection_idx = self.selection.primary_idx();
        let drag_state = self.gizmo_fsm.current();
        let (visibility, drag_visual) = match drag_state {
            GizmoState::Drag { op, pivot, .. } => match op {
                DragOp::Translate { axis, .. } => {
                    (GizmoVisibility::DraggingTranslate { axis }, None)
                }
                DragOp::Rotate { axis, last_raw, .. } => (
                    GizmoVisibility::DraggingRotate { axis },
                    Some(GizmoDragVisual::Rotate {
                        axis,
                        angle: last_raw,
                    }),
                ),
                DragOp::ScaleAxis {
                    axis,
                    current_anchor,
                    proportional,
                    ..
                } => {
                    // Proportional (Shift) is a uniform scale that
                    // doesn't have a per-axis cursor projection — the
                    // cube stays put and the object grows uniformly.
                    let drag_visual = if proportional {
                        None
                    } else {
                        let axial_distance = (current_anchor - pivot).dot(axis.world_dir());
                        Some(GizmoDragVisual::ScaleAxis {
                            axis,
                            axial_distance,
                        })
                    };
                    (GizmoVisibility::DraggingScaleAxis { axis }, drag_visual)
                }
            },
            GizmoState::Idle => (GizmoVisibility::Idle, None),
        };
        let hover = self.gizmo_fsm.current_hover();
        let gizmo_frame = selection_idx.and_then(|sel| {
            gizmo::frame::build_gizmo_frame(
                scene_for_render,
                camera,
                sel,
                h_logical,
                visibility,
                hover,
                drag_visual,
            )
        });

        let Some(texture) = renderer.render(
            scene_for_render,
            camera,
            selection_idx,
            gizmo_frame,
            eff_w,
            eff_h,
        ) else {
            return;
        };
        self.frame.update(FrameState::after_render);
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

#[cfg(test)]
mod selection_tests {
    use super::*;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};

    /// `PointerModifiers::default()` with all flags off. The selection
    /// tests don't exercise modifier-driven branches, so every press
    /// uses this.
    fn no_mods() -> PointerModifiers {
        PointerModifiers::default()
    }

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
        ctrl.clear_frame_state_for_test();
        ctrl.on_select_by_index(0);
        assert!(
            !ctrl.redraw_owed_for_test(),
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
        ctrl.on_pointer_down(50.0, 50.0, no_mods());
        ctrl.on_pointer_up(50.0, 50.0, no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn viewport_click_with_zero_viewport_is_a_no_op() {
        // Width/height of zero means we have no projection to build a
        // ray from. Bail rather than divide-by-zeroing.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(1));
        // viewport defaults to (0.0, 0.0)
        ctrl.on_pointer_down(0.0, 0.0, no_mods());
        ctrl.on_pointer_up(0.0, 0.0, no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn pointer_up_without_matching_down_is_ignored() {
        // Defensive: if Slint somehow sends an up without a down (a
        // press captured elsewhere, or our state out of sync), the
        // controller must not run the picker against an unset press.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(1));
        ctrl.on_pointer_up(50.0, 50.0, no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn cycle_with_no_selection_picks_first_object() {
        // Tab from "nothing selected" doubles as "select the first
        // thing". Both directions land on index 0 — the user's
        // mental model is "advance to the first item that exists".
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(3));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(0));

        ctrl.on_clear_selection();
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_forward_advances_through_objects_and_wraps() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(3));
        ctrl.on_select_by_index(0);
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(2));
        ctrl.on_selection_cycle(1);
        // Wraps from the end back to the start.
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_backward_advances_in_reverse_and_wraps() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(3));
        ctrl.on_select_by_index(0);
        // From index 0, Shift+Tab wraps to the last object.
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(2));
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_on_empty_scene_is_a_no_op() {
        let ctrl = ViewportController::new_for_test();
        // No scene installed.
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), None);

        // Empty scene installed.
        ctrl.install_scene_for_test(Scene {
            meta: None,
            resources: vec![],
            objects: vec![],
        });
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn cycle_with_single_object_stays_on_it() {
        // One-object scene → cycling either direction lands on
        // index 0 every time. Mod-N arithmetic with N=1 collapses
        // to 0 cleanly.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(1));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(0));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(0));
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_clamps_non_unit_deltas() {
        // A delta != ±1 from a stale Slint binding should still
        // advance by exactly one step (or none for delta == 0)
        // rather than indexing past the array.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(scene_with_n_objects(3));
        ctrl.on_select_by_index(0);
        ctrl.on_selection_cycle(99);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_selection_cycle(0);
        // delta == 0 → step is 0; selection is unchanged.
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_selection_cycle(-99);
        assert_eq!(ctrl.selection(), Some(0));
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

    /// `PointerModifiers::default()` — all four flags off.
    fn no_mods() -> PointerModifiers {
        PointerModifiers::default()
    }

    /// `PointerModifiers` with Shift held; other flags off.
    fn shift_held() -> PointerModifiers {
        PointerModifiers {
            shift: true,
            control: false,
            alt: false,
            meta: false,
        }
    }

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
        ctrl.set_viewport_for_test(200.0, 100.0);

        ctrl.on_pointer_down(100.0, 50.0, no_mods());
        ctrl.on_pointer_up(100.0, 50.0, no_mods());
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
        ctrl.set_viewport_for_test(200.0, 100.0);

        // Pre-seed a selection so we can observe the clear.
        ctrl.on_select_by_index(0);
        assert_eq!(ctrl.selection(), Some(0));

        // Click at the corner — no triangle there.
        ctrl.on_pointer_down(0.0, 0.0, no_mods());
        ctrl.on_pointer_up(0.0, 0.0, no_mods());
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
        ctrl.set_viewport_for_test(200.0, 100.0);

        // Press over the triangle, release 50 px away → drag.
        ctrl.on_pointer_down(100.0, 50.0, no_mods());
        ctrl.on_pointer_up(150.0, 50.0, no_mods());
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

    /// Width / height of the viewport used by the gizmo-drag tests.
    /// 400×200 is wide enough that the X-axis cone tip
    /// (`gizmo_scale × 1.18 ≈ 1.89` world units along +X with the
    /// front camera at distance 3) lands well inside the viewport.
    /// Smaller viewports clip the cone off-screen and make
    /// cone-tip presses impossible.
    const GIZMO_TEST_VIEWPORT: (f32, f32) = (400.0, 200.0);

    fn install_single_object_at_origin(ctrl: &ViewportController) {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![front_facing_triangle_at([0.0, 0.0, 0.0])],
        };
        ctrl.install_scene_for_test(scene);
        *ctrl.camera.borrow_mut() = Some(front_camera());
        let (vw, vh) = GIZMO_TEST_VIEWPORT;
        ctrl.set_viewport_for_test(vw, vh);
        ctrl.on_select_by_index(0);
    }

    /// Logical-pixel position of a click on the X-axis cone tip.
    /// Computed for the gizmo test setup (front camera, 400×200
    /// viewport, default FOV, scene radius 1.0): the cone occupies
    /// `screen-x ∈ [320, 350]`, screen-y ≈ 100. `(335, 100)` lands
    /// safely inside.
    const X_CONE_PRESS: (f32, f32) = (335.0, 100.0);

    fn read_translation(ctrl: &ViewportController) -> Vec3 {
        let scene_slot = ctrl.scene.borrow();
        let scene = scene_slot.as_ref().unwrap();
        match scene.objects[0].transform {
            Transform3D::Identity => Vec3::ZERO,
            Transform3D::Affine(a) => Vec3::new(
                a.translation()[0] as f32,
                a.translation()[1] as f32,
                a.translation()[2] as f32,
            ),
        }
    }

    fn read_linear(ctrl: &ViewportController) -> [[f64; 3]; 3] {
        let scene_slot = ctrl.scene.borrow();
        let scene = scene_slot.as_ref().unwrap();
        match scene.objects[0].transform {
            Transform3D::Identity => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            Transform3D::Affine(a) => *a.linear(),
        }
    }

    /// Project the X-axis midpoint cube's centre to logical-pixel
    /// coords for the standard test setup. The cube sits at
    /// `pivot + axis_world * (scale * MIDPOINT_FRACTION)`; with the
    /// front camera, that lands on screen-y = viewport-centre.
    fn x_midpoint_press_screen(ctrl: &ViewportController) -> (f32, f32) {
        let cam = ctrl.camera.borrow().as_ref().unwrap().clone();
        let (vw, vh) = ctrl.viewport_for_test().unwrap();
        let fov_y = default_fov_y();
        let scale = gizmo::gizmo_scale(&cam, fov_y, vh);
        let midpoint = Vec3::new(scale * gizmo::dimensions::MIDPOINT_FRACTION, 0.0, 0.0);
        picking::project_world_to_screen(&cam, fov_y, vw, vh, midpoint).unwrap()
    }

    #[test]
    fn pointer_down_on_x_handle_enters_drag_state() {
        // The X cone in the front-camera + 400×200 setup lives at
        // screen-x ∈ [320, 350] / screen-y = 100. `X_CONE_PRESS` is
        // a press inside that region; only the cone tip is the
        // active translate handle (the shaft line is decorative).
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        match ctrl.gizmo_fsm.current() {
            GizmoState::Drag {
                op: DragOp::Translate { axis, .. },
                ..
            } => assert_eq!(axis, Axis::X),
            other => panic!("expected Drag(Translate, X), got {other:?}"),
        }
    }

    #[test]
    fn pointer_down_off_handle_does_not_enter_drag_state() {
        // A press far from any handle lands as a plain click
        // candidate — the FSM stays Idle.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(0.0, 0.0, no_mods());
        assert!(matches!(ctrl.gizmo_fsm.current(), GizmoState::Idle));
    }

    #[test]
    fn drag_translates_object_along_world_axis() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        // Press lands on the X cone tip; release stays on the cone
        // (close enough that the closest-point-on-axis delta is the
        // controller's only output to verify).
        let press = X_CONE_PRESS;
        let release = (X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1);
        let (vw, vh) = GIZMO_TEST_VIEWPORT;
        let cam = ctrl.camera.borrow().as_ref().unwrap().clone();
        let fov_y = default_fov_y();
        let press_ray = picking::ray_from_pixel(&cam, fov_y, vw, vh, press.0, press.1);
        let release_ray = picking::ray_from_pixel(&cam, fov_y, vw, vh, release.0, release.1);
        let press_anchor = gizmo::drag_math::closest_point_on_axis(&press_ray, Vec3::ZERO, Axis::X);
        let release_anchor =
            gizmo::drag_math::closest_point_on_axis(&release_ray, Vec3::ZERO, Axis::X);
        let expected = release_anchor - press_anchor;

        ctrl.on_pointer_down(press.0, press.1, no_mods());
        // Drag must have started — otherwise the move is a no-op.
        assert!(matches!(ctrl.gizmo_fsm.current(), GizmoState::Drag { .. }));
        ctrl.on_pointer_move(release.0, release.1, no_mods());

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

        let press = X_CONE_PRESS;
        let mid = (X_CONE_PRESS.0 + 3.0, X_CONE_PRESS.1);
        let release = (X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1);
        let (vw, vh) = GIZMO_TEST_VIEWPORT;
        let cam = ctrl.camera.borrow().as_ref().unwrap().clone();
        let fov_y = default_fov_y();
        let press_anchor = gizmo::drag_math::closest_point_on_axis(
            &picking::ray_from_pixel(&cam, fov_y, vw, vh, press.0, press.1),
            Vec3::ZERO,
            Axis::X,
        );
        let release_anchor = gizmo::drag_math::closest_point_on_axis(
            &picking::ray_from_pixel(&cam, fov_y, vw, vh, release.0, release.1),
            Vec3::ZERO,
            Axis::X,
        );
        let expected = release_anchor - press_anchor;

        ctrl.on_pointer_down(press.0, press.1, no_mods());
        ctrl.on_pointer_move(mid.0, mid.1, no_mods());
        ctrl.on_pointer_move(release.0, release.1, no_mods());

        let actual = read_translation(&ctrl);
        assert_approx_eq!(f32, actual.x, expected.x, epsilon = 1e-4);
    }

    #[test]
    fn pointer_up_after_drag_clears_state_and_keeps_translation() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_move(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());
        let mid_drag = read_translation(&ctrl);
        ctrl.on_pointer_up(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());

        // Drag state cleared.
        assert!(matches!(ctrl.gizmo_fsm.current(), GizmoState::Idle));
        // Translation persists — the user committed the drag.
        let after = read_translation(&ctrl);
        assert_approx_eq!(f32, after.x, mid_drag.x, epsilon = 1e-6);
    }

    #[test]
    fn esc_mid_drag_reverts_transform() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_move(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());
        // Object has moved.
        assert!(read_translation(&ctrl).x.abs() > 1e-4);

        // First Esc cascades into the drag-abort: transform reverts,
        // FSM returns to Idle, but the selection survives (Esc only
        // drops one level per press).
        ctrl.on_clear_selection();
        let reverted = read_translation(&ctrl);
        assert_approx_eq!(f32, reverted.x, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, reverted.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, reverted.z, 0.0, epsilon = 1e-6);
        assert!(matches!(ctrl.gizmo_fsm.current(), GizmoState::Idle));
        assert_eq!(
            ctrl.selection(),
            Some(0),
            "first Esc only aborts the drag — selection is the next level of the cascade"
        );

        // A second Esc, with no drag and no keyboard focus, clears
        // the selection (Phase B behaviour).
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn close_scene_mid_drag_does_not_panic() {
        // The drag captures `object_idx`, which becomes invalid when
        // the scene is dropped. `on_close_scene` must clear FSM
        // state without trying to revert through the missing scene.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_move(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_close_scene();

        assert!(matches!(ctrl.gizmo_fsm.current(), GizmoState::Idle));
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
        ctrl.set_viewport_for_test(200.0, 100.0);

        ctrl.on_pointer_down(100.0, 50.0, no_mods());
        ctrl.on_pointer_up(101.5, 51.5, no_mods());
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn pressing_axis_cube_with_shift_held_starts_proportional_drag() {
        // Pressing the per-axis midpoint cube while Shift is already
        // held captures `proportional = true` immediately — the very
        // first move applies the uniform formula, no toggle needed.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);
        ctrl.on_pointer_down(press.0, press.1, shift_held());
        match ctrl.gizmo_fsm.current() {
            GizmoState::Drag {
                op: DragOp::ScaleAxis {
                    axis, proportional, ..
                },
                ..
            } => {
                assert_eq!(axis, Axis::X);
                assert!(
                    proportional,
                    "proportional must capture from press-time Shift state"
                );
            }
            other => panic!("expected Drag(ScaleAxis, X, proportional=true), got {other:?}"),
        }
    }

    #[test]
    fn pressing_axis_cube_without_shift_starts_per_axis_drag() {
        // Sanity check the inverse: no Shift → per-axis (proportional
        // = false). Pins the default so a future regression that
        // captures stale modifier state surfaces here.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);
        ctrl.on_pointer_down(press.0, press.1, no_mods());
        match ctrl.gizmo_fsm.current() {
            GizmoState::Drag {
                op: DragOp::ScaleAxis { proportional, .. },
                ..
            } => assert!(
                !proportional,
                "no Shift at press → proportional must be false"
            ),
            other => panic!("expected Drag(ScaleAxis, .., proportional=false), got {other:?}"),
        }
    }

    #[test]
    fn shift_toggle_during_axis_scale_flips_to_uniform_formula() {
        // Press without Shift → per-axis drag, move to grow X. Then
        // press Shift on a subsequent move: re-anchor (running
        // transform preserved). A *further* move with Shift held
        // exercises the uniform formula — its `diag(f, f, f)`
        // composes onto the captured running transform, so Y and Z
        // both pick up the same `f` factor while X picks up an extra
        // `f` on top of the per-axis stretch baked in by the first
        // move.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);

        ctrl.on_pointer_down(press.0, press.1, no_mods());
        let move_no_shift = (press.0 + 20.0, press.1);
        ctrl.on_pointer_move(move_no_shift.0, move_no_shift.1, no_mods());
        let m_axis = read_linear(&ctrl);
        assert!(
            m_axis[0][0] > 1.0 + 1e-3,
            "X should have grown, got m[0][0]={}",
            m_axis[0][0]
        );
        assert_approx_eq!(f64, m_axis[1][1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m_axis[2][2], 1.0, epsilon = 1e-6);

        // Shift toggles on at the same cursor position — re-anchors
        // without changing the running transform.
        let toggle = (move_no_shift.0, move_no_shift.1);
        ctrl.on_pointer_move(toggle.0, toggle.1, shift_held());

        // Move further with Shift held — uniform formula runs against
        // the re-anchored press at `toggle`.
        let further = (toggle.0 + 20.0, toggle.1);
        ctrl.on_pointer_move(further.0, further.1, shift_held());
        let m_uniform = read_linear(&ctrl);

        // Uniform formula → Y and Z scale by the same factor.
        assert_approx_eq!(f64, m_uniform[1][1], m_uniform[2][2], epsilon = 1e-4);
        // Both moved away from 1.0 (the formula did something).
        assert!(
            (m_uniform[1][1] - 1.0).abs() > 1e-3,
            "Y should have scaled under uniform formula, got m[1][1]={}",
            m_uniform[1][1]
        );
        // X carries the per-axis stretch on top of the uniform factor,
        // so it differs from Y/Z.
        assert!(
            (m_uniform[0][0] - m_uniform[1][1]).abs() > 1e-3,
            "X should differ from Y under uniform-after-per-axis: m[0][0]={} m[1][1]={}",
            m_uniform[0][0],
            m_uniform[1][1]
        );
    }

    #[test]
    fn shift_toggle_does_not_snap_running_transform() {
        // Re-anchoring on a Shift toggle must preserve the running
        // transform on the toggling frame: the scene's transform
        // immediately after the toggle move equals the running
        // transform from immediately before.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);

        ctrl.on_pointer_down(press.0, press.1, no_mods());
        let move1 = (press.0 + 20.0, press.1);
        ctrl.on_pointer_move(move1.0, move1.1, no_mods());
        let before_toggle = read_linear(&ctrl);
        let translation_before = read_translation(&ctrl);

        // Same cursor position, Shift toggles on. Re-anchor must
        // preserve the running transform — a "toggle without cursor
        // motion" is the easiest snap-bug to write a test for.
        ctrl.on_pointer_move(move1.0, move1.1, shift_held());
        let after_toggle = read_linear(&ctrl);
        let translation_after = read_translation(&ctrl);

        for r in 0..3 {
            for c in 0..3 {
                assert_approx_eq!(f64, after_toggle[r][c], before_toggle[r][c], epsilon = 1e-4);
            }
        }
        assert_approx_eq!(
            f32,
            translation_after.x,
            translation_before.x,
            epsilon = 1e-4
        );
        assert_approx_eq!(
            f32,
            translation_after.y,
            translation_before.y,
            epsilon = 1e-4
        );
        assert_approx_eq!(
            f32,
            translation_after.z,
            translation_before.z,
            epsilon = 1e-4
        );
    }

    #[test]
    fn shift_toggle_off_returns_to_per_axis_formula() {
        // Press with Shift held → uniform. Move once. Release Shift on
        // the next move (re-anchor again) → per-axis formula should
        // resume on subsequent moves: only X scales relative to the
        // new anchor, while Y and Z stay at the value frozen at the
        // toggle.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);

        // Start uniform.
        ctrl.on_pointer_down(press.0, press.1, shift_held());
        let move1 = (press.0 + 20.0, press.1);
        ctrl.on_pointer_move(move1.0, move1.1, shift_held());

        // Release Shift — re-anchor. Frozen Y/Z factors persist.
        ctrl.on_pointer_move(move1.0, move1.1, no_mods());
        let frozen = read_linear(&ctrl);

        // Move further along X — only m[0][0] should change.
        let move2 = (move1.0 + 10.0, move1.1);
        ctrl.on_pointer_move(move2.0, move2.1, no_mods());
        let after = read_linear(&ctrl);

        assert!(
            (after[0][0] - frozen[0][0]).abs() > 1e-3,
            "X should have scaled further: frozen={} after={}",
            frozen[0][0],
            after[0][0]
        );
        assert_approx_eq!(f64, after[1][1], frozen[1][1], epsilon = 1e-4);
        assert_approx_eq!(f64, after[2][2], frozen[2][2], epsilon = 1e-4);
    }
}

#[cfg(test)]
mod keyboard_focus_tests {
    use super::*;
    use glam::Vec3;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};

    fn one_object_scene() -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(IndexedMesh {
                    meta: None,
                    vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
                    faces: vec![],
                }),
            }],
        }
    }

    #[test]
    fn focus_requires_an_existing_selection() {
        // No selection → T is a no-op. Pinning this matters because
        // the integrator dereferences `selection`; a focus without a
        // selection would crash.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_gizmo_focus_translate();
        assert_eq!(ctrl.kbd_drive.current_focus(), KbdFocus::None);
    }

    #[test]
    fn focus_set_with_selection_takes_effect() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        assert_eq!(ctrl.kbd_drive.current_focus(), KbdFocus::Translate);
        ctrl.on_gizmo_focus_rotate();
        assert_eq!(ctrl.kbd_drive.current_focus(), KbdFocus::Rotate);
        ctrl.on_gizmo_focus_scale();
        assert_eq!(ctrl.kbd_drive.current_focus(), KbdFocus::Scale);
    }

    #[test]
    fn switching_focus_resets_held_arrow_bitset() {
        // Holding +X under translate focus, then switching to rotate
        // focus must clear the held bit — the user's mental model
        // is "release on mode switch".
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        assert!(ctrl.kbd_drive.current_arrows().any_held());

        ctrl.on_gizmo_focus_rotate();
        assert!(
            !ctrl.kbd_drive.current_arrows().any_held(),
            "switching focus must clear held arrows so a held key from one mode \
             doesn't leak into another"
        );
    }

    #[test]
    fn arrow_press_without_focus_is_a_no_op() {
        // Without focus, an arrow code is silently ignored — the
        // FocusScope on the Slint side filters them out, but the
        // Rust handler still has to be defensive in case the
        // contract is ever violated.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        assert!(!ctrl.kbd_drive.current_arrows().any_held());
    }

    #[test]
    fn arrow_release_clears_bit_even_with_focus_dropped() {
        // A release that arrives after focus was dropped (race with
        // Esc) must still clear the bit — otherwise the next focus
        // session would inherit a phantom held key.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        // Focus drop happens (e.g. via Esc cascade).
        ctrl.on_clear_selection();
        // Stale release arrives.
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);
        assert!(!ctrl.kbd_drive.current_arrows().any_held());
    }

    #[test]
    fn esc_cascade_drag_then_focus_then_selection() {
        // Each Esc drops one level of the cascade.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        // No drag is possible without a camera; simulate the
        // mid-cascade state by directly writing the gizmo cell.
        ctrl.gizmo_fsm.force_set(GizmoState::Drag {
            op: DragOp::Translate {
                axis: Axis::X,
                anchor: Vec3::ZERO,
            },
            original: Transform3D::Identity,
            pivot: Vec3::ZERO,
            object_idx: 0,
        });

        // Level 1 → drag aborts; focus and selection survive.
        ctrl.on_clear_selection();
        assert!(matches!(ctrl.gizmo_fsm.current(), GizmoState::Idle));
        assert_eq!(ctrl.kbd_drive.current_focus(), KbdFocus::Translate);
        assert_eq!(ctrl.selection(), Some(0));

        // Level 2 → focus drops; selection survives.
        ctrl.on_clear_selection();
        assert_eq!(ctrl.kbd_drive.current_focus(), KbdFocus::None);
        assert_eq!(ctrl.selection(), Some(0));

        // Level 3 → selection clears.
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }
}
