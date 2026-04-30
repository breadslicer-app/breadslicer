/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Owner of the gizmo drag FSM cell. Drives the press → drag-update →
//! release → abort lifecycle by hit-testing on press, applying the
//! per-op delta on move, and reverting the captured original on
//! abort. The pure-data types (`GizmoState`, `DragOp`, `HandleHit`,
//! `ReanchorCtx`) plus the per-frame compose helpers
//! (`apply_drag_move`, `re_anchor_scale_axis`) live in
//! `viewport::gizmo_drag` — this module hosts only the cell and the
//! controller-side coordination glue.

use std::cell::{Cell, RefCell};

use sc_mesh_core::scene::{Scene, Transform3D};

use crate::PointerModifiers;
use crate::camera::OrbitCamera;
use crate::gizmo;
use crate::mesh_render::default_fov_y;
use crate::picking;

use super::gizmo_drag::{
    DragOp, GizmoState, HandleHit, ReanchorCtx, apply_drag_move, re_anchor_scale_axis,
};
use super::types::{ScreenPoint, ViewportSize};

/// Per-press inputs carried into [`GizmoFsm::try_start_drag`].
pub(super) struct StartCtx<'a> {
    pub(super) scene: &'a RefCell<Option<Scene>>,
    pub(super) camera: &'a RefCell<Option<OrbitCamera>>,
    pub(super) viewport: Option<ViewportSize>,
    pub(super) selection_idx: Option<usize>,
    pub(super) press: ScreenPoint,
    pub(super) modifiers: PointerModifiers,
}

/// Per-move inputs carried into [`GizmoFsm::on_move`].
pub(super) struct MoveCtx<'a> {
    pub(super) scene: &'a RefCell<Option<Scene>>,
    pub(super) camera: &'a RefCell<Option<OrbitCamera>>,
    pub(super) viewport: Option<ViewportSize>,
    pub(super) cursor: ScreenPoint,
    pub(super) modifiers: PointerModifiers,
}

pub(super) struct GizmoFsm {
    state: Cell<GizmoState>,
}

impl GizmoFsm {
    pub(super) fn new() -> Self {
        Self {
            state: Cell::new(GizmoState::Idle),
        }
    }

    pub(super) fn current(&self) -> GizmoState {
        self.state.get()
    }

    /// Force the FSM back to `Idle` without reverting any in-flight
    /// transform. Used by the drag-end path in `on_pointer_up` (the
    /// composed transform stays) and by the scene-close path (the
    /// object is gone, so reversion is moot).
    pub(super) fn set_idle(&self) {
        self.state.set(GizmoState::Idle);
    }

    /// Test-only setter used by the integration tests in `controller.rs`
    /// to drive the FSM directly into specific drag states without
    /// going through `try_start_drag`.
    #[cfg(test)]
    pub(super) fn force_set(&self, state: GizmoState) {
        self.state.set(state);
    }

    /// Runs the three gizmo hit tests against the current camera +
    /// cursor ray and, if a handle is grabbed, transitions the FSM
    /// into `Drag`. The front-most candidate (smallest positive `t`
    /// along the press ray) wins across translate cones, rotate arcs,
    /// and scale midpoint cubes. Bails on missing camera / scene /
    /// no selection / zero viewport, in which case the press behaves
    /// like a plain click candidate.
    ///
    /// `modifiers.shift` at press time captures into the resulting
    /// `ScaleAxis.proportional` flag — a press on the per-axis cube
    /// with Shift held starts uniformly; subsequent toggles
    /// re-anchor through the move path.
    pub(super) fn try_start_drag(&self, ctx: StartCtx<'_>) {
        let Some(sel_idx) = ctx.selection_idx else {
            return;
        };
        let Some(vp) = ctx.viewport else {
            return;
        };
        let (w, h) = (vp.width(), vp.height());

        let camera_slot = ctx.camera.borrow();
        let Some(camera) = camera_slot.as_ref() else {
            return;
        };
        let scene_slot = ctx.scene.borrow();
        let Some(scene) = scene_slot.as_ref() else {
            return;
        };
        let Some(pivot) = gizmo::pivot_for_object(scene, sel_idx) else {
            return;
        };
        let fov_y = default_fov_y();
        let scale = gizmo::gizmo_scale(camera, fov_y, h);
        let cyl_radius = gizmo::hit_radius_world(camera, fov_y, h);
        let arc_radius_world = scale * gizmo::dimensions::ROTATE_ARC_RADIUS_FRACTION;
        // Both rotate cones and scale cubes scale with the gizmo —
        // their handle-local sizes (in `gizmo` constants) become
        // world-space sizes via the per-frame shaft length.
        let rotate_cone_length_world = scale * gizmo::dimensions::ROTATE_CONE_LENGTH;
        let rotate_cone_radius_world = scale * gizmo::dimensions::ROTATE_CONE_RADIUS;
        let world_per_pixel = camera.world_per_pixel(fov_y, h);
        let axis_cube_world = world_per_pixel * gizmo::dimensions::AXIS_SCALE_CUBE_PIXELS;

        let x = ctx.press.x;
        let y = ctx.press.y;
        let ray = picking::ray_from_pixel(camera, fov_y, w, h, x, y);

        // Front-most across the four hit tests. Each candidate is
        // tagged with its category so the dispatch below knows which
        // `DragOp` variant to construct.
        let mut best: Option<(HandleHit, f32)> = None;
        let mut consider = |hit: HandleHit, t: f32| match best {
            Some((_, prev_t)) if prev_t <= t => {}
            _ => best = Some((hit, t)),
        };
        if let Some((axis, t)) = gizmo::hit_test::hit_test(&ray, pivot, scale, cyl_radius) {
            consider(HandleHit::Translate(axis), t);
        }
        if let Some((axis, t)) = gizmo::hit_test::rotate_hit_test(
            &ray,
            pivot,
            arc_radius_world,
            rotate_cone_length_world,
            rotate_cone_radius_world,
        ) {
            consider(HandleHit::Rotate(axis), t);
        }
        if let Some((axis, t)) =
            gizmo::hit_test::scale_hit_test(&ray, pivot, scale, axis_cube_world)
        {
            consider(HandleHit::ScaleAxis(axis), t);
        }
        let Some((winner, _)) = best else {
            return;
        };

        let Some(obj) = scene.objects.get(sel_idx) else {
            return;
        };
        let original = obj.transform;

        let op = match winner {
            HandleHit::Translate(axis) => {
                let anchor = gizmo::drag_math::closest_point_on_axis(&ray, pivot, axis);
                DragOp::Translate { axis, anchor }
            }
            HandleHit::Rotate(axis) => {
                // `rotate_drag_angle` returns `None` only inside the
                // arc lock-out, which `rotate_hit_test` already
                // rejected — but bail defensively if it slips
                // through, leaving the press as a click candidate.
                let Some(start_angle) = gizmo::drag_math::rotate_drag_angle(&ray, pivot, axis)
                else {
                    return;
                };
                DragOp::Rotate {
                    axis,
                    start_angle,
                    last_raw: start_angle,
                    current_unwrapped: start_angle,
                }
            }
            HandleHit::ScaleAxis(axis) => {
                let start_anchor = gizmo::drag_math::closest_point_on_axis(&ray, pivot, axis);
                // The uniform-branch anchors are captured at press as
                // well, even when `proportional` starts false — that
                // way a mid-drag Shift press flips the formula
                // through the same `re_anchor_scale_axis` path
                // either direction without needing a "first-toggle"
                // special case.
                let Some(pivot_screen) =
                    picking::project_world_to_screen(camera, fov_y, w, h, pivot)
                else {
                    return;
                };
                DragOp::ScaleAxis {
                    axis,
                    start_anchor,
                    proportional: ctx.modifiers.shift,
                    press_screen: (x, y),
                    pivot_screen,
                }
            }
        };
        drop(scene_slot);
        drop(camera_slot);
        self.state.set(GizmoState::Drag {
            op,
            original,
            pivot,
            object_idx: sel_idx,
        });
    }

    /// Pointer-move drag-update step. Branches on the active `DragOp`,
    /// recomputes the per-op delta from the current cursor ray, and
    /// composes the resulting transform onto the captured original.
    /// If a `ScaleAxis` drag is live and the live Shift state has
    /// flipped since the last frame, re-anchors the drag (current
    /// transform → new `original`, current cursor → new anchors) so
    /// the per-axis ↔ uniform formula switch is visually snap-free.
    ///
    /// Returns `true` if the scene's transform was updated. Callers
    /// mark the controller dirty on `true`.
    pub(super) fn on_move(&self, ctx: MoveCtx<'_>) -> bool {
        let GizmoState::Drag {
            mut op,
            mut original,
            pivot,
            object_idx,
        } = self.state.get()
        else {
            return false;
        };
        let Some(vp) = ctx.viewport else {
            return false;
        };

        let x = ctx.cursor.x;
        let y = ctx.cursor.y;

        // Compute the new transform and the (possibly mutated) op
        // state inside a borrow scope, then drop the camera borrow
        // before mutating the scene.
        let result: Option<(Transform3D, DragOp)> = {
            let camera_slot = ctx.camera.borrow();
            let Some(camera) = camera_slot.as_ref() else {
                return false;
            };
            let scene_slot = ctx.scene.borrow();
            let scene = scene_slot.as_ref();
            // Live Shift toggle on a `ScaleAxis` drag re-anchors before
            // the move's delta is computed — the running transform
            // becomes the new `original`, the current cursor becomes
            // the new press, so the per-axis ↔ uniform formula switch
            // does not snap the object.
            if let DragOp::ScaleAxis { proportional, .. } = op
                && proportional != ctx.modifiers.shift
                && let Some(scene_ref) = scene
                && let Some(obj) = scene_ref.objects.get(object_idx)
            {
                let reanchor_ctx = ReanchorCtx {
                    camera,
                    pivot,
                    fov_y: default_fov_y(),
                    viewport: (vp.width(), vp.height()),
                    cursor: (x, y),
                };
                if let Some(re_anchored) =
                    re_anchor_scale_axis(op, obj.transform, &reanchor_ctx, ctx.modifiers.shift)
                {
                    op = re_anchored.0;
                    original = re_anchored.1;
                }
            }
            let ray =
                picking::ray_from_pixel(camera, default_fov_y(), vp.width(), vp.height(), x, y);
            apply_drag_move(op, &ray, pivot, &original, x, y)
        };

        let Some((new_transform, new_op)) = result else {
            return false;
        };

        // Persist the (possibly updated) drag state — Rotate's
        // unwrap counter mutates per move; the others' state is
        // captured at press and stays put. `original` may also have
        // been reset by a Shift re-anchor on this frame.
        self.state.set(GizmoState::Drag {
            op: new_op,
            original,
            pivot,
            object_idx,
        });

        let mut scene_slot = ctx.scene.borrow_mut();
        let Some(scene) = scene_slot.as_mut() else {
            return false;
        };
        if let Some(obj) = scene.objects.get_mut(object_idx) {
            obj.transform = new_transform;
        }
        true
    }

    /// Aborts any in-progress drag and reverts the affected object's
    /// transform to its captured original. Returns `true` if a drag
    /// was in flight (caller marks dirty + clears input phase).
    pub(super) fn abort(&self, scene: &RefCell<Option<Scene>>) -> bool {
        let GizmoState::Drag {
            original,
            object_idx,
            ..
        } = self.state.get()
        else {
            return false;
        };
        self.state.set(GizmoState::Idle);
        let mut scene_slot = scene.borrow_mut();
        if let Some(scene) = scene_slot.as_mut()
            && let Some(obj) = scene.objects.get_mut(object_idx)
        {
            obj.transform = original;
        }
        true
    }
}
