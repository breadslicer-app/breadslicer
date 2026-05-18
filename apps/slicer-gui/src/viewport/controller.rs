/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::rc::Rc;

use slint::ComponentHandle;
use slint::wgpu_28::wgpu;

use super::camera_controller::CameraController;
use super::gizmo_drag::{DragOp, GizmoState};
use super::input_dispatch::{InputDispatch, PointerUpOutcome};
use super::keyboard_drive::KbdFocus;
use super::object_list::{build_object_list_names, push_object_list};
use super::render_host::RenderHost;
use super::session_state::SessionState;
use super::types::{FrameState, ScreenPoint};
use crate::MainWindow;
use crate::PointerModifiers;
use crate::build_volume::BuildVolume;
use crate::camera::AxisView;
use crate::error_dialog::DialogManager;
use crate::gizmo;
use crate::i18n::ctr;
use crate::mesh_loader::start_load_with_ticket;
use crate::mesh_render::{GizmoDragVisual, GizmoVisibility, SceneRender};
use crate::pending_load::PendingLoad;
use crate::scene_document::{SceneDocument, SceneEvent};
use crate::scene_loader::{PreparedScene, SceneLoader};
use crate::selection::{LeafId, Selection};

pub struct ViewportController {
    ui: slint::Weak<MainWindow>,
    session: SessionState,
    camera: CameraController,
    render: RenderHost,
    input: InputDispatch,
}

impl ViewportController {
    pub fn new(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        pending_load: Rc<PendingLoad>,
    ) -> Self {
        let dialogs = DialogManager::new();
        let loader = Rc::new(SceneLoader::new(ui, dialogs.clone()));
        Self {
            ui: ui.as_weak(),
            session: SessionState::new(document, loader, pending_load, dialogs),
            camera: CameraController::new(),
            render: RenderHost::new(ui),
            input: InputDispatch::new(),
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
        let loader = Rc::new(SceneLoader::new_for_test(dialogs.clone()));
        let pending_load = Rc::new(PendingLoad::new());
        Self {
            ui: slint::Weak::default(),
            session: SessionState::new(SceneDocument::new(), loader, pending_load, dialogs),
            camera: CameraController::new(),
            render: RenderHost::new_for_test(),
            input: InputDispatch::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn install_graph_for_test(&self, graph: sc_mesh_core::SceneGraph) {
        // Tests don't care about the source path; populate the slot
        // with a sentinel so the document's invariant ("scene present
        // ⇔ path present") holds and any future test assertion on
        // `source_path` is explicit about its synthetic origin.
        self.session
            .document()
            .install_graph(graph, std::path::PathBuf::from("<test>"));
    }

    #[cfg(test)]
    pub(crate) fn clear_frame_state_for_test(&self) {
        self.render.clear_frame_state();
    }

    #[cfg(test)]
    pub(crate) fn redraw_owed_for_test(&self) -> bool {
        self.render.redraw_owed()
    }

    #[cfg(test)]
    pub(crate) fn set_viewport_for_test(&self, w: f32, h: f32) {
        self.render.set_viewport(w, h);
    }

    #[cfg(test)]
    pub(crate) fn viewport_for_test(&self) -> Option<(f32, f32)> {
        self.render.viewport().map(|vp| (vp.width(), vp.height()))
    }

    /// Access to the loader FSM. `wiring.rs` drives the async file-load
    /// handshake (begin/end, stamp, slot) against the loader directly
    /// rather than through delegating methods on the controller.
    pub fn loader(&self) -> Rc<SceneLoader> {
        self.session.loader().clone()
    }

    /// Access to the document. Used by `wiring.rs` to drive the
    /// `Export As` flow (read scene + source path) without going
    /// through delegating methods on the controller.
    pub fn document(&self) -> Rc<SceneDocument> {
        self.session.document().clone()
    }

    /// Access to the dialog manager. Used by `wiring.rs` to surface
    /// errors that don't originate from a controller call (e.g. an
    /// async file-save dialog flow whose result needs an error
    /// dialog).
    pub fn dialogs(&self) -> Rc<DialogManager> {
        self.session.dialogs().clone()
    }

    /// Subscribes the controller to its document. Called from
    /// `app_state` once the controller is wrapped in `Rc`. The
    /// listener captures a `Weak<Self>` to avoid an `Rc` cycle through
    /// the document.
    pub fn register_document_listener(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let sub = self.session.document().subscribe(move |event| {
            if let Some(strong) = weak.upgrade() {
                strong.handle_scene_event(event);
            }
        });
        self.session.install_subscription(sub);
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
            ui.set_can_undo(self.session.document().can_undo());
            ui.set_can_redo(self.session.document().can_redo());
        }
    }

    /// Reset that follows a fresh scene install. Camera reset to
    /// `None` so the renderer-bounds framing branch in
    /// `on_scene_load_finished` can populate it; UI mirrors the new
    /// scene's object list and the freshly-cleared selection.
    fn on_scene_installed(&self) {
        self.camera.reset();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_has_scene(true);
            let names = self
                .session
                .document()
                .with_scene(|spatial| build_object_list_names(spatial.graph()))
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
        self.camera.reset();
        self.render.set_frame(FrameState::default());
        self.input.reset();
        self.render.clear_renderer_scene();
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
        self.render.request_redraw();
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
    /// Current build envelope. Read by the load pipeline at dispatch
    /// time so the worker can drop the parsed scene onto the plate.
    pub fn build_volume(&self) -> BuildVolume {
        self.session.build_volume()
    }

    pub fn set_build_volume(&self, bv: BuildVolume) {
        if self.session.set_build_volume(bv) {
            self.mark_dirty();
        }
    }

    fn mark_dirty(&self) {
        self.render.mark_dirty();
    }

    pub fn on_viewport_resized(&self, w: f32, h: f32) {
        self.render.on_viewport_resized(w, h);
    }

    /// Called once `viewport-resized` has stayed quiet for `RESIZE_SETTLE`.
    /// Drops the bucketed MSAA / target allocations so the next frame
    /// reallocates at the exact logical size, eliminating the `image-fit:
    /// fill` stretch that persisted during the drag.
    pub fn on_resize_settled(&self) {
        self.render.on_resize_settled();
    }

    pub fn on_rotate(&self, dx: f32, dy: f32) {
        if self.camera.on_rotate(dx, dy, self.render.viewport()) {
            self.mark_dirty();
        }
    }

    pub fn on_pan(&self, dx: f32, dy: f32) {
        if self.camera.on_pan(dx, dy, self.render.viewport()) {
            self.mark_dirty();
        }
    }

    pub fn on_zoom(&self, delta: f32, cursor_x: f32, cursor_y: f32) {
        if self
            .camera
            .on_zoom(delta, cursor_x, cursor_y, self.render.viewport())
        {
            self.mark_dirty();
        }
    }

    pub fn on_frame_view(&self) {
        if self.camera.on_frame_view() {
            self.mark_dirty();
        }
    }

    pub fn on_axis_view(&self, view: crate::AxisView) {
        if self.camera.on_axis_view(axis_from_ui(view)) {
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
        self.session.loader().invalidate_and_clear();
        self.session.document().clear_scene();
    }

    pub fn on_scene_load_finished(&self) {
        let Some(prepared) = self.session.loader().take_finished_load() else {
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
            .render
            .renderer_cell()
            .borrow_mut()
            .as_mut()
            .map(|r| r.install_scene(gpu_scene, &built_with))
            .unwrap_or(false);

        // Fires `SceneEvent::Installed`; the listener resets the
        // camera to `None` (so the bounds-based framing below sees a
        // fresh slot), pushes the new object list, and rearms the UI
        // mirror.
        self.session
            .document()
            .install_spatial(spatial, source_path);

        if scene_installed {
            self.session.loader().note_upload_success();
            // Frame on the union of the scene's world AABB and the
            // build envelope, so the plate is always inside the view
            // and the orbit centre / zoom limits are anchored to the
            // printer's coordinate frame regardless of how big or
            // small the imported model is. Mirrors the framing the
            // pre-first-paint path uses when the camera slot is empty.
            let scene_bounds = self
                .render
                .renderer_cell()
                .borrow()
                .as_ref()
                .and_then(|r| r.scene_bounds());
            self.camera
                .replace_with_framing(scene_bounds, self.session.build_volume());
            self.render.update_frame(FrameState::with_redraw_owed);
        } else {
            self.render.update_frame(FrameState::with_upload_owed);
        }
    }

    /// Selection, resolved to its pre-order leaf index against the
    /// live graph. Test-only; the document's authoritative form is
    /// [`SceneDocument::selection_primary_node_id`].
    #[cfg(test)]
    pub(crate) fn selection(&self) -> Option<usize> {
        let document = self.session.document();
        document
            .selection_primary_node_id()
            .and_then(|id| document.with_scene(|s| s.leaf_index_of(id)).flatten())
    }

    /// Slint-side selection-changed callback. Out-of-bounds indices map
    /// to `None` (defensive, the UI shouldn't normally hand us one,
    /// but a stale callback can race with a scene reload). The Cell
    /// guard short-circuits the round-trip so the mirror-back to the UI
    /// doesn't fire `selection-changed` again.
    pub fn on_select_by_index(&self, idx: usize) {
        // Resolve the legacy pre-order index against the live graph
        // and store the resulting NodeId in the selection. An
        // out-of-range index degrades to `Selection::None`, the same
        // defensive behaviour as before; `leaf_id_at_index` returns
        // `None` past the last reachable leaf.
        let new_sel = self
            .session
            .document()
            .with_scene(|spatial| spatial.leaf_id_at_index(idx).map(LeafId))
            .flatten()
            .map_or(Selection::None, Selection::One);
        // `set_selection` short-circuits on no-change and fires
        // `SceneEvent::SelectionChanged` only on actual transitions;
        // the listener takes care of mirror-to-UI + mark-dirty.
        self.session.document().set_selection(new_sel);
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
            .session
            .document()
            .with_scene(|s| s.graph().reachable_leaf_count())
            .unwrap_or(0);
        if scene_len == 0 {
            return;
        }
        let step = delta.signum();
        let document = self.session.document();
        let cur_idx = document
            .selection_primary_node_id()
            .and_then(|id| document.with_scene(|s| s.leaf_index_of(id)).flatten());
        let next = match cur_idx {
            None => 0,
            Some(cur) => {
                let len_i = scene_len as i32;
                ((cur as i32 + step).rem_euclid(len_i)) as usize
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
        self.session.document().undo();
    }

    /// Edit → Redo handler.
    pub fn on_redo_requested(&self) {
        self.session.document().redo();
    }

    pub fn on_clear_selection(&self) {
        if matches!(self.input.gizmo_fsm().current(), GizmoState::Drag { .. }) {
            self.input.abort_gizmo_drag(self.session.document());
            return;
        }
        if self.input.kbd_drive().current_focus() != KbdFocus::None {
            self.apply_keyboard_focus(KbdFocus::None);
            return;
        }
        if matches!(self.session.document().selection(), Selection::None) {
            return;
        }
        // Selection going away kills the gizmo, so any cached hover
        // tint targets a handle that no longer renders. Clearing
        // before the document mutation keeps the order matching the
        // pre-listener implementation; `set_selection` then fires
        // `SelectionChanged` and the listener mirrors UI + dirty.
        self.input.gizmo_fsm().clear_hover();
        self.session.document().set_selection(Selection::None);
    }

    /// `T`/`R`/`S` keyboard handlers. Each requires a selection to
    /// exist; without one, the press is a no-op (the user has to
    /// pick something to focus on first). Pressing the same focus
    /// type twice is a no-op so the integrator's tick clock isn't
    /// reset by autorepeat.
    pub fn on_gizmo_focus_translate(&self) {
        self.apply_keyboard_focus(KbdFocus::Translate);
    }

    pub fn on_gizmo_focus_rotate(&self) {
        self.apply_keyboard_focus(KbdFocus::Rotate);
    }

    pub fn on_gizmo_focus_scale(&self) {
        self.apply_keyboard_focus(KbdFocus::Scale);
    }

    /// Sets the keyboard focus on the input concern, then mirrors
    /// the activation state to the UI property and marks dirty on
    /// any actual change. The mirror + dirty step is the cross-
    /// concern part of the cascade: InputDispatch owns the focus
    /// cell, the coordinator owns the UI handle.
    fn apply_keyboard_focus(&self, focus: KbdFocus) {
        let Some(new_focus) = self
            .input
            .set_keyboard_focus(self.session.document(), focus)
        else {
            return;
        };
        if let Some(ui) = self.ui.upgrade() {
            ui.set_gizmo_keyboard_focus_active(new_focus != KbdFocus::None);
        }
        self.mark_dirty();
    }

    pub fn on_gizmo_arrow_pressed(&self, code: i32) {
        if self
            .input
            .on_gizmo_arrow_pressed(self.session.document(), code)
        {
            self.mark_dirty();
        }
    }

    pub fn on_gizmo_arrow_released(&self, code: i32) {
        if self
            .input
            .on_gizmo_arrow_released(self.session.document(), code)
        {
            self.mark_dirty();
        }
    }

    pub fn on_pointer_down(&self, x: f32, y: f32, modifiers: PointerModifiers) {
        self.input.on_pointer_down(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            ScreenPoint { x, y },
            modifiers,
        );
    }

    pub fn on_pointer_move(&self, x: f32, y: f32, modifiers: PointerModifiers) {
        if self.input.on_pointer_move(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            self.session.build_volume(),
            ScreenPoint { x, y },
            modifiers,
        ) {
            self.mark_dirty();
        }
    }

    pub fn on_pointer_up(&self, x: f32, y: f32, _modifiers: PointerModifiers) {
        match self
            .input
            .on_pointer_up(self.session.document(), ScreenPoint { x, y })
        {
            PointerUpOutcome::DragCommitted => self.mark_dirty(),
            PointerUpOutcome::Click(cursor) => self.run_pick_at(cursor),
            PointerUpOutcome::Nothing => (),
        }
    }

    /// Picker dispatch: hand the pixel coordinates to InputDispatch
    /// (which builds the ray and runs `pick_object`) and apply the
    /// pick result through the document's selection setters. The
    /// picker carries `NodeId` directly; the document stores the
    /// stable handle without a pre-order-index detour.
    fn run_pick_at(&self, cursor: ScreenPoint) {
        let Some(pick) = self.input.pick_at(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            cursor,
        ) else {
            return;
        };
        match pick {
            Some(node_id) => self
                .session
                .document()
                .set_selection(Selection::One(LeafId(node_id))),
            None => self.on_clear_selection(),
        }
    }

    fn mirror_selection_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let document = self.session.document();
            let idx = document
                .selection_primary_node_id()
                .and_then(|id| document.with_scene(|s| s.leaf_index_of(id)).flatten())
                .map_or(-1, |i| i as i32);
            ui.set_selected_object_index(idx);
        }
    }

    pub fn handle_rendering_setup(&self, device: wgpu::Device, queue: wgpu::Queue) {
        self.session.loader().set_gpu(device.clone(), queue.clone());
        self.render.handle_rendering_setup(device, queue);
        self.session.loader().reset_upload_failures();
        if self.session.document().is_loaded() {
            self.render.update_frame(FrameState::with_upload_owed);
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
        if let Some(path) = self.session.pending_load().take()
            && let Some(ticket) = self.session.loader().begin_load()
        {
            start_load_with_ticket(
                ticket,
                path,
                self.session.loader(),
                self.session.build_volume(),
            );
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
        self.session.loader().clear_gpu();
        self.render.handle_rendering_teardown();
    }

    pub fn handle_before_rendering(&self) {
        // Step the keyboard-focus integrator before borrowing the
        // renderer, the integrator mutates the scene's transform
        // and marks dirty, which is exactly the input the renderer
        // pass below needs to read in this same frame.
        self.input.integrate_keyboard_arrows(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            self.session.build_volume(),
        );

        let mut renderer_slot = self.render.renderer_cell().borrow_mut();
        let Some(renderer) = renderer_slot.as_mut() else {
            return;
        };

        if self.render.frame().upload_owed() {
            let upload = self
                .session
                .document()
                .with_scene(|spatial| renderer.upload_scene(spatial.graph()));
            match upload {
                Some(Ok(())) => {
                    self.render.update_frame(FrameState::after_upload_success);
                    self.session.loader().note_upload_success();
                }
                Some(Err(err)) => {
                    // Leave the upload flag set so it continues to
                    // represent "upload owed", a subsequent renderer
                    // setup or scene reload (both of which reset the
                    // loader's failure counter) will drive a retry
                    // without relying on coincidental side effects.
                    self.session.loader().note_upload_failure(&err);
                    return;
                }
                None => {
                    // Flag set but the scene was cleared before we got to it.
                    self.render.update_frame(FrameState::upload_cleared);
                }
            }
        }

        // Initialise the orbit camera if it isn't yet. With the plate
        // visible at first paint, framing comes from the union of the
        // scene's world AABB (when uploaded) and the build envelope.
        // Skipped if a camera already exists (re-upload across a
        // RenderingTeardown -> Setup cycle preserves the user's view).
        if !self.camera.is_initialised() {
            let scene_bounds = renderer.scene_bounds();
            self.camera
                .replace_with_framing(scene_bounds, self.session.build_volume());
        }

        if !self.render.frame().redraw_owed() {
            return;
        }

        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(vp) = self.render.viewport() else {
            return;
        };
        let h_logical = vp.height();
        let scale = ui.window().scale_factor();
        let width = (vp.width() * scale).round() as u32;
        let height = (h_logical * scale).round() as u32;
        if width == 0 || height == 0 {
            return;
        }

        let (eff_w, eff_h) = self.render.bucketed_size(width, height);

        let camera_slot = self.camera.camera_cell().borrow();
        let Some(camera) = camera_slot.as_ref() else {
            return;
        };

        let build_volume = self.session.build_volume();

        // Render path. With a scene installed, the closure inside
        // `with_scene` builds the gizmo frame against the live scene
        // and renders the union (plate + objects). With no scene,
        // `with_scene` returns `None` and the fallback below renders
        // the plate alone with neither selection nor gizmo.
        let texture = self
            .session
            .document()
            .with_scene(|spatial_for_render| {
                let selection_node_id = self.session.document().selection_primary_node_id();
                let drag_state = self.input.gizmo_fsm().current();
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
                let hover = self.input.gizmo_fsm().current_hover();
                let gizmo_frame = selection_node_id.and_then(|sel| {
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
                        graph: spatial_for_render.graph(),
                        selection: selection_node_id,
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
        self.render.update_frame(FrameState::after_render);
        match slint::Image::try_from(texture) {
            Ok(image) => ui.set_mesh_rendering(image),
            Err(err) => self.session.dialogs().show_error_deferred(
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
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::Transform3D;
    use sc_mesh_core::{IndexedMesh, LeafPayload, SceneGraph};

    /// `PointerModifiers::default()` with all flags off. The selection
    /// tests don't exercise modifier-driven branches, so every press
    /// uses this.
    fn no_mods() -> PointerModifiers {
        PointerModifiers::default()
    }

    fn empty_leaf_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![],
        }
    }

    /// Mirrors `SceneGraph::try_from(Scene { objects: vec![...n], .. })`:
    /// all leaves land under a single identity-Group root.
    fn graph_with_n_objects(n: usize) -> SceneGraph {
        let mut g = SceneGraph::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        for _ in 0..n {
            let leaf = g
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(empty_leaf_mesh()),
                )
                .expect("add leaf");
            g.add_child(root, leaf).expect("attach leaf");
        }
        g
    }

    #[test]
    fn fresh_controller_has_no_selection() {
        let ctrl = ViewportController::new_for_test();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn select_by_index_records_in_bounds_indices() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
        ctrl.on_select_by_index(1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_select_by_index(2);
        assert_eq!(ctrl.selection(), Some(2));
    }

    #[test]
    fn select_by_index_clamps_out_of_bounds_to_none() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(2));
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
        ctrl.install_graph_for_test(graph_with_n_objects(2));
        ctrl.on_select_by_index(1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn close_scene_clears_selection() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
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
        ctrl.install_graph_for_test(graph_with_n_objects(2));
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
        ctrl.install_graph_for_test(graph_with_n_objects(1));
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
        ctrl.install_graph_for_test(graph_with_n_objects(1));
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
        ctrl.install_graph_for_test(graph_with_n_objects(1));
        ctrl.on_pointer_up(50.0, 50.0, no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn cycle_with_no_selection_picks_first_object() {
        // Tab from "nothing selected" doubles as "select the first
        // thing". Both directions land on index 0, the user's
        // mental model is "advance to the first item that exists".
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(0));

        ctrl.on_clear_selection();
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_forward_advances_through_objects_and_wraps() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
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
        ctrl.install_graph_for_test(graph_with_n_objects(3));
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

        // Empty scene installed (loader-shape: Group root with no leaves).
        ctrl.install_graph_for_test(graph_with_n_objects(0));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn cycle_with_single_object_stays_on_it() {
        // One-object scene → cycling either direction lands on
        // index 0 every time. Mod-N arithmetic with N=1 collapses
        // to 0 cleanly.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(1));
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
        ctrl.install_graph_for_test(graph_with_n_objects(3));
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
    use crate::gizmo::axis::Axis;
    use crate::mesh_render::default_fov_y;
    use crate::picking;
    use crate::scene_document::SceneSubscription;
    use float_cmp::assert_approx_eq;
    use glam::Vec3;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::Transform3D;
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::{IndexedMesh, LeafPayload, NodeId, SceneGraph};
    use std::cell::Cell;

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

    /// Fresh graph with a single identity-Group root, matching the
    /// loader-pipeline topology these click tests expect to drive.
    fn graph_with_group_root() -> (SceneGraph, NodeId) {
        let mut g = SceneGraph::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    /// CCW-from-+Z triangle at `translation`, attached as a leaf
    /// under `parent` (the loader-shape Group root).
    fn add_front_facing_triangle(g: &mut SceneGraph, parent: NodeId, translation: [f32; 3]) {
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
        let transform = Transform3D::Affine(AffineTransform3D::from_translation([
            translation[0] as f64,
            translation[1] as f64,
            translation[2] as f64,
        ]));
        let leaf = g
            .add_leaf(None, transform, LeafPayload::Mesh(mesh))
            .expect("add leaf");
        g.add_child(parent, leaf).expect("attach leaf");
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
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        add_front_facing_triangle(&mut g, root, [10.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);

        // Camera framed on the first triangle, looking down -Z.
        // Centre pixel of the viewport casts a ray straight at it.
        *ctrl.camera.camera_cell().borrow_mut() =
            Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        ctrl.set_viewport_for_test(200.0, 100.0);

        ctrl.on_pointer_down(100.0, 50.0, no_mods());
        ctrl.on_pointer_up(100.0, 50.0, no_mods());
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn click_in_empty_space_clears_the_selection() {
        let ctrl = ViewportController::new_for_test();
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);
        *ctrl.camera.camera_cell().borrow_mut() =
            Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
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
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);
        *ctrl.camera.camera_cell().borrow_mut() =
            Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
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
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);
        *ctrl.camera.camera_cell().borrow_mut() = Some(front_camera());
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
        ctrl.session
            .document()
            .with_scene(|spatial| {
                let id = spatial
                    .leaf_id_at_index(0)
                    .expect("test fixture has a leaf");
                match spatial.leaf(id).unwrap().local_transform() {
                    Transform3D::Identity => Vec3::ZERO,
                    Transform3D::Affine(a) => Vec3::new(
                        a.translation()[0] as f32,
                        a.translation()[1] as f32,
                        a.translation()[2] as f32,
                    ),
                }
            })
            .unwrap()
    }

    fn read_linear(ctrl: &ViewportController) -> [[f64; 3]; 3] {
        ctrl.session
            .document()
            .with_scene(|spatial| {
                let id = spatial
                    .leaf_id_at_index(0)
                    .expect("test fixture has a leaf");
                match spatial.leaf(id).unwrap().local_transform() {
                    Transform3D::Identity => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    Transform3D::Affine(a) => *a.linear(),
                }
            })
            .unwrap()
    }

    /// Project the X-axis midpoint cube's centre to logical-pixel
    /// coords for the standard test setup. The cube sits at
    /// `pivot + axis_world * (scale * MIDPOINT_FRACTION)`; with the
    /// front camera, that lands on screen-y = viewport-centre.
    fn x_midpoint_press_screen(ctrl: &ViewportController) -> (f32, f32) {
        let cam = ctrl.camera.camera_cell().borrow().as_ref().unwrap().clone();
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
        match ctrl.input.gizmo_fsm().current() {
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
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
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
        let cam = ctrl.camera.camera_cell().borrow().as_ref().unwrap().clone();
        let fov_y = default_fov_y();
        let press_ray = picking::ray_from_pixel(&cam, fov_y, vw, vh, press.0, press.1);
        let release_ray = picking::ray_from_pixel(&cam, fov_y, vw, vh, release.0, release.1);
        let press_anchor = gizmo::drag_math::closest_point_on_axis(&press_ray, Vec3::ZERO, Axis::X);
        let release_anchor =
            gizmo::drag_math::closest_point_on_axis(&release_ray, Vec3::ZERO, Axis::X);
        let expected = release_anchor - press_anchor;

        ctrl.on_pointer_down(press.0, press.1, no_mods());
        // Drag must have started, otherwise the move is a no-op.
        assert!(matches!(
            ctrl.input.gizmo_fsm().current(),
            GizmoState::Drag { .. }
        ));
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
        let cam = ctrl.camera.camera_cell().borrow().as_ref().unwrap().clone();
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
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
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

        assert!(ctrl.session.document().can_undo());
        assert!(!ctrl.session.document().can_redo());

        let after_drag = read_translation(&ctrl);
        assert!(after_drag.x.abs() > 1e-4);

        assert!(ctrl.session.document().undo());
        let after_undo = read_translation(&ctrl);
        assert_approx_eq!(f32, after_undo.x, 0.0, epsilon = 1e-6);
        assert!(!ctrl.session.document().can_undo());
        assert!(ctrl.session.document().can_redo());

        assert!(ctrl.session.document().redo());
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

        assert!(!ctrl.session.document().can_undo());
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

        assert!(!ctrl.session.document().can_undo());
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
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
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

        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn jitter_within_threshold_still_counts_as_a_click() {
        // A press → release that moves only a few subpixels
        // (trackpad jitter, debounced finger lift) must still be
        // treated as a click. Threshold is 4 logical pixels.
        let ctrl = ViewportController::new_for_test();
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);
        *ctrl.camera.camera_cell().borrow_mut() =
            Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
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
        match ctrl.input.gizmo_fsm().current() {
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
        match ctrl.input.gizmo_fsm().current() {
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

        let (count, _sub) = subscribe_mutated_counter(ctrl.session.document());
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

        let (count, _sub) = subscribe_mutated_counter(ctrl.session.document());
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
        assert!(matches!(
            ctrl.input.gizmo_fsm().current(),
            GizmoState::Drag { .. }
        ));

        let (count, _sub) = subscribe_mutated_counter(ctrl.session.document());
        let aborted = ctrl.input.gizmo_fsm().abort(ctrl.session.document());
        assert!(aborted);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn gizmo_abort_with_no_drag_fires_no_mutated_event() {
        // No drag in flight, abort short-circuits before touching
        // the document, so no Mutated must fire.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));

        let (count, _sub) = subscribe_mutated_counter(ctrl.session.document());
        let aborted = ctrl.input.gizmo_fsm().abort(ctrl.session.document());
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
        let initial = ctrl.session.build_volume();
        let new_bv = BuildVolume::new(50.0, 50.0, 50.0);
        assert_ne!(initial, new_bv);
        ctrl.clear_frame_state_for_test();
        assert!(!ctrl.redraw_owed_for_test());

        ctrl.set_build_volume(new_bv);

        assert_eq!(ctrl.session.build_volume(), new_bv);
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
        assert_eq!(ctrl.session.build_volume(), bv);
        assert!(!ctrl.redraw_owed_for_test());

        let bigger = BuildVolume::new(150.0, 80.0, 120.0);
        ctrl.set_build_volume(bigger);
        assert_eq!(ctrl.session.build_volume(), bigger);
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
        ctrl.session
            .set_build_volume(BuildVolume::new(1.0, 4.0, 4.0));

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
    use super::super::keyboard_drive::{ARROW_X_POS, ARROW_Y_POS, IntegrationCtx};
    use super::*;
    use crate::camera::OrbitCamera;
    use crate::gizmo::axis::Axis;
    use crate::scene_document::SceneEdit;
    use glam::Vec3;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::Transform3D;
    use sc_mesh_core::{IndexedMesh, LeafPayload, SceneGraph};

    /// Loader-shape one-object graph: a single inline-mesh leaf
    /// attached under an identity-Group root.
    fn one_object_graph() -> SceneGraph {
        let mut g = SceneGraph::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![],
        };
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh))
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        g
    }

    #[test]
    fn focus_requires_an_existing_selection() {
        // No selection → T is a no-op. Pinning this matters because
        // the integrator dereferences `selection`; a focus without a
        // selection would crash.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_gizmo_focus_translate();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::None);
    }

    #[test]
    fn focus_set_with_selection_takes_effect() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::Translate);
        ctrl.on_gizmo_focus_rotate();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::Rotate);
        ctrl.on_gizmo_focus_scale();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::Scale);
    }

    #[test]
    fn switching_focus_resets_held_arrow_bitset() {
        // Holding +X under translate focus, then switching to rotate
        // focus must clear the held bit, the user's mental model
        // is "release on mode switch".
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        assert!(ctrl.input.kbd_drive().current_arrows().any_held());

        ctrl.on_gizmo_focus_rotate();
        assert!(
            !ctrl.input.kbd_drive().current_arrows().any_held(),
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
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        assert!(!ctrl.input.kbd_drive().current_arrows().any_held());
    }

    #[test]
    fn arrow_release_clears_bit_even_with_focus_dropped() {
        // A release that arrives after focus was dropped (race with
        // Esc) must still clear the bit, otherwise the next focus
        // session would inherit a phantom held key.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        // Focus drop happens (e.g. via Esc cascade).
        ctrl.on_clear_selection();
        // Stale release arrives.
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);
        assert!(!ctrl.input.kbd_drive().current_arrows().any_held());
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
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        let node_id = ctrl.session.document().selection_primary_node_id().unwrap();
        ctrl.session
            .document()
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation_x(2.0),
            });
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);

        assert!(ctrl.session.document().can_undo());
        assert!(ctrl.session.document().undo());
        let after_undo = ctrl
            .session
            .document()
            .with_scene(|s| {
                let id = s.leaf_id_at_index(0).expect("test fixture has a leaf");
                s.leaf(id).unwrap().local_transform()
            })
            .unwrap();
        assert_eq!(after_undo, Transform3D::Identity);
    }

    #[test]
    fn keyboard_burst_with_no_motion_pushes_no_entry() {
        // Press → release without any integration tick: scene
        // transform never changed, push_edit's no-op filter drops
        // the entry.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);

        assert!(!ctrl.session.document().can_undo());
    }

    #[test]
    fn keyboard_burst_focus_change_flushes_pending_burst() {
        // Switching from translate to rotate mid-burst commits the
        // current run as one undo entry rather than discarding it;
        // the user's translate work is preserved.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        let node_id = ctrl.session.document().selection_primary_node_id().unwrap();
        ctrl.session
            .document()
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation_x(3.0),
            });
        // User switches mode without releasing the key.
        ctrl.on_gizmo_focus_rotate();

        assert!(ctrl.session.document().can_undo());
        ctrl.session.document().undo();
        let after_undo = ctrl
            .session
            .document()
            .with_scene(|s| {
                let id = s.leaf_id_at_index(0).expect("test fixture has a leaf");
                s.leaf(id).unwrap().local_transform()
            })
            .unwrap();
        assert_eq!(after_undo, Transform3D::Identity);
    }

    #[test]
    fn keyboard_burst_two_keys_held_is_a_single_undo_entry() {
        // Two arrows pressed then released independently is still
        // one burst, only the first-press transition arms the
        // tracker, only the all-released transition flushes it.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.on_gizmo_arrow_pressed(ARROW_Y_POS);
        let node_id = ctrl.session.document().selection_primary_node_id().unwrap();
        ctrl.session
            .document()
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation_x(1.0),
            });
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);
        // Bitset still has Y_POS held, must not flush yet.
        assert!(!ctrl.session.document().can_undo());
        ctrl.on_gizmo_arrow_released(ARROW_Y_POS);
        assert!(ctrl.session.document().can_undo());
        // Exactly one entry: a second undo is rejected.
        assert!(ctrl.session.document().undo());
        assert!(!ctrl.session.document().can_undo());
    }

    #[test]
    fn keyboard_burst_dropped_on_scene_close() {
        // Scene close drops the burst without flushing, the object
        // index it captured is about to become meaningless.
        let ctrl = std::rc::Rc::new(ViewportController::new_for_test());
        ctrl.register_document_listener();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        let node_id = ctrl.session.document().selection_primary_node_id().unwrap();
        ctrl.session
            .document()
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation_x(1.0),
            });

        ctrl.on_close_scene();
        assert!(!ctrl.session.document().can_undo());
    }

    #[test]
    fn esc_cascade_drag_then_focus_then_selection() {
        // Each Esc drops one level of the cascade.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        // No drag is possible without a camera; simulate the
        // mid-cascade state by directly writing the gizmo cell.
        let node_id = ctrl
            .session
            .document()
            .selection_primary_node_id()
            .expect("test selected object 0");
        ctrl.input.gizmo_fsm().force_set(GizmoState::Drag {
            op: DragOp::Translate {
                axis: Axis::X,
                anchor: Vec3::ZERO,
            },
            original: Transform3D::Identity,
            pivot: Vec3::ZERO,
            node_id,
        });

        // Level 1 → drag aborts; focus and selection survive.
        ctrl.on_clear_selection();
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::Translate);
        assert_eq!(ctrl.selection(), Some(0));

        // Level 2 → focus drops; selection survives.
        ctrl.on_clear_selection();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::None);
        assert_eq!(ctrl.selection(), Some(0));

        // Level 3 → selection clears.
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }

    /// Single object whose local AABB has world.max.x = 1 with the
    /// identity transform. Useful for clamp tests where the goal is
    /// "envelope is exactly flush with the object's +X face, so any
    /// +X delta clamps to zero".
    fn cube_object_graph() -> SceneGraph {
        let mut g = SceneGraph::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([-1.0, -1.0, 0.0]), Vertex::new([1.0, 1.0, 2.0])],
            faces: vec![],
        };
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh))
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        g
    }

    /// Front-axis camera + 400 x 200 viewport. Mirrors the gizmo
    /// tests' setup so `world_per_pixel` is the same scale, and
    /// the keyboard integrator's translation rate is comparable.
    fn install_for_integration(ctrl: &ViewportController) {
        ctrl.install_graph_for_test(cube_object_graph());
        let mut cam = OrbitCamera::new(Vec3::ZERO, 1.0);
        cam.set_axis_view(crate::camera::AxisView::Front);
        *ctrl.camera.camera_cell().borrow_mut() = Some(cam);
        ctrl.set_viewport_for_test(400.0, 200.0);
    }

    /// Drive `kbd_drive.integrate` twice with a real-time pause
    /// between calls so the per-tick `dt` is non-zero on the second
    /// call. The first call seeds `last_tick`; the second produces
    /// the actual integration step.
    fn integrate_two_ticks(ctrl: &ViewportController) {
        let _ = ctrl.input.kbd_drive().integrate(IntegrationCtx {
            document: ctrl.session.document(),
            camera: ctrl.camera.camera_cell(),
            viewport: ctrl.render.viewport(),
            selection_id: ctrl.session.document().selection_primary_node_id(),
            build_volume: ctrl.session.build_volume(),
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = ctrl.input.kbd_drive().integrate(IntegrationCtx {
            document: ctrl.session.document(),
            camera: ctrl.camera.camera_cell(),
            viewport: ctrl.render.viewport(),
            selection_id: ctrl.session.document().selection_primary_node_id(),
            build_volume: ctrl.session.build_volume(),
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
        ctrl.session
            .set_build_volume(BuildVolume::new(2.0, 4.0, 4.0));
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        integrate_two_ticks(&ctrl);

        let transform = ctrl
            .session
            .document()
            .with_scene(|s| {
                let id = s.leaf_id_at_index(0).expect("test fixture has a leaf");
                s.leaf(id).unwrap().local_transform()
            })
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
        ctrl.session
            .set_build_volume(BuildVolume::new(2.0, 4.0, 4.0));
        ctrl.on_gizmo_focus_rotate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        integrate_two_ticks(&ctrl);

        let transform = ctrl
            .session
            .document()
            .with_scene(|s| {
                let id = s.leaf_id_at_index(0).expect("test fixture has a leaf");
                s.leaf(id).unwrap().local_transform()
            })
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
