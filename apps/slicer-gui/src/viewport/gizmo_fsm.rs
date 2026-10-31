/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Owner of the gizmo drag FSM cell. Drives the press → drag-update → release →
//! abort lifecycle by hit-testing on press, applying the per-op delta on move,
//! and reverting the captured original on abort. The pure-data types
//! (`GizmoState`, `DragOp`, `HandleHit`, `ReanchorCtx`) plus the per-frame
//! compose helpers (`apply_drag_move`, `re_anchor_scale_axis`) live in
//! `viewport::gizmo_drag`; this module hosts only the cell and the
//! controller-side coordination glue.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;

use crate::PointerModifiers;
use crate::camera::OrbitCamera;
use crate::gizmo;
use crate::mesh_render::{GizmoHover, default_fov_y};
use crate::picking;
use crate::scene_document::{SceneDocument, SceneEdit};

use super::gizmo_drag::{
    DragOp, GizmoState, HandleHit, ReanchorCtx, apply_drag_move, re_anchor_scale_axis,
};
use super::group_move::{GroupMove, group_pivot};
use super::types::{ScreenPoint, ViewportSize};

/// Per-press inputs carried into [`GizmoFsm::try_start_drag`].
pub(super) struct StartCtx<'a> {
    pub(super) document: &'a Rc<SceneDocument>,
    pub(super) camera: &'a RefCell<Option<OrbitCamera>>,
    pub(super) viewport: Option<ViewportSize>,
    pub(super) selection_id: Option<NodeId>,
    /// Every selected leaf. With more than one, the gizmo stands at their
    /// shared centre and only its arrows grab, moving them all (#13).
    pub(super) group: &'a [NodeId],
    pub(super) press: ScreenPoint,
    pub(super) modifiers: PointerModifiers,
}

/// Per-move inputs carried into [`GizmoFsm::on_move`].
pub(super) struct MoveCtx<'a> {
    pub(super) document: &'a Rc<SceneDocument>,
    pub(super) camera: &'a RefCell<Option<OrbitCamera>>,
    pub(super) viewport: Option<ViewportSize>,
    pub(super) cursor: ScreenPoint,
    pub(super) modifiers: PointerModifiers,
}

/// Per-hover inputs carried into [`GizmoFsm::update_hover`]. Same shape as
/// `StartCtx` minus the modifier (hover doesn't need it).
pub(super) struct HoverCtx<'a> {
    pub(super) document: &'a Rc<SceneDocument>,
    pub(super) camera: &'a RefCell<Option<OrbitCamera>>,
    pub(super) viewport: Option<ViewportSize>,
    pub(super) selection_id: Option<NodeId>,
    /// Every selected leaf, as in [`StartCtx::group`].
    pub(super) group: &'a [NodeId],
    pub(super) cursor: ScreenPoint,
}

pub(super) struct GizmoFsm {
    state: Cell<GizmoState>,
    hover: Cell<Option<GizmoHover>>,
    /// The leaves a drag of several selected objects moves; `None` for a
    /// drag of one.
    group: RefCell<Option<Rc<GroupMove>>>,
}

impl GizmoFsm {
    pub(super) fn new() -> Self {
        Self {
            state: Cell::new(GizmoState::Idle),
            hover: Cell::new(None),
            group: RefCell::new(None),
        }
    }

    pub(super) fn current(&self) -> GizmoState {
        self.state.get()
    }

    pub(super) fn current_hover(&self) -> Option<GizmoHover> {
        self.hover.get()
    }

    /// Force the FSM back to `Idle` without reverting any in-flight transform.
    /// Used by the scene-close path (the object is gone, so reversion is moot).
    /// The drag-end path goes through `commit_drag` so the gesture lands on the
    /// undo stack.
    pub(super) fn set_idle(&self) {
        self.state.set(GizmoState::Idle);
        self.group.replace(None);
    }

    /// Drag-end commit: if a drag is in flight, push the `before → after` pair
    /// onto the document's undo stack and transition to `Idle`. No-op edits
    /// (object's transform unchanged since press, or the object got removed
    /// mid-drag) are filtered inside `SceneDocument::push_edit`. Returns `true`
    /// if a drag was in flight when called.
    pub(super) fn commit_drag(&self, document: &Rc<SceneDocument>) -> bool {
        let GizmoState::Drag {
            original, node_id, ..
        } = self.state.get()
        else {
            return false;
        };
        self.state.set(GizmoState::Idle);
        if let Some(group) = self.group.take() {
            group.commit(document);
            return true;
        }
        let after =
            document.with_scene(|spatial| spatial.leaf(node_id).map(|q| q.local_transform()));
        if let Some(Some(after)) = after {
            document.push_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: original,
                after,
            });
        }
        true
    }

    /// Drop any cached hover. Returns `true` if the cell was set. Used when the
    /// pointer leaves the viewport or selection clears.
    pub(super) fn clear_hover(&self) -> bool {
        self.hover.replace(None).is_some()
    }

    /// Test-only setter used by the integration tests in `controller.rs` to
    /// drive the FSM directly into specific drag states without going through
    /// `try_start_drag`.
    #[cfg(test)]
    pub(super) fn force_set(&self, state: GizmoState) {
        self.state.set(state);
    }

    /// Runs the three gizmo hit tests against the current camera + cursor ray
    /// and, if a handle is grabbed, transitions the FSM into `Drag`. The
    /// front-most candidate (smallest positive `t` along the press ray) wins
    /// across translate cones, rotate arcs, and scale midpoint cubes. Bails on
    /// missing camera / scene / no selection / zero viewport, in which case the
    /// press behaves like a plain click candidate.
    ///
    /// `modifiers.shift` at press time captures into the resulting
    /// `ScaleAxis.proportional` flag: a press on the per-axis cube with Shift
    /// held starts uniformly; subsequent toggles re-anchor through the move
    /// path.
    pub(super) fn try_start_drag(&self, ctx: StartCtx<'_>) {
        let Some(sel_id) = ctx.selection_id else {
            return;
        };
        let Some(vp) = ctx.viewport else {
            return;
        };
        let (w, h) = (vp.width(), vp.height());
        let fov_y = default_fov_y();
        let x = ctx.press.x;
        let y = ctx.press.y;
        let several = ctx.group.len() > 1;

        // Compute the drag-start under both borrows; both drop at the closure's
        // end, so `state.set` below can run without overlap.
        let computed: Option<(DragOp, Transform3D, glam::Vec3)> = ctx
            .document
            .with_scene(|spatial| {
                let camera_slot = ctx.camera.borrow();
                let camera = camera_slot.as_ref()?;
                let pivot = gizmo_pivot(spatial, sel_id, ctx.group)?;
                let ray = picking::ray_from_pixel(camera, fov_y, w, h, x, y);
                let (winner, _) = handle_under(&ray, camera, pivot, fov_y, h, several)?;
                let original = spatial.leaf(sel_id)?.local_transform();

                let op = match winner {
                    HandleHit::Translate(axis) => {
                        // `closest_point_on_axis` returns `None` only inside
                        // the translate lock-out, which `hit_test` already
                        // rejected; bail defensively if it slips through,
                        // leaving the press as a click candidate.
                        let anchor = gizmo::drag_math::closest_point_on_axis(&ray, pivot, axis)?;
                        DragOp::Translate { axis, anchor }
                    }
                    HandleHit::Rotate(axis) => {
                        // `rotate_drag_angle` returns `None` only inside the
                        // arc lock-out, which `rotate_hit_test` already
                        // rejected; but bail defensively if it slips through,
                        // leaving the press as a click candidate.
                        let start_angle = gizmo::drag_math::rotate_drag_angle(&ray, pivot, axis)?;
                        DragOp::Rotate {
                            axis,
                            start_angle,
                            last_raw: start_angle,
                            current_unwrapped: start_angle,
                        }
                    }
                    HandleHit::ScaleAxis(axis) => {
                        let start_anchor =
                            gizmo::drag_math::closest_point_on_axis(&ray, pivot, axis)?;
                        // The uniform-branch anchors are captured at press as
                        // well, even when `proportional` starts false; that way
                        // a mid-drag Shift press flips the formula through the
                        // same `re_anchor_scale_axis` path either direction
                        // without needing a "first-toggle" special case.
                        let pivot_screen =
                            picking::project_world_to_screen(camera, fov_y, w, h, pivot)?;
                        DragOp::ScaleAxis {
                            axis,
                            start_anchor,
                            current_anchor: start_anchor,
                            proportional: ctx.modifiers.shift,
                            press_screen: (x, y),
                            pivot_screen,
                        }
                    }
                };
                Some((op, original, pivot))
            })
            .flatten();

        let Some((op, original, pivot)) = computed else {
            return;
        };
        let group = if several {
            match GroupMove::capture(ctx.document, ctx.group) {
                Some(group) => Some(Rc::new(group)),
                None => return,
            }
        } else {
            None
        };
        self.group.replace(group);
        self.state.set(GizmoState::Drag {
            op,
            original,
            pivot,
            node_id: sel_id,
        });
    }

    /// Pointer-move drag-update step. Branches on the active `DragOp`,
    /// recomputes the per-op delta from the current cursor ray, and composes
    /// the resulting transform onto the captured original. If a `ScaleAxis`
    /// drag is live and the live Shift state has flipped since the last frame,
    /// re-anchors the drag (current transform → new `original`, current cursor
    /// → new anchors) so the per-axis ↔ uniform formula switch is visually
    /// snap-free.
    ///
    /// Returns `true` if the scene's transform was updated. Callers mark the
    /// controller dirty on `true`.
    pub(super) fn on_move(&self, ctx: MoveCtx<'_>) -> bool {
        let GizmoState::Drag {
            mut op,
            mut original,
            pivot,
            node_id,
        } = self.state.get()
        else {
            return false;
        };
        let Some(vp) = ctx.viewport else {
            return false;
        };

        let x = ctx.cursor.x;
        let y = ctx.cursor.y;

        // Compute the new transform and the (possibly mutated) op state inside
        // the document's read borrow + camera borrow. Both drop at the
        // closure's end before the scene write below.
        let result: Option<(Transform3D, DragOp)> = ctx
            .document
            .with_scene(|spatial| {
                let camera_slot = ctx.camera.borrow();
                let camera = camera_slot.as_ref()?;
                // Live Shift toggle on a `ScaleAxis` drag re-anchors before the
                // move's delta is computed: the running transform becomes the
                // new `original`, the current cursor becomes the new press, so
                // the per-axis ↔ uniform formula switch does not snap the
                // object.
                if let DragOp::ScaleAxis { proportional, .. } = op
                    && proportional != ctx.modifiers.shift
                    && let Some(running) = spatial.leaf(node_id).map(|q| q.local_transform())
                {
                    let reanchor_ctx = ReanchorCtx {
                        camera,
                        pivot,
                        fov_y: default_fov_y(),
                        viewport: (vp.width(), vp.height()),
                        cursor: (x, y),
                    };
                    if let Some(re_anchored) =
                        re_anchor_scale_axis(op, running, &reanchor_ctx, ctx.modifiers.shift)
                    {
                        op = re_anchored.0;
                        original = re_anchored.1;
                    }
                }
                let ray =
                    picking::ray_from_pixel(camera, default_fov_y(), vp.width(), vp.height(), x, y);
                let parent_world = spatial
                    .leaf(node_id)
                    .map(|q| q.parent_world())
                    .unwrap_or(Transform3D::Identity);
                apply_drag_move(op, &ray, pivot, &parent_world, &original, x, y)
            })
            .flatten();

        let Some((new_transform, new_op)) = result else {
            return false;
        };

        // Translation is free, also out of the plate: the out-of-volume
        // warning marks it instead of holding it back. Rotation and scale
        // receive only a floor lift, in the same undoable edit.
        let new_transform = if matches!(new_op, DragOp::Translate { .. }) {
            new_transform
        } else {
            ctx.document
                .with_scene(|spatial| {
                    super::bed_clearance::keep_above_bed(spatial, node_id, original, new_transform)
                })
                .unwrap_or(new_transform)
        };

        // Persist the (possibly updated) drag state: Rotate's unwrap counter
        // mutates per move; the others' state is captured at press and stays
        // put. `original` may also have been reset by a Shift re-anchor on this
        // frame.
        self.state.set(GizmoState::Drag {
            op: new_op,
            original,
            pivot,
            node_id,
        });

        // Several objects: each moves by the offset the arrow gives the
        // primary one.
        // The cell is not borrowed while the edit notifies the listeners.
        let group = self.group.borrow().clone();
        if let Some(group) = group {
            let offset = ctx
                .document
                .with_scene(|spatial| {
                    let parent = spatial.leaf(node_id)?.parent_world();
                    world_offset(&parent.compose(&original), &parent.compose(&new_transform))
                })
                .flatten();
            return offset.is_some_and(|d| group.apply(ctx.document, d).is_ok());
        }

        ctx.document
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: original,
                after: new_transform,
            })
            .is_ok()
    }

    /// Aborts any in-progress drag and reverts the affected object's transform
    /// to its captured original. Returns `true` if a drag was in flight (caller
    /// clears the input phase; the `Mutated` event from `apply_edit` drives the
    /// redraw).
    pub(super) fn abort(&self, document: &Rc<SceneDocument>) -> bool {
        let GizmoState::Drag {
            original, node_id, ..
        } = self.state.get()
        else {
            return false;
        };
        self.state.set(GizmoState::Idle);
        if let Some(group) = self.group.take() {
            group.abort(document);
            return true;
        }
        // The current transform on the leaf is whatever the last drag tick
        // wrote; we need it as the `before` so the edit describes "from
        // current, back to original."
        let current = document
            .with_scene(|spatial| spatial.leaf(node_id).map(|q| q.local_transform()))
            .flatten()
            .unwrap_or(original);
        // A rejected revert means the tick did not land; the abort still ends
        // the drag either way.
        let _ = document.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: current,
            after: original,
        });
        true
    }

    /// Recomputes the hover-handle under the cursor and stores it on the FSM.
    /// No-op while a drag is in progress (the dragged handle is already at full
    /// brightness, and hover would just race with the drag state). Returns
    /// `true` if the cached hover changed, the caller marks the renderer dirty
    /// so the new tint takes effect on the next frame.
    pub(super) fn update_hover(&self, ctx: HoverCtx<'_>) -> bool {
        if matches!(self.state.get(), GizmoState::Drag { .. }) {
            return false;
        }
        let new_hover = compute_hover(&ctx);
        if self.hover.get() == new_hover {
            return false;
        }
        self.hover.set(new_hover);
        true
    }
}

/// Runs the three gizmo hit-tests (translate cones, rotate cones, scale
/// midpoint cubes) against `ray` and returns the front-most candidate. Shared
/// by `try_start_drag` (which converts the winner to a `DragOp`) and the hover
/// path (which converts it to a `GizmoHover`).
fn front_most_handle(
    ray: &picking::Ray,
    camera: &OrbitCamera,
    pivot: glam::Vec3,
    fov_y: f32,
    h: f32,
) -> Option<(HandleHit, f32)> {
    let scale = gizmo::gizmo_scale(camera, pivot, fov_y, h)?;
    let cyl_radius = gizmo::hit_radius_world(camera, pivot, fov_y, h)?;
    let arc_radius_world = scale * gizmo::dimensions::ROTATE_ARC_RADIUS_FRACTION;
    // Rotate cones and scale cubes scale with the gizmo: handle-local sizes (in
    // `gizmo` constants) map to world space via the per-frame shaft length.
    let rotate_cone_length_world = scale * gizmo::dimensions::ROTATE_CONE_LENGTH;
    let rotate_cone_radius_world = scale * gizmo::dimensions::ROTATE_CONE_RADIUS;
    let world_per_pixel = camera.world_per_pixel_at(pivot, fov_y, h)?;
    let axis_cube_world = world_per_pixel * gizmo::dimensions::AXIS_SCALE_CUBE_PIXELS;

    let mut best: Option<(HandleHit, f32)> = None;
    let mut consider = |hit: HandleHit, t: f32| match best {
        Some((_, prev_t)) if prev_t <= t => {}
        _ => best = Some((hit, t)),
    };
    if let Some((axis, t)) = gizmo::hit_test::hit_test(ray, pivot, scale, cyl_radius) {
        consider(HandleHit::Translate(axis), t);
    }
    if let Some((axis, t)) = gizmo::hit_test::rotate_hit_test(
        ray,
        pivot,
        arc_radius_world,
        rotate_cone_length_world,
        rotate_cone_radius_world,
    ) {
        consider(HandleHit::Rotate(axis), t);
    }
    if let Some((axis, t)) = gizmo::hit_test::scale_hit_test(ray, pivot, scale, axis_cube_world) {
        consider(HandleHit::ScaleAxis(axis), t);
    }
    best
}

/// [`front_most_handle`], or the front-most arrow alone when `move_only`:
/// with several objects selected the arrows are the only handles drawn,
/// since turning and scaling them together waits for #218.
fn handle_under(
    ray: &picking::Ray,
    camera: &OrbitCamera,
    pivot: glam::Vec3,
    fov_y: f32,
    h: f32,
    move_only: bool,
) -> Option<(HandleHit, f32)> {
    if !move_only {
        return front_most_handle(ray, camera, pivot, fov_y, h);
    }
    let scale = gizmo::gizmo_scale(camera, pivot, fov_y, h)?;
    let cyl_radius = gizmo::hit_radius_world(camera, pivot, fov_y, h)?;
    gizmo::hit_test::hit_test(ray, pivot, scale, cyl_radius)
        .map(|(axis, t)| (HandleHit::Translate(axis), t))
}

/// Pure-data hover computation: derives the front-most handle under the cursor
/// without touching FSM state. `None` whenever the prereqs (selection / scene /
/// camera / viewport / pivot) are missing or the cursor isn't over any handle.
fn compute_hover(ctx: &HoverCtx<'_>) -> Option<GizmoHover> {
    let sel_id = ctx.selection_id?;
    let vp = ctx.viewport?;
    let (w, h) = (vp.width(), vp.height());
    let fov_y = default_fov_y();
    ctx.document
        .with_scene(|spatial| {
            let camera_slot = ctx.camera.borrow();
            let camera = camera_slot.as_ref()?;
            let pivot = gizmo_pivot(spatial, sel_id, ctx.group)?;
            if !cursor_within_gizmo_screen_radius(camera, fov_y, w, h, pivot, ctx.cursor) {
                return None;
            }
            let ray = picking::ray_from_pixel(camera, fov_y, w, h, ctx.cursor.x, ctx.cursor.y);
            let (hit, _) = handle_under(&ray, camera, pivot, fov_y, h, ctx.group.len() > 1)?;
            Some(handle_hit_to_hover(hit))
        })
        .flatten()
}

/// Where the gizmo stands: at the selected leaf's centre, or at the centre of
/// the box around every selected leaf when there are several (#13).
pub(super) fn gizmo_pivot(
    spatial: &sc_mesh_spatial::SceneSpatial,
    primary: NodeId,
    group: &[NodeId],
) -> Option<glam::Vec3> {
    if group.len() > 1 {
        group_pivot(spatial, group)
    } else {
        gizmo::pivot_for_leaf(spatial, primary)
    }
}

/// The offset in the world between two world transforms with the same
/// linear part, as a translate drag makes.
fn world_offset(from: &Transform3D, to: &Transform3D) -> Option<[f64; 3]> {
    let at = |t: &Transform3D| match t {
        Transform3D::Identity => [0.0; 3],
        Transform3D::Affine(a) => *a.translation(),
    };
    let (a, b) = (at(from), at(to));
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    d.iter().all(|c| c.is_finite()).then_some(d)
}

/// Returns `false` when the cursor is far enough from the gizmo's projected
/// centre that no handle can be hit, allowing `compute_hover` to skip the three
/// handle ray-tests. The radius covers the rotate arc (the outermost handle)
/// plus `HANDLE_PIXEL_LENGTH` of slack so handles at the rim stay hit-able.
/// Returns `true` when the projection fails (point behind camera) so the
/// existing hit-test path runs and produces its own `None`.
fn cursor_within_gizmo_screen_radius(
    camera: &OrbitCamera,
    fov_y: f32,
    viewport_w: f32,
    viewport_h: f32,
    pivot: glam::Vec3,
    cursor: ScreenPoint,
) -> bool {
    let Some((px, py)) =
        picking::project_world_to_screen(camera, fov_y, viewport_w, viewport_h, pivot)
    else {
        return true;
    };
    let arc_radius_px =
        gizmo::dimensions::ROTATE_ARC_RADIUS_FRACTION * gizmo::dimensions::HANDLE_PIXEL_LENGTH;
    let radius_px = arc_radius_px + gizmo::dimensions::HANDLE_PIXEL_LENGTH;
    let dx = cursor.x - px;
    let dy = cursor.y - py;
    dx * dx + dy * dy <= radius_px * radius_px
}

fn handle_hit_to_hover(hit: HandleHit) -> GizmoHover {
    match hit {
        HandleHit::Translate(axis) => GizmoHover::Translate(axis),
        HandleHit::Rotate(axis) => GizmoHover::Rotate(axis),
        HandleHit::ScaleAxis(axis) => GizmoHover::ScaleAxis(axis),
    }
}

#[cfg(test)]
mod hover_pre_check_tests {
    use super::*;
    use glam::Vec3;

    fn camera_at_origin(distance: f32) -> OrbitCamera {
        let mut c = OrbitCamera::new(Vec3::ZERO, 1.0);
        c.distance = distance;
        c
    }

    fn pivot_screen(camera: &OrbitCamera, fov_y: f32, w: f32, h: f32) -> (f32, f32) {
        picking::project_world_to_screen(camera, fov_y, w, h, Vec3::ZERO).unwrap()
    }

    #[test]
    fn cursor_at_pivot_is_inside_radius() {
        let camera = camera_at_origin(10.0);
        let fov_y = default_fov_y();
        let (w, h) = (1024.0, 768.0);
        let (px, py) = pivot_screen(&camera, fov_y, w, h);
        let cursor = ScreenPoint { x: px, y: py };
        assert!(cursor_within_gizmo_screen_radius(
            &camera,
            fov_y,
            w,
            h,
            Vec3::ZERO,
            cursor,
        ));
    }

    #[test]
    fn cursor_far_outside_is_rejected() {
        let camera = camera_at_origin(10.0);
        let fov_y = default_fov_y();
        let (w, h) = (1024.0, 768.0);
        // A cursor in the corner of a 1024x768 viewport sits ~640 px from the
        // centre, well outside the rotate-arc radius (~ a few dozen px at this
        // distance) plus the HANDLE_PIXEL_LENGTH slack (120 px).
        let cursor = ScreenPoint { x: 0.0, y: 0.0 };
        assert!(!cursor_within_gizmo_screen_radius(
            &camera,
            fov_y,
            w,
            h,
            Vec3::ZERO,
            cursor,
        ));
    }

    #[test]
    fn cursor_outside_radius_just_beyond_slack_is_rejected() {
        let camera = camera_at_origin(10.0);
        let fov_y = default_fov_y();
        let (w, h) = (1024.0, 768.0);
        let radius_px = gizmo::dimensions::ROTATE_ARC_RADIUS_FRACTION
            * gizmo::dimensions::HANDLE_PIXEL_LENGTH
            + gizmo::dimensions::HANDLE_PIXEL_LENGTH;
        let (px, py) = pivot_screen(&camera, fov_y, w, h);
        // 1 pixel past the inflated radius along +x.
        let cursor = ScreenPoint {
            x: px + radius_px + 1.0,
            y: py,
        };
        assert!(!cursor_within_gizmo_screen_radius(
            &camera,
            fov_y,
            w,
            h,
            Vec3::ZERO,
            cursor,
        ));
    }

    #[test]
    fn cursor_inside_radius_passes() {
        let camera = camera_at_origin(10.0);
        let fov_y = default_fov_y();
        let (w, h) = (1024.0, 768.0);
        let radius_px = gizmo::dimensions::ROTATE_ARC_RADIUS_FRACTION
            * gizmo::dimensions::HANDLE_PIXEL_LENGTH
            + gizmo::dimensions::HANDLE_PIXEL_LENGTH;
        let (px, py) = pivot_screen(&camera, fov_y, w, h);
        // Halfway out along +x: comfortably inside the radius.
        let cursor = ScreenPoint {
            x: px + radius_px * 0.5,
            y: py,
        };
        assert!(cursor_within_gizmo_screen_radius(
            &camera,
            fov_y,
            w,
            h,
            Vec3::ZERO,
            cursor,
        ));
    }
}

#[cfg(test)]
mod front_most_handle_tests {
    use super::*;
    use crate::camera::AxisView;
    use crate::gizmo::axis::Axis;
    use crate::gizmo::dimensions::HANDLE_PIXEL_LENGTH;
    use glam::Vec3;

    #[test]
    fn x_cone_is_hit_where_it_is_drawn_off_the_orbit_target_plane() {
        // Front view from (0, -3, 0). The X cone spans 1.0 to 1.18 shaft
        // lengths along screen +x from the projected pivot. Pivots at y = -1
        // and y = +1 are in front of and behind the plane through the orbit
        // target.
        let (w, h) = (1024.0, 768.0);
        let fov_y = default_fov_y();
        let mut camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        camera.set_axis_view(AxisView::Front);
        for pivot in [Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)] {
            let (px, py) = picking::project_world_to_screen(&camera, fov_y, w, h, pivot).unwrap();
            let ray =
                picking::ray_from_pixel(&camera, fov_y, w, h, px + 1.1 * HANDLE_PIXEL_LENGTH, py);
            let hit = front_most_handle(&ray, &camera, pivot, fov_y, h).map(|(hit, _)| hit);
            assert_eq!(hit, Some(HandleHit::Translate(Axis::X)), "pivot {pivot}");
        }
    }
}
