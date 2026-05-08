/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[cfg(test)]
use sc_mesh_core::scene::Scene;
use slint::ComponentHandle;
use slint::wgpu_28::wgpu;

use super::gizmo_drag::{DragOp, GizmoState};
use super::gizmo_fsm::{
    GizmoFsm, HoverCtx as GizmoHoverCtx, MoveCtx as GizmoMoveCtx, StartCtx as GizmoStartCtx,
};
use super::input_state::{ClickOrDrag, InputState};
#[cfg(test)]
use super::keyboard_drive::{ARROW_X_POS, ARROW_Y_POS};
use super::keyboard_drive::{IntegrationCtx, KbdFocus, KeyboardDriveState};
use super::object_list::{build_object_list_names, push_object_list};
use super::render_scheduler;
use super::types::{FrameState, ScreenPoint, ViewportSize};
use crate::MainWindow;
use crate::PointerModifiers;
use crate::build_volume::{BuildVolume, HARDCODED_DEFAULT_BUILD_VOLUME};
use crate::camera::{AxisView, OrbitCamera};
use crate::error_dialog::DialogManager;
use crate::gizmo;
use crate::i18n::ctr;
use crate::mesh_loader::start_load_with_ticket;
use crate::mesh_render::{GizmoDragVisual, GizmoVisibility, Renderer, SceneRender, default_fov_y};
use crate::pending_load::PendingLoad;
use crate::picking;
#[cfg(test)]
use crate::scene_document::SceneEdit;
use crate::scene_document::{SceneDocument, SceneEvent, SceneSubscription};
use crate::scene_loader::{PreparedScene, SceneLoader};
use crate::selection::{ObjectIdx, Selection};

#[cfg(test)]
use crate::gizmo::axis::Axis;

pub struct ViewportController {
    ui: slint::Weak<MainWindow>,
    // Document-level model. Owns the loaded scene + selection (and,
    // ahead, the undo stack). The controller observes it.
    document: Rc<SceneDocument>,
    renderer: RefCell<Option<Renderer>>,
    // The camera lives next to the scene (not inside the GPU renderer), so it
    // survives device-loss / RenderingTeardown→Setup cycles for free and can
    // be manipulated without a GPU present.
    camera: RefCell<Option<OrbitCamera>>,
    viewport: Cell<Option<ViewportSize>>,
    // Per-frame work owed: a redraw (something visual changed) and/or
    // a GPU upload (CPU-side scene differs from the renderer's
    // buffers). Both can be set independently, a successful upload
    // also sets the redraw flag so the just-uploaded scene is shown,
    // and a redraw can be flagged alone (e.g. camera moved without a
    // scene change).
    frame: Cell<FrameState>,
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
    // Slot for a load request that arrived before the GPU device was
    // available (typically argv at startup, or future macOS `openURLs`
    // delivered before first paint). Shared with `WiredMainWindow`,
    // which is the writer; the controller drains it in
    // `handle_rendering_setup` once `set_gpu` makes the device
    // snapshottable. See `pending_load.rs` for the full lifecycle.
    pending_load: Rc<PendingLoad>,
    // True between a `viewport-resized` event and the debounce timer firing
    // in `on_resize_settled`. While set, `handle_before_rendering` rounds the
    // render target size up to the next `RESIZE_BUCKET` multiple so drag
    // resizes don't churn the MSAA/target allocations on every pointer move.
    resizing: Cell<bool>,
    dialogs: Rc<DialogManager>,
    // Document subscription. Held by the controller so it's dropped
    // alongside the controller; populated by `register_document_listener`
    // after the controller is wrapped in `Rc`.
    document_subscription: RefCell<Option<SceneSubscription>>,
    // Build envelope used to clamp translate edits. `Cell` so the
    // hardcoded default can later be swapped at runtime by the
    // preferences pipeline (and by tests, which inject a smaller
    // envelope to make the wall reachable in a few drag pixels).
    build_volume: Cell<BuildVolume>,
}

impl ViewportController {
    pub fn new(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        pending_load: Rc<PendingLoad>,
    ) -> Self {
        let dialogs = DialogManager::new();
        Self {
            ui: ui.as_weak(),
            document,
            renderer: RefCell::new(None),
            camera: RefCell::new(None),
            viewport: Cell::new(None),
            frame: Cell::new(FrameState::default()),
            input_state: InputState::new(),
            gizmo_fsm: GizmoFsm::new(),
            kbd_drive: KeyboardDriveState::new(),
            loader: Rc::new(SceneLoader::new(ui, dialogs.clone())),
            pending_load,
            resizing: Cell::new(false),
            dialogs,
            document_subscription: RefCell::new(None),
            build_volume: Cell::new(HARDCODED_DEFAULT_BUILD_VOLUME),
        }
    }

    /// Constructor for selection-FSM unit tests. The stored `Weak` is
    /// dangling, so every `ui.upgrade()` returns `None` and the
    /// UI-mirroring side effects (`set_selected_object_index`,
    /// `set_object_list`, `request_redraw`) silently skip, leaving the
    /// in-Rust selection cell and dirty flag as the only observable
    /// state. This matches `SceneLoader::new_for_test`.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        let dialogs = DialogManager::new();
        Self {
            ui: slint::Weak::default(),
            document: SceneDocument::new(),
            renderer: RefCell::new(None),
            camera: RefCell::new(None),
            viewport: Cell::new(None),
            frame: Cell::new(FrameState::default()),
            input_state: InputState::new(),
            gizmo_fsm: GizmoFsm::new(),
            kbd_drive: KeyboardDriveState::new(),
            loader: Rc::new(SceneLoader::new_for_test(dialogs.clone())),
            pending_load: Rc::new(PendingLoad::new()),
            resizing: Cell::new(false),
            dialogs,
            document_subscription: RefCell::new(None),
            build_volume: Cell::new(HARDCODED_DEFAULT_BUILD_VOLUME),
        }
    }

    #[cfg(test)]
    pub(crate) fn install_scene_for_test(&self, scene: Scene) {
        // Tests don't care about the source path; populate the slot
        // with a sentinel so the document's invariant ("scene present
        // ⇔ path present") holds and any future test assertion on
        // `source_path` is explicit about its synthetic origin.
        self.document
            .install_scene(scene, std::path::PathBuf::from("<test>"));
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

    /// Access to the document. Used by `wiring.rs` to drive the
    /// `Export As` flow (read scene + source path) without going
    /// through delegating methods on the controller.
    pub fn document(&self) -> Rc<SceneDocument> {
        self.document.clone()
    }

    /// Access to the dialog manager. Used by `wiring.rs` to surface
    /// errors that don't originate from a controller call (e.g. an
    /// async file-save dialog flow whose result needs an error
    /// dialog).
    pub fn dialogs(&self) -> Rc<DialogManager> {
        self.dialogs.clone()
    }

    /// Subscribes the controller to its document. Called from
    /// `app_state` once the controller is wrapped in `Rc`. The
    /// listener captures a `Weak<Self>` to avoid an `Rc` cycle through
    /// the document.
    pub fn register_document_listener(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let sub = self.document.subscribe(move |event| {
            if let Some(strong) = weak.upgrade() {
                strong.handle_scene_event(event);
            }
        });
        *self.document_subscription.borrow_mut() = Some(sub);
    }

    /// Translates a `SceneEvent` into the viewport-state reset that
    /// used to live inline in the lifecycle methods. Intended only as
    /// a listener body, public callers should drive the document
    /// directly and let the listener react.
    fn handle_scene_event(&self, event: SceneEvent) {
        match event {
            SceneEvent::Installed => self.on_scene_installed(),
            SceneEvent::Cleared => self.on_scene_cleared(),
            SceneEvent::SelectionChanged => {
                self.mirror_selection_to_ui();
                self.mark_dirty();
            }
            SceneEvent::Mutated => {
                // Every scene mutation routes through `apply_command`,
                // so this is the single place that schedules a redraw
                // for them.
                self.mark_dirty();
            }
            SceneEvent::UndoStackChanged => self.mirror_undo_state_to_ui(),
        }
    }

    fn mirror_undo_state_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_can_undo(self.document.can_undo());
            ui.set_can_redo(self.document.can_redo());
        }
    }

    /// Reset that follows a fresh scene install. Camera reset to
    /// `None` so the renderer-bounds framing branch in
    /// `on_scene_load_finished` can populate it; UI mirrors the new
    /// scene's object list and the freshly-cleared selection.
    fn on_scene_installed(&self) {
        *self.camera.borrow_mut() = None;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_has_scene(true);
            let names = self
                .document
                .with_scene(|spatial| build_object_list_names(spatial.scene()))
                .unwrap_or_default();
            push_object_list(&ui, names);
            ui.set_selected_object_index(-1);
            ui.set_gizmo_keyboard_focus_active(false);
        }
        self.mirror_undo_state_to_ui();
        self.mark_dirty();
    }

    /// Full viewport reset on scene close. Mirror of the inline body
    /// the close path used to carry. The drag's captured `object_idx`
    /// becomes meaningless once the scene is gone, so the FSM is
    /// reset without trying to revert the (now-missing) object's
    /// transform; same for the keyboard integrator's focus and
    /// held-arrow bitset.
    fn on_scene_cleared(&self) {
        *self.camera.borrow_mut() = None;
        self.frame.set(FrameState::default());
        self.gizmo_fsm.set_idle();
        self.gizmo_fsm.clear_hover();
        self.input_state.set_idle();
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
        self.mirror_undo_state_to_ui();
    }

    fn request_redraw(&self) {
        render_scheduler::request_redraw(&self.ui);
    }

    /// Replace the active build volume and trigger a redraw. The
    /// renderer's plate-buffer cache keys off the `BuildVolume`, so
    /// the next render rebuilds the floor + cage geometry against
    /// the new envelope automatically. Camera position is preserved
    /// (mid-session edits leave the user's view alone).
    ///
    /// Uses `mark_dirty`, not just `request_redraw`: the render path
    /// early-returns when `frame.redraw_owed` is false, so a bare
    /// window-poke isn't enough on its own.
    pub fn set_build_volume(&self, bv: BuildVolume) {
        if self.build_volume.get() == bv {
            return;
        }
        self.build_volume.set(bv);
        self.mark_dirty();
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
        // Loader-side invalidation is not driven by the document's
        // listener, it's coupled to the close *gesture*, not to the
        // scene state itself. The `clear_scene` call below fires
        // `SceneEvent::Cleared`, and the controller's listener
        // handles the full viewport-state reset (camera, frame,
        // gizmo, kbd, renderer, UI).
        self.loader.invalidate_and_clear();
        self.document.clear_scene();
    }

    pub fn on_scene_load_finished(&self) {
        let Some(prepared) = self.loader.take_finished_load() else {
            return;
        };
        let PreparedScene {
            spatial,
            gpu_scene,
            device: built_with,
            warnings: _,
            source_path,
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

        // Fires `SceneEvent::Installed`; the listener resets the
        // camera to `None` (so the bounds-based framing below sees a
        // fresh slot), pushes the new object list, and rearms the UI
        // mirror.
        self.document.install_spatial(spatial, source_path);

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
    }

    /// Selection, in the document's authoritative form. Tests use
    /// this to assert FSM transitions.
    #[cfg(test)]
    pub(crate) fn selection(&self) -> Option<usize> {
        self.document.selection_primary_idx()
    }

    /// Slint-side selection-changed callback. Out-of-bounds indices map
    /// to `None` (defensive, the UI shouldn't normally hand us one,
    /// but a stale callback can race with a scene reload). The Cell
    /// guard short-circuits the round-trip so the mirror-back to the UI
    /// doesn't fire `selection-changed` again.
    pub fn on_select_by_index(&self, idx: usize) {
        let scene_len = self
            .document
            .with_scene(|s| s.scene().objects.len())
            .unwrap_or(0);
        let new_sel = if idx < scene_len {
            Selection::One(ObjectIdx(idx))
        } else {
            Selection::None
        };
        // `set_selection` short-circuits on no-change and fires
        // `SceneEvent::SelectionChanged` only on actual transitions;
        // the listener takes care of mirror-to-UI + mark-dirty.
        self.document.set_selection(new_sel);
    }

    /// Tab / Shift+Tab selection cycling.
    ///
    /// - `delta == 1` advances forward (Tab); `delta == -1` advances
    ///   backward (Shift+Tab). Other deltas are clamped to ±1 so a
    ///   stale Slint binding can't drive larger jumps.
    /// - With no selection, either direction selects
    ///   `scene.objects[0]`, the same key thus serves "select the
    ///   first thing" and "advance to the next".
    /// - With a selection, the index wraps modulo `len`.
    /// - On an empty / missing scene, nothing happens.
    ///
    /// A side-effect of changing the selection through this path is
    /// that any in-progress mouse drag aborts (because the abort runs
    /// out of the same `on_clear_selection` cascade, but we don't
    /// invoke it here, since Tab on a different object should pick
    /// up the new selection cleanly without touching the running
    /// transform). The current implementation does not abort an
    /// in-progress drag on Tab; if that turns out to be wrong, the
    /// fix is to call `abort_gizmo_drag` here.
    pub fn on_selection_cycle(&self, delta: i32) {
        let scene_len = self
            .document
            .with_scene(|s| s.scene().objects.len())
            .unwrap_or(0);
        if scene_len == 0 {
            return;
        }
        let step = delta.signum();
        let next = match self.document.selection() {
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
    /// 3. Else → clear the selection.
    ///
    /// "Click on empty space" enters at level 3 (the picker has
    /// already dispatched here from `on_pointer_up` after determining
    /// no object was hit); Esc enters at level 1 and naturally
    /// cascades through repeated presses.
    /// Edit → Undo handler. Bounced if no edit is on the stack
    /// (which is what `can-undo` already gates in the menu, but the
    /// shortcut can race a freshly-emptied stack, the document's
    /// own check is the source of truth).
    pub fn on_undo_requested(&self) {
        self.document.undo();
    }

    /// Edit → Redo handler.
    pub fn on_redo_requested(&self) {
        self.document.redo();
    }

    pub fn on_clear_selection(&self) {
        if matches!(self.gizmo_fsm.current(), GizmoState::Drag { .. }) {
            self.abort_gizmo_drag();
            return;
        }
        if self.kbd_drive.current_focus() != KbdFocus::None {
            self.set_keyboard_focus(KbdFocus::None);
            return;
        }
        if matches!(self.document.selection(), Selection::None) {
            return;
        }
        // Selection going away kills the gizmo, so any cached hover
        // tint targets a handle that no longer renders. Clearing
        // before the document mutation keeps the order matching the
        // pre-listener implementation; `set_selection` then fires
        // `SelectionChanged` and the listener mirrors UI + dirty.
        self.gizmo_fsm.clear_hover();
        self.document.set_selection(Selection::None);
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
        let has_selection = !matches!(self.document.selection(), Selection::None);
        if !self
            .kbd_drive
            .try_set_focus(&self.document, focus, has_selection)
        {
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
        let sel = self.document.selection_primary_idx();
        if self.kbd_drive.update_arrow(&self.document, sel, code, true) {
            self.mark_dirty();
        }
    }

    /// Counterpart to `on_gizmo_arrow_pressed`. Released codes are
    /// always processed even with focus dropped, a release that
    /// arrives just after the focus was cleared (race with Esc
    /// → focus drop) still has to clear its bit so the bitset
    /// doesn't accumulate stuck flags.
    pub fn on_gizmo_arrow_released(&self, code: i32) {
        let sel = self.document.selection_primary_idx();
        if self
            .kbd_drive
            .update_arrow(&self.document, sel, code, false)
        {
            self.mark_dirty();
        }
    }

    /// Left button pressed at logical coords `(x, y)`. Records the
    /// press position so `on_pointer_up` can disambiguate click from
    /// drag. If a gizmo handle is selected and the press hits it,
    /// the FSM also enters `Drag`, capturing the original transform
    /// and the world-space anchor on the axis. The modifier state
    /// at press feeds the `ScaleAxis` proportional flag, pressing
    /// the per-axis cube with Shift already held starts the drag in
    /// uniform-scale mode.
    pub fn on_pointer_down(&self, x: f32, y: f32, modifiers: PointerModifiers) {
        self.input_state.set_pressed(ScreenPoint { x, y });
        self.try_start_gizmo_drag(x, y, modifiers);
    }

    /// Pointer moved while the left button is held and no camera
    /// gesture is active. Drives the gizmo drag, branches on the
    /// active `DragOp`, recomputes the per-op delta from the current
    /// cursor ray, and composes the resulting transform onto the
    /// captured original. If a `ScaleAxis` drag is live and the live
    /// Shift state has flipped since the last frame, re-anchors the
    /// drag (current transform → new `original`, current cursor →
    /// new anchors) so the per-axis ↔ uniform formula switch is
    /// visually snap-free.
    pub fn on_pointer_move(&self, x: f32, y: f32, modifiers: PointerModifiers) {
        let ctx = GizmoMoveCtx {
            document: &self.document,
            camera: &self.camera,
            viewport: self.viewport.get(),
            cursor: ScreenPoint { x, y },
            modifiers,
            build_volume: self.build_volume.get(),
        };
        // A successful drag move routes through `apply_command`, which
        // fires `Mutated` and schedules the redraw via the listener;
        // the bool return is intentionally discarded here.
        self.gizmo_fsm.on_move(ctx);
        // No-op while a drag is in progress (the FSM short-circuits
        // hover updates internally), so this just covers the hover-
        // tint update on idle pointer moves. Hover state is not a
        // scene mutation, so it stays on the manual dirty path.
        let hover_ctx = GizmoHoverCtx {
            document: &self.document,
            camera: &self.camera,
            viewport: self.viewport.get(),
            selection_idx: self.document.selection_primary_idx(),
            cursor: ScreenPoint { x, y },
        };
        if self.gizmo_fsm.update_hover(hover_ctx) {
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
        if self.gizmo_fsm.commit_drag(&self.document) {
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
    /// `ScaleAxis.proportional` flag, a press on the per-axis cube
    /// with Shift held starts uniformly; subsequent toggles
    /// re-anchor through the move path.
    fn try_start_gizmo_drag(&self, x: f32, y: f32, modifiers: PointerModifiers) {
        let ctx = GizmoStartCtx {
            document: &self.document,
            camera: &self.camera,
            viewport: self.viewport.get(),
            selection_idx: self.document.selection_primary_idx(),
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
        // The revert routes through `apply_command` and the `Mutated`
        // listener schedules the redraw; only the input-phase reset
        // is the controller's job.
        if self.gizmo_fsm.abort(&self.document) {
            self.input_state.set_idle();
        }
    }

    /// Per-frame integrator: if a keyboard gizmo focus is active and
    /// any arrow key is held, applies one tick of motion to the
    /// selected object. Bails (without resetting the tick clock) on
    /// missing camera / scene / selection / zero viewport so a hot
    /// re-init doesn't hand back time the user didn't drive.
    ///
    /// A successful tick routes through `apply_command` and the
    /// `Mutated` listener marks the controller dirty so the next
    /// `handle_before_rendering` runs again, the integrator
    /// self-pumps as long as the user holds keys, and quiesces when
    /// they release.
    fn integrate_keyboard_arrows(&self) {
        let ctx = IntegrationCtx {
            document: &self.document,
            camera: &self.camera,
            viewport: self.viewport.get(),
            selection_idx: self.document.selection_primary_idx(),
            build_volume: self.build_volume.get(),
        };
        self.kbd_drive.integrate(ctx);
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
            let ray =
                picking::ray_from_pixel(camera, default_fov_y(), vp.width(), vp.height(), x, y);
            // Both borrows (camera_slot here, document scene inside
            // `with_scene`) drop at this expression's end, so the
            // selection setters below can re-borrow without overlap.
            self.document
                .with_scene(|scene| picking::pick_object(&ray, scene))
        };
        match pick {
            None => (),
            Some(Some(idx)) => self.on_select_by_index(idx),
            Some(None) => self.on_clear_selection(),
        }
    }

    fn mirror_selection_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let idx = match self.document.selection() {
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
        if self.document.is_loaded() {
            self.frame.update(FrameState::with_upload_owed);
        }
        // Always request a redraw after RenderingSetup: even with no
        // scene, the plate is part of the steady-state output.
        self.request_redraw();
        // The device is now snapshottable; drain any path that was
        // queued before this point (argv at startup, etc.).
        // `begin_load` returning `None` means a load is somehow already
        // in flight on this fresh setup, extremely unusual but
        // possible after a teardown→setup cycle that races a click on
        // Open; in that case the queued path is dropped, matching the
        // "drop while busy" decision for drag-and-drop.
        if let Some(path) = self.pending_load.take()
            && let Some(ticket) = self.loader.begin_load()
        {
            start_load_with_ticket(ticket, path, &self.loader);
        }
    }

    // Release GPU-owning state before the graphics context goes away. The
    // scene (CPU-side) and the camera stay in the controller, so the next
    // `RenderingSetup` can re-upload without losing the user's view,
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
        // renderer, the integrator mutates the scene's transform
        // and marks dirty, which is exactly the input the renderer
        // pass below needs to read in this same frame.
        self.integrate_keyboard_arrows();

        let mut renderer_slot = self.renderer.borrow_mut();
        let Some(renderer) = renderer_slot.as_mut() else {
            return;
        };

        if self.frame.get().upload_owed() {
            let upload = self
                .document
                .with_scene(|spatial| renderer.upload_scene(spatial.scene()));
            match upload {
                Some(Ok(())) => {
                    self.frame.update(FrameState::after_upload_success);
                    self.loader.note_upload_success();
                }
                Some(Err(err)) => {
                    // Leave the upload flag set so it continues to
                    // represent "upload owed", a subsequent renderer
                    // setup or scene reload (both of which reset the
                    // loader's failure counter) will drive a retry
                    // without relying on coincidental side effects.
                    self.loader.note_upload_failure(&err);
                    return;
                }
                None => {
                    // Flag set but the scene was cleared before we got to it.
                    self.frame.update(FrameState::upload_cleared);
                }
            }
        }

        // Initialise the orbit camera if it isn't yet. With the plate
        // visible at first paint, framing comes from the union of the
        // scene's world AABB (when uploaded) and the build envelope.
        // Skipped if a camera already exists (re-upload across a
        // RenderingTeardown -> Setup cycle preserves the user's view).
        if self.camera.borrow().is_none() {
            let (centre, radius) = self.build_volume.get().framing(renderer.scene_bounds());
            *self.camera.borrow_mut() = Some(OrbitCamera::new(centre, radius));
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

        let build_volume = self.build_volume.get();

        // Render path. With a scene installed, the closure inside
        // `with_scene` builds the gizmo frame against the live scene
        // and renders the union (plate + objects). With no scene,
        // `with_scene` returns `None` and the fallback below renders
        // the plate alone with neither selection nor gizmo.
        let texture = self
            .document
            .with_scene(|spatial_for_render| {
                let selection_idx = self.document.selection_primary_idx();
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
                            // Proportional (Shift) is a uniform scale
                            // that doesn't have a per-axis cursor
                            // projection, the cube stays put and the
                            // object grows uniformly.
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
                        spatial_for_render,
                        camera,
                        sel,
                        h_logical,
                        visibility,
                        hover,
                        drag_visual,
                    )
                });

                renderer.render(
                    Some(SceneRender {
                        scene: spatial_for_render.scene(),
                        selection: selection_idx,
                        gizmo: gizmo_frame,
                    }),
                    build_volume,
                    camera,
                    eff_w,
                    eff_h,
                )
            })
            .unwrap_or_else(|| renderer.render(None, build_volume, camera, eff_w, eff_h));

        let Some(texture) = texture else {
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
        // The scene cell is cleared too, a follow-up select must not
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
        // thing". Both directions land on index 0, the user's
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

    /// `PointerModifiers::default()`, all four flags off.
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
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
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
        // A 200×100 viewport with the default gizmo size leaves no
        // truly-empty pixel: HANDLE_PIXEL_LENGTH is 120 px, so the
        // rotate-cone bounding volumes (perspective-projected close
        // to the camera in Top view) span most of the screen. Use a
        // larger viewport so the gizmo occupies its usual ~15% and
        // the corner is unambiguously empty.
        ctrl.set_viewport_for_test(800.0, 600.0);

        // Pre-seed a selection so we can observe the clear.
        ctrl.on_select_by_index(0);
        assert_eq!(ctrl.selection(), Some(0));

        // Click at the corner, no triangle there.
        ctrl.on_pointer_down(0.0, 0.0, no_mods());
        ctrl.on_pointer_up(0.0, 0.0, no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn drag_does_not_run_the_picker() {
        // A press → release with movement larger than the click
        // threshold is a drag, which is currently dropped on the
        // floor (the gizmo FSM will consume it once wired). It must
        // not hand off to the picker, that would let large pointer
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
    /// of the viewport, clicks at `(centre_x + Δ, centre_y)` land on
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
        ctrl.document
            .with_scene(|spatial| match spatial.scene().objects[0].transform {
                Transform3D::Identity => Vec3::ZERO,
                Transform3D::Affine(a) => Vec3::new(
                    a.translation()[0] as f32,
                    a.translation()[1] as f32,
                    a.translation()[2] as f32,
                ),
            })
            .unwrap()
    }

    fn read_linear(ctrl: &ViewportController) -> [[f64; 3]; 3] {
        ctrl.document
            .with_scene(|spatial| match spatial.scene().objects[0].transform {
                Transform3D::Identity => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                Transform3D::Affine(a) => *a.linear(),
            })
            .unwrap()
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
        // candidate, the FSM stays Idle.
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
        // Drag must have started, otherwise the move is a no-op.
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
        // distinction matters once rotation and scale ride the same
        // drag pipeline; pinning it here means a bug in that
        // direction trips this test loudly.
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
        // Translation persists, the user committed the drag.
        let after = read_translation(&ctrl);
        assert_approx_eq!(f32, after.x, mid_drag.x, epsilon = 1e-6);
    }

    #[test]
    fn pointer_up_after_drag_pushes_one_undo_entry() {
        // A complete press → move → release gesture lands as one
        // entry on the document's undo stack, with the pre-press
        // transform as the `before`. A subsequent undo restores it.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_move(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_up(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());

        assert!(ctrl.document.can_undo());
        assert!(!ctrl.document.can_redo());

        let after_drag = read_translation(&ctrl);
        assert!(after_drag.x.abs() > 1e-4);

        assert!(ctrl.document.undo());
        let after_undo = read_translation(&ctrl);
        assert_approx_eq!(f32, after_undo.x, 0.0, epsilon = 1e-6);
        assert!(!ctrl.document.can_undo());
        assert!(ctrl.document.can_redo());

        assert!(ctrl.document.redo());
        let after_redo = read_translation(&ctrl);
        assert_approx_eq!(f32, after_redo.x, after_drag.x, epsilon = 1e-6);
    }

    #[test]
    fn pointer_up_with_no_movement_pushes_no_undo_entry() {
        // Press → release without a move tick: the transform is
        // unchanged, `push_edit` filters the no-op, the stack stays
        // empty.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_up(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());

        assert!(!ctrl.document.can_undo());
    }

    #[test]
    fn esc_mid_drag_pushes_no_undo_entry() {
        // Esc-to-cancel is not undoable, the user explicitly
        // cancelled the gesture, so nothing should land on the
        // history stack.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_move(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_clear_selection();

        assert!(!ctrl.document.can_undo());
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
            "first Esc only aborts the drag, selection is the next level of the cascade"
        );

        // A second Esc, with no drag and no keyboard focus, clears
        // the selection.
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn close_scene_mid_drag_does_not_panic() {
        // The drag captures `object_idx`, which becomes invalid when
        // the scene is dropped. `on_close_scene` must clear FSM
        // state without trying to revert through the missing scene.
        // Wrap in `Rc` and register the document listener so the
        // close-driven viewport reset (which now lives in the
        // listener body) actually runs in the test.
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.register_document_listener();
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
        // held captures `proportional = true` immediately, the very
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
        // exercises the uniform formula, its `diag(f, f, f)`
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

        // Shift toggles on at the same cursor position, re-anchors
        // without changing the running transform.
        let toggle = (move_no_shift.0, move_no_shift.1);
        ctrl.on_pointer_move(toggle.0, toggle.1, shift_held());

        // Move further with Shift held, uniform formula runs against
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
        // preserve the running transform, a "toggle without cursor
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

        // Release Shift, re-anchor. Frozen Y/Z factors persist.
        ctrl.on_pointer_move(move1.0, move1.1, no_mods());
        let frozen = read_linear(&ctrl);

        // Move further along X, only m[0][0] should change.
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

    /// Subscribes to the document and counts only `Mutated` events.
    /// Used by the routing tests below to filter out the
    /// `Installed`/`Cleared`/`SelectionChanged` traffic that
    /// `install_single_object_at_origin` and friends generate.
    fn subscribe_mutated_counter(doc: &Rc<SceneDocument>) -> (Rc<Cell<usize>>, SceneSubscription) {
        let count = Rc::new(Cell::new(0_usize));
        let count_for_listener = count.clone();
        let sub = doc.subscribe(move |event| {
            if matches!(event, SceneEvent::Mutated) {
                count_for_listener.set(count_for_listener.get() + 1);
            }
        });
        (count, sub)
    }

    #[test]
    fn drag_move_fires_one_mutated_event_per_pointer_move() {
        // Press the X cone to enter Drag, then a single move tick
        // that mutates the object's transform must fire exactly one
        // Mutated. Subscribing after the press skips the
        // SelectionChanged from `install_single_object_at_origin`.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());

        let (count, _sub) = subscribe_mutated_counter(&ctrl.document);
        ctrl.on_pointer_move(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn pointer_hover_without_drag_fires_no_mutated_event() {
        // No press, no drag, just pointer moves. Hover state can
        // change but the scene is not mutated, so no Mutated must
        // fire. Regression test for the `on_pointer_move`
        // simplification that split the gizmo and hover dirty
        // paths.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        let (count, _sub) = subscribe_mutated_counter(&ctrl.document);
        ctrl.on_pointer_move(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_move(X_CONE_PRESS.0 + 50.0, X_CONE_PRESS.1, no_mods());
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn gizmo_abort_with_drag_in_flight_fires_one_mutated_event() {
        // Press into Drag, subscribe, then abort the drag. The
        // revert routes through `apply_command`, which fires
        // Mutated exactly once.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        assert!(matches!(ctrl.gizmo_fsm.current(), GizmoState::Drag { .. }));

        let (count, _sub) = subscribe_mutated_counter(&ctrl.document);
        let aborted = ctrl.gizmo_fsm.abort(&ctrl.document);
        assert!(aborted);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn gizmo_abort_with_no_drag_fires_no_mutated_event() {
        // No drag in flight, abort short-circuits before touching
        // the document, so no Mutated must fire.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        assert!(matches!(ctrl.gizmo_fsm.current(), GizmoState::Idle));

        let (count, _sub) = subscribe_mutated_counter(&ctrl.document);
        let aborted = ctrl.gizmo_fsm.abort(&ctrl.document);
        assert!(!aborted);
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn drag_move_marks_redraw_owed_via_mutated_listener() {
        // End-to-end: with `register_document_listener` installed,
        // a drag move fires Mutated and the controller's listener
        // marks the controller dirty. This is the chain that
        // replaced the manual `mark_dirty` calls under each former
        // `with_scene_mut`.
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.register_document_listener();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.clear_frame_state_for_test();
        assert!(!ctrl.redraw_owed_for_test());

        ctrl.on_pointer_move(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1, no_mods());
        assert!(ctrl.redraw_owed_for_test());
    }

    #[test]
    fn set_build_volume_updates_field_and_marks_redraw_owed() {
        // The render path gates on `frame.redraw_owed`; without the
        // dirty flag the next `before-rendering` tick early-returns
        // and the new envelope only paints when something else
        // (camera rotate, pointer move) happens to mark the frame.
        let ctrl = ViewportController::new_for_test();
        let initial = ctrl.build_volume.get();
        let new_bv = BuildVolume::new(50.0, 50.0, 50.0);
        assert_ne!(initial, new_bv);
        ctrl.clear_frame_state_for_test();
        assert!(!ctrl.redraw_owed_for_test());

        ctrl.set_build_volume(new_bv);

        assert_eq!(ctrl.build_volume.get(), new_bv);
        assert!(ctrl.redraw_owed_for_test());
    }

    #[test]
    fn set_build_volume_is_idempotent_for_equal_values() {
        // Same value twice: the second call is a no-op, the frame
        // stays clean. A subsequent real change re-marks dirty.
        let ctrl = ViewportController::new_for_test();
        let bv = BuildVolume::new(80.0, 80.0, 120.0);
        ctrl.set_build_volume(bv);
        ctrl.clear_frame_state_for_test();

        ctrl.set_build_volume(bv);
        assert_eq!(ctrl.build_volume.get(), bv);
        assert!(!ctrl.redraw_owed_for_test());

        let bigger = BuildVolume::new(150.0, 80.0, 120.0);
        ctrl.set_build_volume(bigger);
        assert_eq!(ctrl.build_volume.get(), bigger);
        assert!(ctrl.redraw_owed_for_test());
    }

    #[test]
    fn gizmo_translate_drag_clamps_at_plus_x_wall() {
        // The fixture's triangle has world.max.x = 0.5 with the
        // installed transform (Affine zero). Setting a build envelope
        // whose env.max.x = 0.5 leaves zero room on +X: any positive
        // drag delta must clamp to zero. Without the clamp wiring,
        // the same press + move would translate the object along +X
        // (covered by `drag_translates_object_along_world_axis`).
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.build_volume.set(BuildVolume::new(1.0, 4.0, 4.0));

        ctrl.on_pointer_down(X_CONE_PRESS.0, X_CONE_PRESS.1, no_mods());
        ctrl.on_pointer_move(X_CONE_PRESS.0 + 50.0, X_CONE_PRESS.1, no_mods());

        let actual = read_translation(&ctrl);
        assert_approx_eq!(f32, actual.x, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, actual.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, actual.z, 0.0, epsilon = 1e-6);
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
        // focus must clear the held bit, the user's mental model
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
        // Without focus, an arrow code is silently ignored, the
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
        // Esc) must still clear the bit, otherwise the next focus
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

    fn translation_x(dx: f64) -> Transform3D {
        Transform3D::Affine(
            sc_mesh_core::transform::AffineTransform3D::from_translation([dx, 0.0, 0.0]),
        )
    }

    #[test]
    fn keyboard_burst_release_pushes_one_undo_entry() {
        // First arrow arms the burst; intervening transform mutation
        // simulates one or more integrator ticks; release flushes
        // the burst as a single undo entry.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.document.apply_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: translation_x(2.0),
        });
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);

        assert!(ctrl.document.can_undo());
        assert!(ctrl.document.undo());
        let after_undo = ctrl
            .document
            .with_scene(|s| s.scene().objects[0].transform)
            .unwrap();
        assert_eq!(after_undo, Transform3D::Identity);
    }

    #[test]
    fn keyboard_burst_with_no_motion_pushes_no_entry() {
        // Press → release without any integration tick: scene
        // transform never changed, push_edit's no-op filter drops
        // the entry.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);

        assert!(!ctrl.document.can_undo());
    }

    #[test]
    fn keyboard_burst_focus_change_flushes_pending_burst() {
        // Switching from translate to rotate mid-burst commits the
        // current run as one undo entry rather than discarding it;
        // the user's translate work is preserved.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.document.apply_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: translation_x(3.0),
        });
        // User switches mode without releasing the key.
        ctrl.on_gizmo_focus_rotate();

        assert!(ctrl.document.can_undo());
        ctrl.document.undo();
        let after_undo = ctrl
            .document
            .with_scene(|s| s.scene().objects[0].transform)
            .unwrap();
        assert_eq!(after_undo, Transform3D::Identity);
    }

    #[test]
    fn keyboard_burst_two_keys_held_is_a_single_undo_entry() {
        // Two arrows pressed then released independently is still
        // one burst, only the first-press transition arms the
        // tracker, only the all-released transition flushes it.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.on_gizmo_arrow_pressed(ARROW_Y_POS);
        ctrl.document.apply_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: translation_x(1.0),
        });
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);
        // Bitset still has Y_POS held, must not flush yet.
        assert!(!ctrl.document.can_undo());
        ctrl.on_gizmo_arrow_released(ARROW_Y_POS);
        assert!(ctrl.document.can_undo());
        // Exactly one entry: a second undo is rejected.
        assert!(ctrl.document.undo());
        assert!(!ctrl.document.can_undo());
    }

    #[test]
    fn keyboard_burst_dropped_on_scene_close() {
        // Scene close drops the burst without flushing, the object
        // index it captured is about to become meaningless.
        let ctrl = std::rc::Rc::new(ViewportController::new_for_test());
        ctrl.register_document_listener();
        ctrl.install_scene_for_test(one_object_scene());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.document.apply_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: translation_x(1.0),
        });

        ctrl.on_close_scene();
        assert!(!ctrl.document.can_undo());
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

    /// Single object whose local AABB has world.max.x = 1 with the
    /// identity transform. Useful for clamp tests where the goal is
    /// "envelope is exactly flush with the object's +X face, so any
    /// +X delta clamps to zero".
    fn cube_object_scene() -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(IndexedMesh {
                    meta: None,
                    vertices: vec![Vertex::new([-1.0, -1.0, 0.0]), Vertex::new([1.0, 1.0, 2.0])],
                    faces: vec![],
                }),
            }],
        }
    }

    /// Front-axis camera + 400 x 200 viewport. Mirrors the gizmo
    /// tests' setup so `world_per_pixel` is the same scale, and
    /// the keyboard integrator's translation rate is comparable.
    fn install_for_integration(ctrl: &ViewportController) {
        ctrl.install_scene_for_test(cube_object_scene());
        let mut cam = OrbitCamera::new(Vec3::ZERO, 1.0);
        cam.set_axis_view(crate::camera::AxisView::Front);
        *ctrl.camera.borrow_mut() = Some(cam);
        ctrl.set_viewport_for_test(400.0, 200.0);
    }

    /// Drive `kbd_drive.integrate` twice with a real-time pause
    /// between calls so the per-tick `dt` is non-zero on the second
    /// call. The first call seeds `last_tick`; the second produces
    /// the actual integration step.
    fn integrate_two_ticks(ctrl: &ViewportController) {
        let _ = ctrl.kbd_drive.integrate(IntegrationCtx {
            document: &ctrl.document,
            camera: &ctrl.camera,
            viewport: ctrl.viewport.get(),
            selection_idx: ctrl.document.selection_primary_idx(),
            build_volume: ctrl.build_volume.get(),
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = ctrl.kbd_drive.integrate(IntegrationCtx {
            document: &ctrl.document,
            camera: &ctrl.camera,
            viewport: ctrl.viewport.get(),
            selection_idx: ctrl.document.selection_primary_idx(),
            build_volume: ctrl.build_volume.get(),
        });
    }

    #[test]
    fn keyboard_translate_focus_clamps_at_plus_x_wall() {
        // Cube world AABB.max.x = 1.0 with identity. Envelope of
        // (2, 4, 4) gives env.max.x = 1.0 exactly: zero room on +X.
        // Holding +X under Translate focus must leave the object's
        // translation at zero across as many ticks as we run.
        let ctrl = ViewportController::new_for_test();
        install_for_integration(&ctrl);
        ctrl.on_select_by_index(0);
        ctrl.build_volume.set(BuildVolume::new(2.0, 4.0, 4.0));
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        integrate_two_ticks(&ctrl);

        let transform = ctrl
            .document
            .with_scene(|s| s.scene().objects[0].transform)
            .unwrap();
        let translation = match transform {
            Transform3D::Identity => [0.0; 3],
            Transform3D::Affine(a) => *a.translation(),
        };
        assert!(
            translation[0] <= f64::EPSILON,
            "expected translation.x clamped to 0 against +X wall, got {}",
            translation[0]
        );
    }

    #[test]
    fn keyboard_rotate_focus_unaffected_by_envelope() {
        // Same envelope-flush-with-cube setup. Rotate focus must
        // ignore the clamp: the linear part of the resulting
        // transform should reflect the integrated rotation, even
        // though the composed translation component is non-zero
        // (rotation about a non-origin pivot leaves the box centroid
        // in place but writes a non-zero translation into the affine
        // form). The test asserts the linear part diverges from
        // identity, which is the load-bearing claim that "rotation
        // happened, the clamp didn't squash it".
        let ctrl = ViewportController::new_for_test();
        install_for_integration(&ctrl);
        ctrl.on_select_by_index(0);
        ctrl.build_volume.set(BuildVolume::new(2.0, 4.0, 4.0));
        ctrl.on_gizmo_focus_rotate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        integrate_two_ticks(&ctrl);

        let transform = ctrl
            .document
            .with_scene(|s| s.scene().objects[0].transform)
            .unwrap();
        match transform {
            Transform3D::Identity => panic!("rotate focus should have produced an affine"),
            Transform3D::Affine(a) => {
                let m = a.linear();
                // R_x(theta) maps (0, 1, 0) to (0, cos, sin) and
                // (0, 0, 1) to (0, -sin, cos). For any non-zero
                // theta, m[2][1] (the Y-into-Z coefficient) is
                // non-zero, distinguishing the result from the
                // identity matrix.
                assert!(
                    m[2][1].abs() > 1e-6,
                    "expected non-trivial rotation, got linear {:?}",
                    m
                );
            }
        }
    }
}
