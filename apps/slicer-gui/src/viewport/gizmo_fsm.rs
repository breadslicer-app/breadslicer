/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Owner of the gizmo drag FSM cell. Drives the press → drag-update → release →
//! abort lifecycle by hit-testing on press, applying the per-op delta on move,
//! and reverting the captured original on abort. The pure-data types
//! (`GizmoState`, `DragOp`, `HandleHit`) plus the per-frame compose helper
//! (`apply_drag_move`) live in `viewport::gizmo_drag`; this module hosts only
//! the cells and the controller-side coordination glue.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;

use crate::PointerModifiers;
use crate::camera::OrbitCamera;
use crate::gizmo;
use crate::gizmo::axis::Axis;
use crate::gizmo::look::{Family, GizmoLook};
use crate::gizmo::modifiers::PrecisionCursor;
use crate::mesh_render::{GizmoHover, default_fov_y};
use crate::picking;
use crate::scene_document::{SceneDocument, SceneEdit};

use super::gizmo_drag::{DragOp, GizmoState, HandleHit, apply_drag_move};
use super::group_move::{GroupMove, group_pivot, world_delta};
use super::types::{ScreenPoint, ViewportSize};

/// Per-press inputs carried into [`GizmoFsm::try_start_drag`].
pub(super) struct StartCtx<'a> {
    pub(super) document: &'a Rc<SceneDocument>,
    pub(super) camera: &'a RefCell<Option<OrbitCamera>>,
    pub(super) viewport: Option<ViewportSize>,
    pub(super) selection_id: Option<NodeId>,
    /// Every selected leaf. With more than one, the gizmo stands at their
    /// shared centre and moves, turns or scales them all about it (#13,
    /// #218).
    pub(super) group: &'a [NodeId],
    pub(super) press: ScreenPoint,
    pub(super) modifiers: PointerModifiers,
    /// The gizmo the user picked: which handles show and how large.
    pub(super) look: GizmoLook,
}

/// Inputs of [`GizmoFsm::start_body_drag`]: the model whose body was pressed
/// (#209), where, and what moves with it.
pub(super) struct BodyCtx<'a> {
    pub(super) document: &'a Rc<SceneDocument>,
    pub(super) node_id: NodeId,
    /// The world point the press grabbed on the model.
    pub(super) grab: glam::Vec3,
    /// The selection once the press has selected the model: the model
    /// alone, or the several selected objects it belongs to.
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
    pub(super) look: GizmoLook,
}

pub(super) struct GizmoFsm {
    state: Cell<GizmoState>,
    hover: Cell<Option<GizmoHover>>,
    /// The cursor the drag in flight reads: slowed down while Shift is held.
    cursor: Cell<PrecisionCursor>,
    /// The leaves a drag of several selected objects moves; `None` for a
    /// drag of one.
    group: RefCell<Option<Rc<GroupMove>>>,
}

impl GizmoFsm {
    pub(super) fn new() -> Self {
        Self {
            state: Cell::new(GizmoState::Idle),
            hover: Cell::new(None),
            cursor: Cell::new(PrecisionCursor::new([0.0, 0.0], false)),
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

    /// Runs the gizmo hit tests against the current camera + cursor ray and,
    /// if a handle is grabbed, transitions the FSM into `Drag`. The front-most
    /// candidate (smallest positive `t` along the press ray) wins across the
    /// arrows, rings, scale cubes and the centre ball. Bails on missing camera
    /// / scene / no selection / zero viewport, in which case the press behaves
    /// like a plain click candidate.
    ///
    /// Shift held at press starts the drag fine (see `PrecisionCursor`).
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
                let (winner, _) = front_most_handle(&ray, camera, pivot, fov_y, h, &ctx.look)?;
                // The centre ball sits on the selection itself: a Shift press
                // there starts a selection box (#13) rather than a scale.
                if winner == HandleHit::ScaleUniform && ctx.modifiers.shift {
                    return None;
                }
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
                    HandleHit::Plane(normal) => {
                        // The ray cut the square's plane to grab it.
                        let anchor = gizmo::drag_math::ray_hits_drag_plane(&ray, pivot, normal)?;
                        DragOp::Plane {
                            normal,
                            origin: pivot,
                            anchor,
                            body: false,
                        }
                    }
                    HandleHit::Rotate(axis) => {
                        // `rotate_drag_angle` returns `None` only inside the
                        // arc lock-out, which `rotate_hit_test` already
                        // rejected; but bail defensively if it slips through,
                        // leaving the press as a click candidate.
                        let start_angle = gizmo::drag_math::rotate_drag_angle(&ray, pivot, axis)?;
                        // The turn is measured on screen around the projected
                        // pivot, so a circle drawn on screen turns evenly.
                        let pivot_screen =
                            picking::project_world_to_screen(camera, fov_y, w, h, pivot)?;
                        let toward_eye = if camera.orthographic {
                            camera.eye() - camera.target
                        } else {
                            camera.eye() - pivot
                        };
                        DragOp::Rotate {
                            axis,
                            start_angle,
                            turn: gizmo::drag_math::ScreenTurn::new(pivot_screen, (x, y)),
                            sign: gizmo::drag_math::screen_turn_sign(axis, toward_eye),
                            applied: 0.0,
                        }
                    }
                    HandleHit::ScaleAxis(axis) => {
                        let start_anchor =
                            gizmo::drag_math::closest_point_on_axis(&ray, pivot, axis)?;
                        DragOp::ScaleAxis {
                            axis,
                            start_anchor,
                            current_anchor: start_anchor,
                        }
                    }
                    HandleHit::ScaleUniform => DragOp::ScaleUniform { press: (x, y) },
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
        self.cursor
            .set(PrecisionCursor::new([x, y], ctx.modifiers.shift));
        self.state.set(GizmoState::Drag {
            op,
            original,
            pivot,
            node_id: sel_id,
        });
    }

    /// Starts moving the body of a model pressed and dragged past the click
    /// threshold (#209): in the horizontal plane through the grabbed point,
    /// so that point stays under the cursor, with Z unchanged. With several
    /// objects selected, they all move with it. Returns whether the drag
    /// started.
    pub(super) fn start_body_drag(&self, ctx: BodyCtx<'_>) -> bool {
        if matches!(self.state.get(), GizmoState::Drag { .. }) {
            return false;
        }
        let computed = ctx
            .document
            .with_scene(|spatial| {
                let pivot = gizmo_pivot(spatial, ctx.node_id, ctx.group)?;
                let original = spatial.leaf(ctx.node_id)?.local_transform();
                Some((pivot, original))
            })
            .flatten();
        let Some((pivot, original)) = computed else {
            return false;
        };
        let group = if ctx.group.len() > 1 {
            match GroupMove::capture(ctx.document, ctx.group) {
                Some(group) => Some(Rc::new(group)),
                None => return false,
            }
        } else {
            None
        };
        self.group.replace(group);
        self.cursor.set(PrecisionCursor::new(
            [ctx.press.x, ctx.press.y],
            ctx.modifiers.shift,
        ));
        self.state.set(GizmoState::Drag {
            op: DragOp::Plane {
                normal: Axis::Z,
                origin: ctx.grab,
                anchor: ctx.grab,
                body: true,
            },
            original,
            pivot,
            node_id: ctx.node_id,
        });
        true
    }

    /// Pointer-move drag-update step. Branches on the active `DragOp`,
    /// recomputes the per-op delta from the gesture cursor (a tenth of the
    /// travel while Shift is held), rounds it to steps while Ctrl is held, and
    /// composes the resulting transform onto the captured original.
    ///
    /// Returns `true` if the scene's transform was updated. Callers mark the
    /// controller dirty on `true`.
    pub(super) fn on_move(&self, ctx: MoveCtx<'_>) -> bool {
        let GizmoState::Drag {
            op,
            original,
            pivot,
            node_id,
        } = self.state.get()
        else {
            return false;
        };
        let Some(vp) = ctx.viewport else {
            return false;
        };

        let modifiers = super::drag_modifiers(ctx.modifiers);
        let mut precision = self.cursor.get();
        let [x, y] = precision.map([ctx.cursor.x, ctx.cursor.y], modifiers.fine);
        self.cursor.set(precision);

        // Compute the new transform and the (possibly mutated) op state inside
        // the document's read borrow + camera borrow. Both drop at the
        // closure's end before the scene write below.
        let result: Option<(Transform3D, DragOp)> = ctx
            .document
            .with_scene(|spatial| {
                let camera_slot = ctx.camera.borrow();
                let camera = camera_slot.as_ref()?;
                let ray =
                    picking::ray_from_pixel(camera, default_fov_y(), vp.width(), vp.height(), x, y);
                let parent_world = spatial
                    .leaf(node_id)
                    .map(|q| q.parent_world())
                    .unwrap_or(Transform3D::Identity);
                apply_drag_move(op, &ray, pivot, &parent_world, &original, (x, y), modifiers)
            })
            .flatten();

        let Some((new_transform, new_op)) = result else {
            return false;
        };

        // Several objects: each gets the world transform the handle gives the
        // primary one about their shared centre. The cell is not borrowed
        // while the edit notifies the listeners.
        let group = self.group.borrow().clone();
        if let Some(group) = group {
            self.state.set(GizmoState::Drag {
                op: new_op,
                original,
                pivot,
                node_id,
            });
            let delta = ctx
                .document
                .with_scene(|spatial| {
                    let parent = spatial.leaf(node_id)?.parent_world();
                    world_delta(&parent.compose(&original), &parent.compose(&new_transform))
                })
                .flatten();
            let Some(delta) = delta else {
                return false;
            };
            // As for one object: a move is free, a turn or a scale keeps the
            // group on the plate.
            let delta = if matches!(new_op, DragOp::Translate { .. } | DragOp::Plane { .. }) {
                delta
            } else {
                group.kept_on_bed(ctx.document, delta)
            };
            return group.apply_world(ctx.document, delta).is_ok();
        }

        // Translation is free, also out of the plate: the out-of-volume
        // warning marks it instead of holding it back. Rotation and scale
        // receive only a floor lift, in the same undoable edit.
        let new_transform = if matches!(new_op, DragOp::Translate { .. } | DragOp::Plane { .. }) {
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
        // put.
        self.state.set(GizmoState::Drag {
            op: new_op,
            original,
            pivot,
            node_id,
        });

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

/// Runs the gizmo hit-tests (arrows, plane squares, rings, scale cubes,
/// centre ball) against `ray` and returns the front-most candidate. Shared by
/// `try_start_drag` (which converts the winner to a `DragOp`) and the hover
/// path (which converts it to a `GizmoHover`). Only the handles `look` shows
/// count, at its size.
///
/// A scale cube sits on its own arrow's shaft and is smaller than the arrow's
/// hit cylinder, so where both hit, the cube wins whatever the `t`.
fn front_most_handle(
    ray: &picking::Ray,
    camera: &OrbitCamera,
    pivot: glam::Vec3,
    fov_y: f32,
    h: f32,
    look: &GizmoLook,
) -> Option<(HandleHit, f32)> {
    use gizmo::dimensions::{HANDLE_PIXEL_LENGTH, HANDLE_PIXEL_RADIUS, RING_HIT_PIXELS};
    let m = look.metrics();
    // One pixel of the gizmo, in world units at the pivot.
    let world_per_pixel = camera.world_per_pixel_at(pivot, fov_y, h)? * look.size;
    let scale = world_per_pixel * HANDLE_PIXEL_LENGTH;
    let cyl_radius = world_per_pixel * HANDLE_PIXEL_RADIUS;
    let ring_radius_world = scale * m.ring_radius;
    let toward_eye = if camera.orthographic {
        camera.eye() - camera.target
    } else {
        camera.eye() - pivot
    }
    .normalize_or_zero();
    let cube_world = world_per_pixel * (m.cube_px + m.cube_hit_margin_px);

    let mut best: Option<(HandleHit, f32)> = None;
    let mut consider = |hit: HandleHit, t: f32| match best {
        Some((_, prev_t)) if prev_t <= t => {}
        _ => best = Some((hit, t)),
    };
    let arrow = look
        .shows(Family::Arrows)
        .then(|| gizmo::hit_test::hit_test(ray, pivot, scale, cyl_radius))
        .flatten();
    let cube = look
        .shows(Family::Cubes)
        .then(|| gizmo::hit_test::scale_hit_test(ray, pivot, scale, cube_world, m.cube_at))
        .flatten();
    match (arrow, cube) {
        (Some((a, _)), Some((c, t))) if a == c => consider(HandleHit::ScaleAxis(c), t),
        _ => {
            if let Some((axis, t)) = arrow {
                consider(HandleHit::Translate(axis), t);
            }
            if let Some((axis, t)) = cube {
                consider(HandleHit::ScaleAxis(axis), t);
            }
        }
    }
    if look.shows(Family::Planes) {
        let squares = gizmo::hit_test::PlaneSquares {
            offset: world_per_pixel * m.plane_offset_px,
            side: world_per_pixel * m.plane_size_px,
            margin: world_per_pixel * PLANE_HIT_MARGIN_PIXELS,
            view_dir: (camera.target - camera.eye()).normalize_or_zero(),
        };
        if let Some((normal, t)) = gizmo::hit_test::plane_hit_test(ray, pivot, squares) {
            consider(HandleHit::Plane(normal), t);
        }
    }
    if look.shows(Family::Rings)
        && let Some((axis, t)) = gizmo::hit_test::rotate_hit_test(
            ray,
            pivot,
            ring_radius_world,
            world_per_pixel * RING_HIT_PIXELS,
            toward_eye,
        )
    {
        consider(HandleHit::Rotate(axis), t);
    }
    if look.shows(Family::Ball)
        && let Some(t) =
            gizmo::hit_test::centre_hit_test(ray, pivot, world_per_pixel * m.ball_hit_px)
    {
        consider(HandleHit::ScaleUniform, t);
    }
    best
}

/// How far around a plane square, in pixels, a press still grabs it.
const PLANE_HIT_MARGIN_PIXELS: f32 = 3.5;

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
            if !cursor_within_gizmo_screen_radius(camera, fov_y, w, h, pivot, ctx.cursor, &ctx.look)
            {
                return None;
            }
            let ray = picking::ray_from_pixel(camera, fov_y, w, h, ctx.cursor.x, ctx.cursor.y);
            let (hit, _) = front_most_handle(&ray, camera, pivot, fov_y, h, &ctx.look)?;
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

/// Returns `false` when the cursor is far enough from the gizmo's projected
/// centre that no handle can be hit, allowing `compute_hover` to skip the three
/// handle ray-tests. The radius covers the rotate arc (the outermost handle)
/// plus `HANDLE_PIXEL_LENGTH` of slack so handles at the rim stay hit-able,
/// at the size of `look`. Returns `true` when the projection fails (point
/// behind camera) so the existing hit-test path runs and produces its own
/// `None`.
fn cursor_within_gizmo_screen_radius(
    camera: &OrbitCamera,
    fov_y: f32,
    viewport_w: f32,
    viewport_h: f32,
    pivot: glam::Vec3,
    cursor: ScreenPoint,
    look: &GizmoLook,
) -> bool {
    let Some((px, py)) =
        picking::project_world_to_screen(camera, fov_y, viewport_w, viewport_h, pivot)
    else {
        return true;
    };
    let arc_radius_px = look.metrics().ring_radius * gizmo::dimensions::HANDLE_PIXEL_LENGTH;
    let radius_px = (arc_radius_px + gizmo::dimensions::HANDLE_PIXEL_LENGTH) * look.size;
    let dx = cursor.x - px;
    let dy = cursor.y - py;
    dx * dx + dy * dy <= radius_px * radius_px
}

fn handle_hit_to_hover(hit: HandleHit) -> GizmoHover {
    match hit {
        HandleHit::Translate(axis) => GizmoHover::Translate(axis),
        HandleHit::Plane(axis) => GizmoHover::Plane(axis),
        HandleHit::Rotate(axis) => GizmoHover::Rotate(axis),
        HandleHit::ScaleAxis(axis) => GizmoHover::ScaleAxis(axis),
        HandleHit::ScaleUniform => GizmoHover::ScaleUniform,
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
            &GizmoLook::default(),
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
            &GizmoLook::default(),
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
            &GizmoLook::default(),
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
            &GizmoLook::default(),
        ));
    }
}

#[cfg(test)]
mod front_most_handle_tests {
    use super::*;
    use crate::camera::AxisView;
    use crate::gizmo::axis::Axis;
    use crate::gizmo::dimensions::{
        HANDLE_PIXEL_LENGTH, MIDPOINT_FRACTION, ROTATE_ARC_RADIUS_FRACTION,
    };
    use glam::Vec3;

    const W: f32 = 1024.0;
    const H: f32 = 768.0;

    fn front_camera() -> OrbitCamera {
        let mut camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        camera.set_axis_view(AxisView::Front);
        camera
    }

    /// The handle under the screen point `offset` pixels from the projected
    /// pivot (screen y grows downward).
    fn pick_at(camera: &OrbitCamera, pivot: Vec3, offset: (f32, f32)) -> Option<HandleHit> {
        let fov_y = default_fov_y();
        let (px, py) = picking::project_world_to_screen(camera, fov_y, W, H, pivot).unwrap();
        let ray = picking::ray_from_pixel(camera, fov_y, W, H, px + offset.0, py + offset.1);
        front_most_handle(&ray, camera, pivot, fov_y, H, &GizmoLook::default()).map(|(hit, _)| hit)
    }

    #[test]
    fn x_cone_is_hit_where_it_is_drawn_off_the_orbit_target_plane() {
        // Front view from (0, -3, 0). Pivots at y = -1 and y = +1 are in front
        // of and behind the plane through the orbit target.
        let camera = front_camera();
        for pivot in [Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)] {
            let hit = pick_at(&camera, pivot, (1.1 * HANDLE_PIXEL_LENGTH, 0.0));
            assert_eq!(hit, Some(HandleHit::Translate(Axis::X)), "pivot {pivot}");
        }
    }

    #[test]
    fn the_shaft_grabs_along_its_length_and_the_cube_wins_where_it_sits() {
        let camera = front_camera();
        let pivot = Vec3::ZERO;
        for f in [0.25, 0.4, 0.95] {
            assert_eq!(
                pick_at(&camera, pivot, (f * HANDLE_PIXEL_LENGTH, 0.0)),
                Some(HandleHit::Translate(Axis::X)),
                "{f}"
            );
        }
        // Seen from the front, Z is up on screen: its cube sits above the pivot.
        assert_eq!(
            pick_at(
                &camera,
                pivot,
                (0.0, -MIDPOINT_FRACTION * HANDLE_PIXEL_LENGTH)
            ),
            Some(HandleHit::ScaleAxis(Axis::Z))
        );
        assert_eq!(
            pick_at(
                &camera,
                pivot,
                (MIDPOINT_FRACTION * HANDLE_PIXEL_LENGTH, 0.0)
            ),
            Some(HandleHit::ScaleAxis(Axis::X))
        );
    }

    #[test]
    fn the_ring_grabs_anywhere_on_its_circle_and_the_ball_at_the_centre() {
        // From the front the Y ring (in the XZ plane) faces the camera.
        let camera = front_camera();
        let pivot = Vec3::ZERO;
        let r = ROTATE_ARC_RADIUS_FRACTION * HANDLE_PIXEL_LENGTH;
        for deg in [20.0f32, 135.0, 200.0, 290.0] {
            let a = deg.to_radians();
            assert_eq!(
                pick_at(&camera, pivot, (r * a.cos(), r * a.sin())),
                Some(HandleHit::Rotate(Axis::Y)),
                "{deg}°"
            );
        }
        assert_eq!(
            pick_at(&camera, pivot, (2.0, 1.0)),
            Some(HandleHit::ScaleUniform)
        );
        // Between the ball and the ring, off the arrows: nothing.
        assert_eq!(pick_at(&camera, pivot, (0.4 * r, 0.4 * r)), None);
    }

    /// The CPU cost of a frame of the gizmo: the hover hit test under the
    /// cursor, which runs on every pointer move, and the draws of the frame.
    /// Run in release: `cargo test --release -p sc-slicer-gui -- --ignored
    /// measure_gizmo_frame_cost --nocapture`.
    #[test]
    #[ignore]
    fn measure_gizmo_frame_cost() {
        use crate::gizmo::look::{GizmoStyle, GizmoTool};
        use crate::mesh_render::{GizmoFrame, GizmoVisibility};
        let mut camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        camera.yaw = -2.4;
        camera.pitch = 0.67;
        let fov_y = default_fov_y();
        let n = 200_000u32;
        for style in [GizmoStyle::Outline, GizmoStyle::Focused] {
            let look = GizmoLook {
                style,
                size: 1.0,
                tool: GizmoTool::Move,
            };
            let started = std::time::Instant::now();
            let mut hits = 0u32;
            for i in 0..n {
                let x = 412.0 + (i % 200) as f32;
                let y = 284.0 + (i % 150) as f32;
                let ray = picking::ray_from_pixel(&camera, fov_y, W, H, x, y);
                hits +=
                    front_most_handle(&ray, &camera, Vec3::ZERO, fov_y, H, &look).is_some() as u32;
            }
            let hover = started.elapsed() / n;
            let frame = GizmoFrame {
                pivot: Vec3::ZERO,
                scale: 1.0,
                view_dir: (camera.target - camera.eye()).normalize(),
                visibility: GizmoVisibility::Idle,
                hover: None,
                drag_visual: None,
                look,
            };
            let started = std::time::Instant::now();
            let mut draws = 0usize;
            for _ in 0..n {
                draws += crate::mesh_render::gizmo_draw_count_for_test(&frame);
            }
            let build = started.elapsed() / n;
            println!(
                "{style:?}: hover hit test {hover:?} per move ({hits} hits), draws {build:?} per frame ({} draws)",
                draws / n as usize
            );
        }
    }
}
