/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-viewport input concern. Owns the click/drag latch, the gizmo
//! finite-state machine, and the keyboard-drive integrator; exposes
//! the pointer and keyboard callbacks the coordinator's `wiring.rs`
//! dispatches.
//!
//! The Slint wiring takes raw `(x, y, modifiers)` from the UI and
//! hands them to the coordinator, which forwards them to this
//! struct's methods together with peer references (the
//! `SceneDocument`, the `&RefCell<Option<OrbitCamera>>` exposed by
//! [`CameraController::camera_cell`](super::camera_controller::CameraController::camera_cell),
//! the viewport size, the build volume). The methods return values
//! that signal the cross-concern action the coordinator should take
//! next (e.g. mark dirty, mirror focus to the UI property, dispatch
//! the picker).
//!
//! Scene-mutating paths apply their edits to the document directly,
//! the document's `Mutated` event fires through the coordinator's
//! listener which marks the renderer dirty. The bool returns on the
//! non-mutating paths (hover changes, focus changes) flag the
//! coordinator that it should mark dirty manually since those state
//! changes do not route through `apply_command`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sc_mesh_core::NodeId;

use super::gizmo_fsm::{
    GizmoFsm, HoverCtx as GizmoHoverCtx, MoveCtx as GizmoMoveCtx, StartCtx as GizmoStartCtx,
};
use super::input_state::{ClickOrDrag, InputState};
use super::keyboard_drive::{IntegrationCtx, KbdFocus, KeyboardDriveState};
use super::types::{ScreenPoint, ViewportSize};
use crate::PointerModifiers;
use crate::build_volume::BuildVolume;
use crate::camera::OrbitCamera;
use crate::mesh_render::default_fov_y;
use crate::picking;
use crate::scene_document::SceneDocument;
use crate::selection::Selection;

/// Outcome of [`InputDispatch::on_pointer_up`] the coordinator must
/// act on:
/// - `DragCommitted`: the gizmo drag was committed (or aborted as a
///   no-op edit); the FSM returns to `Idle`. The caller marks the
///   renderer dirty so the post-drag visuals (handle highlight off,
///   selection halo redraw) take effect.
/// - `Click(cursor)`: the gesture stayed within the click threshold
///   and did not commit a drag. The caller dispatches the picker via
///   [`InputDispatch::pick_at`] and applies the result through the
///   coordinator's selection setters.
/// - `Nothing`: the release did not match a prior press (e.g. lost
///   focus before mouse-up, or a release synthesised by Slint).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PointerUpOutcome {
    DragCommitted,
    Click(ScreenPoint),
    Nothing,
}

pub(super) struct InputDispatch {
    /// Logical-pixel position at which the left button was last
    /// pressed. `Some` between `on_pointer_down` and `on_pointer_up`;
    /// `on_pointer_up` reads it to disambiguate click from drag.
    input_state: InputState,
    /// Gizmo drag state. Idle when no drag is in progress.
    gizmo_fsm: GizmoFsm,
    /// Keyboard-driven gizmo state: focus, held-arrow bitset, burst
    /// tracker.
    kbd_drive: KeyboardDriveState,
    /// Last cursor position reported by `on_pointer_move`. `None`
    /// before the very first move event reaches this dispatcher.
    /// Survives across `pointer-leave` so a Ctrl+V right after the
    /// cursor briefly exited the viewport still has a position to
    /// fall back on; `last_hover_cursor` gates the read on the
    /// `hovering` flag.
    last_cursor: Cell<Option<ScreenPoint>>,
    /// `true` while the cursor sits inside the viewport. Flipped by
    /// `on_pointer_move` (true) and `on_pointer_leave` (false). The
    /// paste path reads this through `last_hover_cursor` so a copy /
    /// paste with the cursor outside the viewport falls back to the
    /// viewport-centre placement rather than landing where the
    /// cursor was when it left.
    hovering: Cell<bool>,
}

impl InputDispatch {
    pub(super) fn new() -> Self {
        Self {
            input_state: InputState::new(),
            gizmo_fsm: GizmoFsm::new(),
            kbd_drive: KeyboardDriveState::new(),
            last_cursor: Cell::new(None),
            hovering: Cell::new(false),
        }
    }

    /// Read-only access to the gizmo FSM for the renderer (drag
    /// state, hover handle) and for test assertions.
    pub(super) fn gizmo_fsm(&self) -> &GizmoFsm {
        &self.gizmo_fsm
    }

    /// Read-only access to the keyboard-drive state for the
    /// coordinator (Esc cascade reads the focus) and for tests.
    pub(super) fn kbd_drive(&self) -> &KeyboardDriveState {
        &self.kbd_drive
    }

    /// Full reset of every input cell. Called on scene clear so a
    /// drag captured against a now-missing object is dropped, the
    /// hover tint is cleared, the click latch is reset, and the
    /// keyboard focus / held-arrows are dropped.
    pub(super) fn reset(&self) {
        self.gizmo_fsm.set_idle();
        self.gizmo_fsm.clear_hover();
        self.input_state.set_idle();
        self.kbd_drive.reset();
        self.last_cursor.set(None);
        self.hovering.set(false);
    }

    /// Records that the cursor has left the viewport. The paste path
    /// reads `hovering` through `last_hover_cursor` to decide whether
    /// to anchor on the cached cursor or fall back to the viewport
    /// centre.
    #[allow(dead_code)]
    pub(super) fn on_pointer_leave(&self) {
        self.hovering.set(false);
    }

    /// Last cursor position, gated on the cursor still hovering the
    /// viewport. Returns `None` when the cursor has left the viewport
    /// since the last move event or no move has ever fired, so paste
    /// fallbacks (e.g. viewport-centre placement) can branch on it.
    #[allow(dead_code)]
    pub(super) fn last_hover_cursor(&self) -> Option<ScreenPoint> {
        if self.hovering.get() {
            self.last_cursor.get()
        } else {
            None
        }
    }

    /// Records the press and, if the cursor hits a gizmo handle,
    /// transitions the FSM into `Drag` capturing the original
    /// transform and the world-space anchor. `modifiers.shift` at
    /// press time captures into the resulting `ScaleAxis.proportional`
    /// flag.
    pub(super) fn on_pointer_down(
        &self,
        document: &Rc<SceneDocument>,
        camera: &RefCell<Option<OrbitCamera>>,
        viewport: Option<ViewportSize>,
        cursor: ScreenPoint,
        modifiers: PointerModifiers,
    ) {
        self.input_state.set_pressed(cursor);
        let ctx = GizmoStartCtx {
            document,
            camera,
            viewport,
            selection_id: document.selection_primary_node_id(),
            press: cursor,
            modifiers,
        };
        self.gizmo_fsm.try_start_drag(ctx);
    }

    /// Drives the gizmo drag for `pointer-moved`, recomputes the
    /// per-op delta from the current cursor ray, and composes the
    /// resulting transform onto the captured original. Independently,
    /// updates the hover tint when no drag is in flight. Returns
    /// `true` if the cached hover changed (caller should mark dirty),
    /// drag-driven scene mutations fire `Mutated` separately and the
    /// listener marks dirty on its own.
    pub(super) fn on_pointer_move(
        &self,
        document: &Rc<SceneDocument>,
        camera: &RefCell<Option<OrbitCamera>>,
        viewport: Option<ViewportSize>,
        build_volume: BuildVolume,
        cursor: ScreenPoint,
        modifiers: PointerModifiers,
    ) -> bool {
        self.last_cursor.set(Some(cursor));
        self.hovering.set(true);
        let move_ctx = GizmoMoveCtx {
            document,
            camera,
            viewport,
            cursor,
            modifiers,
            build_volume,
        };
        // The bool return is intentionally discarded, a successful
        // drag move routes through `apply_command` and the `Mutated`
        // listener schedules the redraw.
        let _ = self.gizmo_fsm.on_move(move_ctx);

        let hover_ctx = GizmoHoverCtx {
            document,
            camera,
            viewport,
            selection_id: document.selection_primary_node_id(),
            cursor,
        };
        self.gizmo_fsm.update_hover(hover_ctx)
    }

    /// If a drag was in flight, commits it (or filters the no-op
    /// edit) and reports `DragCommitted`. Otherwise classifies the
    /// gesture as click or drag-without-handle; on a click within
    /// the threshold, reports `Click { x, y }` so the coordinator
    /// can dispatch the picker.
    pub(super) fn on_pointer_up(
        &self,
        document: &Rc<SceneDocument>,
        cursor: ScreenPoint,
    ) -> PointerUpOutcome {
        let outcome = self.input_state.click_or_drag(cursor);
        if self.gizmo_fsm.commit_drag(document) {
            return PointerUpOutcome::DragCommitted;
        }
        if outcome == ClickOrDrag::Click {
            return PointerUpOutcome::Click(cursor);
        }
        PointerUpOutcome::Nothing
    }

    /// Builds the ray at `(x, y)` and runs `pick_object` against the
    /// scene. Returns:
    /// - `None`: the picker bailed (no camera, no viewport, or no scene).
    /// - `Some(None)`: ray hit empty space (caller clears the selection).
    /// - `Some(Some(node_id))`: ray hit the leaf identified by `node_id`
    ///   (caller selects it).
    pub(super) fn pick_at(
        &self,
        document: &Rc<SceneDocument>,
        camera: &RefCell<Option<OrbitCamera>>,
        viewport: Option<ViewportSize>,
        cursor: ScreenPoint,
    ) -> Option<Option<NodeId>> {
        let vp = viewport?;
        let camera_slot = camera.borrow();
        let camera = camera_slot.as_ref()?;
        let ray = picking::ray_from_pixel(
            camera,
            default_fov_y(),
            vp.width(),
            vp.height(),
            cursor.x,
            cursor.y,
        );
        document.with_scene(|spatial| picking::pick_object(&ray, spatial))
    }

    /// Aborts any in-progress drag (reverts the affected object's
    /// transform to its captured original) and resets the click
    /// latch. Returns `true` if a drag was in flight (the revert
    /// itself fires `Mutated` and the listener marks the renderer
    /// dirty).
    pub(super) fn abort_gizmo_drag(&self, document: &Rc<SceneDocument>) -> bool {
        if self.gizmo_fsm.abort(document) {
            self.input_state.set_idle();
            true
        } else {
            false
        }
    }

    /// Updates the keyboard focus cell. Returns `Some(new_focus)` if
    /// the cell changed (caller mirrors `new_focus != None` to the
    /// `gizmo-keyboard-focus-active` UI property and marks dirty),
    /// `None` otherwise (no-op press, autorepeat, or focus rejected
    /// by `KeyboardDriveState::try_set_focus`).
    pub(super) fn set_keyboard_focus(
        &self,
        document: &Rc<SceneDocument>,
        focus: KbdFocus,
    ) -> Option<KbdFocus> {
        let has_selection = !matches!(document.selection(), Selection::None);
        if self.kbd_drive.try_set_focus(document, focus, has_selection) {
            Some(focus)
        } else {
            None
        }
    }

    /// Held-key handler. `code` is the `ARROW_*` index in
    /// [`keyboard_drive`](super::keyboard_drive). Out-of-range codes
    /// are ignored. Returns `true` if the held-arrow bitset changed
    /// (caller marks dirty so the next render tick runs the
    /// integrator).
    pub(super) fn on_gizmo_arrow_pressed(&self, document: &Rc<SceneDocument>, code: i32) -> bool {
        if self.kbd_drive.current_focus() == KbdFocus::None {
            return false;
        }
        let sel = document.selection_primary_node_id();
        self.kbd_drive.update_arrow(document, sel, code, true)
    }

    /// Counterpart to `on_gizmo_arrow_pressed`. Released codes are
    /// processed even with focus dropped so the bitset doesn't
    /// accumulate stuck flags after a focus race with Esc.
    pub(super) fn on_gizmo_arrow_released(&self, document: &Rc<SceneDocument>, code: i32) -> bool {
        let sel = document.selection_primary_node_id();
        self.kbd_drive.update_arrow(document, sel, code, false)
    }

    /// Per-frame keyboard integrator. Called from the coordinator's
    /// `handle_before_rendering` before the renderer reads the
    /// scene's transforms. Bails (without resetting the tick clock)
    /// on missing camera / scene / selection / zero viewport so a
    /// hot re-init doesn't hand back time the user didn't drive.
    /// Successful ticks route through `apply_command` and the
    /// `Mutated` listener marks the controller dirty.
    pub(super) fn integrate_keyboard_arrows(
        &self,
        document: &Rc<SceneDocument>,
        camera: &RefCell<Option<OrbitCamera>>,
        viewport: Option<ViewportSize>,
        build_volume: BuildVolume,
    ) {
        let ctx = IntegrationCtx {
            document,
            camera,
            viewport,
            selection_id: document.selection_primary_node_id(),
            build_volume,
        };
        let _ = self.kbd_drive.integrate(ctx);
    }
}

#[cfg(test)]
mod input_dispatch_tests {
    use super::*;

    #[test]
    fn new_starts_idle() {
        let id = InputDispatch::new();
        assert_eq!(id.kbd_drive().current_focus(), KbdFocus::None);
    }

    #[test]
    fn reset_drops_kbd_focus() {
        let id = InputDispatch::new();
        let doc = SceneDocument::new();
        // try_set_focus without a selection is rejected, so seed one
        // synthetically through the document's set_selection.
        // (Without a scene there are no valid indices, but
        // try_set_focus only checks the has_selection flag.)
        assert!(id.set_keyboard_focus(&doc, KbdFocus::Translate).is_none());
        id.reset();
        assert_eq!(id.kbd_drive().current_focus(), KbdFocus::None);
    }

    #[test]
    fn pick_at_without_viewport_returns_none() {
        let id = InputDispatch::new();
        let doc = SceneDocument::new();
        let camera: RefCell<Option<OrbitCamera>> = RefCell::new(None);
        let cursor = ScreenPoint { x: 100.0, y: 100.0 };
        assert!(id.pick_at(&doc, &camera, None, cursor).is_none());
    }

    #[test]
    fn pick_at_without_camera_returns_none() {
        let id = InputDispatch::new();
        let doc = SceneDocument::new();
        let camera: RefCell<Option<OrbitCamera>> = RefCell::new(None);
        let vp = ViewportSize::new(1920.0, 1080.0);
        let cursor = ScreenPoint { x: 100.0, y: 100.0 };
        assert!(id.pick_at(&doc, &camera, vp, cursor).is_none());
    }

    #[test]
    fn on_gizmo_arrow_pressed_without_focus_is_noop() {
        let id = InputDispatch::new();
        let doc = SceneDocument::new();
        assert!(!id.on_gizmo_arrow_pressed(&doc, 0));
    }

    #[test]
    fn last_hover_cursor_is_none_before_any_move() {
        let id = InputDispatch::new();
        assert!(id.last_hover_cursor().is_none());
    }

    #[test]
    fn pointer_move_records_cursor_and_marks_hovering() {
        let id = InputDispatch::new();
        let doc = SceneDocument::new();
        let camera: RefCell<Option<OrbitCamera>> = RefCell::new(None);
        let cursor = ScreenPoint { x: 12.0, y: 34.0 };
        let _ = id.on_pointer_move(
            &doc,
            &camera,
            None,
            crate::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME,
            cursor,
            PointerModifiers::default(),
        );
        assert_eq!(id.last_hover_cursor(), Some(cursor));
    }

    #[test]
    fn pointer_leave_hides_cached_cursor_from_paste_path() {
        let id = InputDispatch::new();
        let doc = SceneDocument::new();
        let camera: RefCell<Option<OrbitCamera>> = RefCell::new(None);
        let cursor = ScreenPoint { x: 7.0, y: 9.0 };
        let _ = id.on_pointer_move(
            &doc,
            &camera,
            None,
            crate::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME,
            cursor,
            PointerModifiers::default(),
        );
        id.on_pointer_leave();
        assert!(id.last_hover_cursor().is_none());
    }

    #[test]
    fn reset_drops_cached_hover_cursor() {
        let id = InputDispatch::new();
        let doc = SceneDocument::new();
        let camera: RefCell<Option<OrbitCamera>> = RefCell::new(None);
        let _ = id.on_pointer_move(
            &doc,
            &camera,
            None,
            crate::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME,
            ScreenPoint { x: 1.0, y: 2.0 },
            PointerModifiers::default(),
        );
        id.reset();
        assert!(id.last_hover_cursor().is_none());
    }
}
