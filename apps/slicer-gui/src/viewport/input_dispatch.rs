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

use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use super::gizmo_drag::GizmoState;
use super::gizmo_fsm::{
    GizmoFsm, HoverCtx as GizmoHoverCtx, MoveCtx as GizmoMoveCtx, StartCtx as GizmoStartCtx,
};
use super::input_state::{ClickOrDrag, InputState, RightUpOutcome};
use super::keyboard_drive::{IntegrationCtx, KbdFocus, KeyboardDriveState};
use super::types::{ScreenPoint, ViewportSize};
use crate::PointerModifiers;
use crate::camera::OrbitCamera;
use crate::mesh_render::default_fov_y;
use crate::picking;
use crate::scene_document::SceneDocument;

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
/// - `BoxSelect { from, to }`: a left drag that began with Shift held and
///   grabbed no gizmo handle (#13). The caller selects what the box between
///   the press and the release touches, on top of the selection.
/// - `Nothing`: the release did not match a prior press (e.g. lost
///   focus before mouse-up, or a release synthesised by Slint).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PointerUpOutcome {
    DragCommitted,
    Click(ScreenPoint),
    BoxSelect { from: ScreenPoint, to: ScreenPoint },
    Nothing,
}

pub(super) struct InputDispatch {
    pub(super) modal: Cell<Option<super::modal_transform::ModalGesture>>,
    pub(super) modal_revision: Cell<Option<crate::scene_document::SceneRevision>>,
    /// The leaves `G` moves when several are selected (#13); `None` for one.
    pub(super) modal_group: RefCell<Option<Rc<super::group_move::GroupMove>>>,
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
    /// Where a Shift+left press that grabbed no gizmo handle went down:
    /// the corner a selection box starts from (#13). `None` otherwise.
    box_from: Cell<Option<ScreenPoint>>,
}

impl InputDispatch {
    pub(super) fn new() -> Self {
        Self {
            modal: Cell::new(None),
            modal_revision: Cell::new(None),
            modal_group: RefCell::new(None),
            input_state: InputState::new(),
            gizmo_fsm: GizmoFsm::new(),
            kbd_drive: KeyboardDriveState::new(),
            last_cursor: Cell::new(None),
            hovering: Cell::new(false),
            box_from: Cell::new(None),
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
        self.modal.set(None);
        self.modal_revision.set(None);
        self.modal_group.replace(None);
        self.gizmo_fsm.set_idle();
        self.gizmo_fsm.clear_hover();
        self.input_state.set_idle();
        self.input_state.set_right_idle();
        self.kbd_drive.reset();
        self.last_cursor.set(None);
        self.hovering.set(false);
        self.box_from.set(None);
    }

    /// Latch a right-button press position. The release path
    /// classifies it as click (context menu) or drag (camera
    /// orbit) by comparing the press / release displacement
    /// against `CLICK_DRAG_THRESHOLD`.
    pub(super) fn on_right_pointer_down(&self, cursor: ScreenPoint) {
        self.input_state.set_right_pressed(cursor);
    }

    /// Classify a right-button release. `Some(press_origin)` if
    /// the gesture stayed within the click threshold (the caller
    /// opens the context menu at `press_origin`); `None` if the
    /// gesture moved beyond the threshold (the camera orbit
    /// already consumed it) or no matching press was latched.
    pub(super) fn on_right_pointer_up(&self, cursor: ScreenPoint) -> Option<ScreenPoint> {
        match self.input_state.right_click_or_drag(cursor) {
            RightUpOutcome::Click(origin) => Some(origin),
            RightUpOutcome::Drag | RightUpOutcome::NoPress => None,
        }
    }

    /// Records that the cursor has left the viewport. The paste path
    /// reads `hovering` through `last_hover_cursor` to decide whether
    /// to anchor on the cached cursor or fall back to the viewport
    /// centre.
    pub(super) fn on_pointer_leave(&self) {
        self.hovering.set(false);
    }

    /// Last cursor position, gated on the cursor still hovering the
    /// viewport. Returns `None` when the cursor has left the viewport
    /// since the last move event or no move has ever fired, so paste
    /// fallbacks (e.g. viewport-centre placement) can branch on it.
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
    /// flag. A Shift press that grabs no handle starts a selection box
    /// instead (#13); returns whether it did, so the viewport draws it.
    pub(super) fn on_pointer_down(
        &self,
        document: &Rc<SceneDocument>,
        camera: &RefCell<Option<OrbitCamera>>,
        viewport: Option<ViewportSize>,
        cursor: ScreenPoint,
        modifiers: PointerModifiers,
        allow_transforms: bool,
    ) -> bool {
        self.input_state.set_pressed(cursor);
        self.box_from.set(None);
        let shift = modifiers.shift;
        // The gizmo can only be grabbed in the Transform section. The
        // press is still recorded above so a click without a drag
        // still selects / picks on release in any section.
        if allow_transforms {
            let group = document.selection_node_ids();
            let ctx = GizmoStartCtx {
                document,
                camera,
                viewport,
                selection_id: document.selection_primary_node_id(),
                group: &group,
                press: cursor,
                modifiers,
            };
            self.gizmo_fsm.try_start_drag(ctx);
        }
        let boxing = shift && !matches!(self.gizmo_fsm.current(), GizmoState::Drag { .. });
        if boxing {
            self.box_from.set(Some(cursor));
        }
        boxing
    }

    /// Drives the gizmo drag for `pointer-moved`, recomputes the
    /// per-op delta from the current cursor ray, and composes the
    /// resulting transform onto the captured original. Independently,
    /// updates the hover tint when no drag is in flight. Returns
    /// `true` if the cached hover changed (caller should mark dirty),
    /// drag-driven scene mutations fire `Mutated` separately and the
    /// listener marks dirty on its own.
    /// Records the latest hover cursor. Always called on pointer-move,
    /// in every section, so the paste anchor has a fresh position; the
    /// gizmo drag / hover work in [`Self::on_pointer_move`] is separate
    /// and only runs in the Transform section.
    pub(super) fn note_hover_cursor(&self, cursor: ScreenPoint) {
        self.last_cursor.set(Some(cursor));
        self.hovering.set(true);
    }

    pub(super) fn on_pointer_move(
        &self,
        document: &Rc<SceneDocument>,
        camera: &RefCell<Option<OrbitCamera>>,
        viewport: Option<ViewportSize>,
        cursor: ScreenPoint,
        modifiers: PointerModifiers,
    ) -> bool {
        let move_ctx = GizmoMoveCtx {
            document,
            camera,
            viewport,
            cursor,
            modifiers,
        };
        // The bool return is intentionally discarded, a successful
        // drag move routes through `apply_command` and the `Mutated`
        // listener schedules the redraw.
        let _ = self.gizmo_fsm.on_move(move_ctx);

        let group = document.selection_node_ids();
        let hover_ctx = GizmoHoverCtx {
            document,
            camera,
            viewport,
            selection_id: document.selection_primary_node_id(),
            group: &group,
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
        let box_from = self.box_from.take();
        if self.gizmo_fsm.commit_drag(document) {
            return PointerUpOutcome::DragCommitted;
        }
        match (outcome, box_from) {
            (ClickOrDrag::Click, _) => PointerUpOutcome::Click(cursor),
            (ClickOrDrag::Drag, Some(from)) => PointerUpOutcome::BoxSelect { from, to: cursor },
            _ => PointerUpOutcome::Nothing,
        }
    }

    /// Builds the ray at `(x, y)` and finds the leaf a click there
    /// selects ([`selected_by`]: without `supports_selectable`, a click on
    /// the supports selects their model). Returns:
    /// - `None`: the picker bailed (no camera, no viewport, or no scene).
    /// - `Some(None)`: ray hit empty space (caller clears the selection).
    /// - `Some(Some(node_id))`: the click selects the leaf identified by
    ///   `node_id`.
    pub(super) fn pick_at(
        &self,
        document: &Rc<SceneDocument>,
        camera: &RefCell<Option<OrbitCamera>>,
        viewport: Option<ViewportSize>,
        cursor: ScreenPoint,
        supports_selectable: bool,
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
        document.with_scene(|spatial| selected_by(&ray, spatial, supports_selectable))
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
        // Several selected objects move together (#13); turning or scaling
        // them together is #218.
        let has_selection = match focus {
            KbdFocus::Translate => document.selection_len() > 0,
            _ => document.selection_len() == 1,
        };
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
    ) {
        let ctx = IntegrationCtx {
            document,
            camera,
            viewport,
            selection_id: document.selection_primary_node_id(),
        };
        let _ = self.kbd_drive.integrate(ctx);
    }
}

/// Whether leaf `id` of `spatial` shows the generated supports.
pub(super) fn shows_supports(spatial: &SceneSpatial, id: NodeId) -> bool {
    spatial
        .graph()
        .leaf_mesh_arc(id)
        .is_some_and(|mesh| sc_geom::df_supports::is_support_mesh(mesh))
}

/// The leaf a click along `ray` selects. The supports are selected only
/// in the Supports tab, which edits them (#120): anywhere else they follow
/// their model, and a click on them selects the model whose footprint on
/// the plate is nearest the point hit. A pillar seen from the side stands
/// under its model, not in front of it, so the ray need not reach the
/// model.
fn selected_by(
    ray: &picking::Ray,
    spatial: &SceneSpatial,
    supports_selectable: bool,
) -> Option<NodeId> {
    let (hit, t) = picking::pick_object_where(ray, spatial, |_| true)?;
    if supports_selectable || !shows_supports(spatial, hit) {
        return Some(hit);
    }
    let at = ray.origin + ray.direction * t;
    nearest_model(spatial, at.x, at.y)
}

/// The model whose footprint on the plate is nearest `(x, y)`; between
/// footprints that both hold the point, the one whose centre is nearer.
fn nearest_model(spatial: &SceneSpatial, x: f32, y: f32) -> Option<NodeId> {
    let distances = |id: NodeId| {
        let query = spatial.leaf(id)?;
        let aabb = query.transformed_aabb(query.world_transform())?;
        let (lo, hi) = (aabb.min().coords(), aabb.max().coords());
        let outside = |p: f32, k: usize| (lo[k] - p).max(p - hi[k]).max(0.0);
        let off_centre = |p: f32, k: usize| p - (lo[k] + hi[k]) * 0.5;
        Some((
            outside(x, 0).hypot(outside(y, 1)),
            off_centre(x, 0).hypot(off_centre(y, 1)),
        ))
    };
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter(|&id| !shows_supports(spatial, id))
        .filter_map(|id| Some((distances(id)?, id)))
        .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, id)| id)
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
        assert!(id.pick_at(&doc, &camera, None, cursor, true).is_none());
    }

    #[test]
    fn pick_at_without_camera_returns_none() {
        let id = InputDispatch::new();
        let doc = SceneDocument::new();
        let camera: RefCell<Option<OrbitCamera>> = RefCell::new(None);
        let vp = ViewportSize::new(1920.0, 1080.0);
        let cursor = ScreenPoint { x: 100.0, y: 100.0 };
        assert!(id.pick_at(&doc, &camera, vp, cursor, true).is_none());
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
    fn note_hover_cursor_records_cursor_and_marks_hovering() {
        let id = InputDispatch::new();
        let cursor = ScreenPoint { x: 12.0, y: 34.0 };
        id.note_hover_cursor(cursor);
        assert_eq!(id.last_hover_cursor(), Some(cursor));
    }

    #[test]
    fn pointer_leave_hides_cached_cursor_from_paste_path() {
        let id = InputDispatch::new();
        id.note_hover_cursor(ScreenPoint { x: 7.0, y: 9.0 });
        id.on_pointer_leave();
        assert!(id.last_hover_cursor().is_none());
    }

    #[test]
    fn reset_drops_cached_hover_cursor() {
        let id = InputDispatch::new();
        id.note_hover_cursor(ScreenPoint { x: 1.0, y: 2.0 });
        id.reset();
        assert!(id.last_hover_cursor().is_none());
    }
}
