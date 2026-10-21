/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure drag-math FSM for the transform gizmo. The data types
//! (`GizmoState`, `DragOp`, `HandleHit`) and the per-frame compose
//! functions (`apply_drag_move`, `re_anchor_scale_axis`) are
//! controller-agnostic: they take the current cursor / camera /
//! pivot inputs and return the new transform plus the (possibly
//! mutated) op state. The `ViewportController` owns the `Cell`s and
//! drives the Slint callbacks; everything in this module is a free
//! function.

use glam::Vec3;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;

use crate::camera::OrbitCamera;
use crate::gizmo::axis::Axis;
use crate::gizmo::{self};
use crate::picking;

/// Gizmo input FSM. `Idle` is the resting state (no drag in
/// progress). `Drag` captures the operation (translate / rotate /
/// scale-axis / scale-uniform), the object's transform at gesture
/// start, the world-space pivot, and the dragged leaf's stable
/// [`NodeId`]. The running transform is composed each frame *from the
/// captured original*, not from the running transform: composing-
/// from-original keeps long drags numerically clean (no float drift
/// across hundreds of move events).
#[derive(Copy, Clone, Debug)]
pub(super) enum GizmoState {
    Idle,
    Drag {
        op: DragOp,
        original: Transform3D,
        pivot: Vec3,
        node_id: NodeId,
    },
}

/// Per-operation drag state carried inside `GizmoState::Drag`.
///
/// - `Translate` composes a translation; `anchor` is the world-space
///   closest point on the axis at press, recomputed each move.
/// - `Rotate` composes a rotation about the axis through the pivot.
///   `start_angle` is the angular position at press; `last_raw` and
///   `current_unwrapped` track the unwrap counter so a full-revolution
///   drag yields a real 360°+ rotation instead of resetting at ±π.
/// - `ScaleAxis` composes either a per-axis scale or a uniform scale
///   about the pivot, selected by `proportional` (driven by the live
///   Shift state). `start_anchor` is the world-space closest point on
///   the axis (used by the per-axis branch); `press_screen` /
///   `pivot_screen` carry the equivalent screen anchors used by the
///   uniform branch. Both anchor sets are captured at press *and*
///   re-captured on every Shift toggle so the formula switch is
///   visually snap-free (see `re_anchor_scale_axis`).
#[derive(Copy, Clone, Debug)]
pub(super) enum DragOp {
    Translate {
        axis: Axis,
        anchor: Vec3,
    },
    Rotate {
        axis: Axis,
        start_angle: f32,
        last_raw: f32,
        current_unwrapped: f32,
    },
    ScaleAxis {
        axis: Axis,
        start_anchor: Vec3,
        /// Latest closest-point-on-axis under the cursor. Captured at
        /// press (= `start_anchor`) and refreshed each move so the
        /// renderer can slide the per-axis cube along the axis to
        /// follow the cursor as visual feedback for the live scale.
        current_anchor: Vec3,
        proportional: bool,
        press_screen: (f32, f32),
        pivot_screen: (f32, f32),
    },
}

/// Categorised winner of the press-time (or hover-time) gizmo
/// hit-test. Used by the controller to map across-category arbitration
/// into the matching `DragOp` variant on press, and into the renderer's
/// hover-tint mask on hover.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum HandleHit {
    Translate(Axis),
    Rotate(Axis),
    ScaleAxis(Axis),
}

/// Bundled inputs for `re_anchor_scale_axis`. The single call site
/// already groups all of these together (camera, viewport, cursor,
/// pivot, fov_y); packing them keeps the function signature short
/// enough to forgo `#[allow(clippy::too_many_arguments)]`.
pub(super) struct ReanchorCtx<'a> {
    pub camera: &'a OrbitCamera,
    pub pivot: Vec3,
    pub fov_y: f32,
    pub viewport: (f32, f32),
    pub cursor: (f32, f32),
}

/// Per-op delta computation for a move event. Returns the new
/// transform composed onto `original`, paired with the (possibly
/// updated) `DragOp` state: Rotate's unwrap counter mutates per
/// frame; the others are pure functions of the press capture and the
/// current cursor.
///
/// Returns `None` if the cursor produces no usable delta this frame
/// (e.g. rotate ray slipped into the lock-out region between move
/// events): the controller leaves the running transform unchanged
/// in that case rather than snapping the object.
pub(super) fn apply_drag_move(
    op: DragOp,
    ray: &picking::Ray,
    pivot: Vec3,
    parent_world: &Transform3D,
    original: &Transform3D,
    cursor_x: f32,
    cursor_y: f32,
) -> Option<(Transform3D, DragOp)> {
    match op {
        DragOp::Translate { axis, anchor } => {
            let new_anchor = gizmo::drag_math::closest_point_on_axis(ray, pivot, axis)?;
            let delta = new_anchor - anchor;
            let translation = AffineTransform3D::from_translation([
                delta.x as f64,
                delta.y as f64,
                delta.z as f64,
            ]);
            Some((Transform3D::Affine(translation).compose(original), op))
        }
        DragOp::Rotate {
            axis,
            start_angle,
            last_raw,
            current_unwrapped,
        } => {
            let raw = gizmo::drag_math::rotate_drag_angle(ray, pivot, axis)?;
            let new_unwrapped = unwrap_angle(last_raw, current_unwrapped, raw);
            let delta_angle = new_unwrapped - start_angle;
            let new_op = DragOp::Rotate {
                axis,
                start_angle,
                last_raw: raw,
                current_unwrapped: new_unwrapped,
            };
            // R(axis, delta_angle) about the world-space pivot,
            // expressed in the leaf's parent space by
            // `compose_about_pivot`.
            let rotation = axis_rotation(axis, delta_angle)
                .map(|r| compose_about_pivot(&r, pivot, parent_world, original));
            rotation.map(|new_t| (new_t, new_op))
        }
        DragOp::ScaleAxis {
            axis,
            start_anchor,
            current_anchor: _,
            proportional,
            press_screen,
            pivot_screen,
        } => {
            // Recompute the cursor's closest-point-on-axis on every
            // move so the renderer can slide the cube along the axis,
            // even when `proportional` is on (the uniform branch
            // doesn't use the anchor for its own math, but we still
            // refresh it so a Shift-toggle back to per-axis re-uses a
            // current anchor instead of the stale press one). `None`
            // here means the camera snapped into the axis lock-out
            // since press; freeze the drag for this frame.
            let current_anchor = gizmo::drag_math::closest_point_on_axis(ray, pivot, axis)?;
            let scale_mat = if proportional {
                let factor = gizmo::drag_math::uniform_scale_factor(
                    pivot_screen.0,
                    pivot_screen.1,
                    press_screen.0,
                    press_screen.1,
                    cursor_x,
                    cursor_y,
                );
                AffineTransform3D::from_uniform_scale(factor as f64)
            } else {
                let factor = gizmo::drag_math::scale_factor_along_axis(
                    start_anchor,
                    current_anchor,
                    pivot,
                    axis,
                );
                scale_along_axis(axis, factor)
            };
            let new_op = DragOp::ScaleAxis {
                axis,
                start_anchor,
                current_anchor,
                proportional,
                press_screen,
                pivot_screen,
            };
            Some((
                compose_about_pivot(&scale_mat, pivot, parent_world, original),
                new_op,
            ))
        }
    }
}

/// Re-anchors a `ScaleAxis` drag for a Shift toggle: the scene's
/// running transform becomes the new `original`, the cursor's current
/// position becomes the new press, both anchor sets (axis + screen)
/// are recomputed from the cursor, and `proportional` is set to the
/// new Shift state. After this call `apply_drag_move` produces the
/// running transform unchanged for the toggling frame, so the formula
/// switch is visually snap-free.
///
/// Returns `None` if the pivot's screen projection fails (degenerate
/// camera) or if the cursor ray slipped into the axis lock-out since
/// press; the caller leaves the drag unchanged in either case.
/// Defensive against op variants other than `ScaleAxis`: returns
/// `None` so the move dispatch falls through to the regular path.
pub(super) fn re_anchor_scale_axis(
    op: DragOp,
    running_transform: Transform3D,
    ctx: &ReanchorCtx<'_>,
    new_proportional: bool,
) -> Option<(DragOp, Transform3D)> {
    let DragOp::ScaleAxis { axis, .. } = op else {
        return None;
    };
    let (viewport_w, viewport_h) = ctx.viewport;
    let (cursor_x, cursor_y) = ctx.cursor;
    let pivot_screen =
        picking::project_world_to_screen(ctx.camera, ctx.fov_y, viewport_w, viewport_h, ctx.pivot)?;
    let ray = picking::ray_from_pixel(
        ctx.camera, ctx.fov_y, viewport_w, viewport_h, cursor_x, cursor_y,
    );
    let start_anchor = gizmo::drag_math::closest_point_on_axis(&ray, ctx.pivot, axis)?;
    let new_op = DragOp::ScaleAxis {
        axis,
        start_anchor,
        current_anchor: start_anchor,
        proportional: new_proportional,
        press_screen: (cursor_x, cursor_y),
        pivot_screen,
    };
    Some((new_op, running_transform))
}

/// Updates the unwrap counter so a multi-revolution drag yields a
/// continuous angle. `last_raw` and `current_unwrapped` are the
/// previous frame's raw atan2 reading and the cumulative unwrapped
/// angle; `raw` is this frame's raw reading. When the per-frame raw
/// difference exceeds π, atan2 has wrapped through ±π; adjust by
/// 2π in the opposite direction so the unwrapped angle stays
/// continuous.
pub(super) fn unwrap_angle(last_raw: f32, current_unwrapped: f32, raw: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut diff = raw - last_raw;
    if diff > PI {
        diff -= TAU;
    } else if diff < -PI {
        diff += TAU;
    }
    current_unwrapped + diff
}

/// Affine rotation about a world-aligned axis by `angle` radians,
/// right-handed. Returns `None` if the axis happens to be zero:
/// can't happen for `Axis::{X,Y,Z}` but the propagation keeps the
/// caller's `?` chain typed.
pub(super) fn axis_rotation(axis: Axis, angle: f32) -> Option<AffineTransform3D> {
    let angle_f64 = angle as f64;
    Some(match axis {
        Axis::X => AffineTransform3D::from_rotation_x(angle_f64),
        Axis::Y => AffineTransform3D::from_rotation_y(angle_f64),
        Axis::Z => AffineTransform3D::from_rotation_z(angle_f64),
    })
}

/// Per-axis scale, with the off-axis components held at 1.0 (no
/// non-uniformity bleed). Used by `ScaleAxis`.
pub(super) fn scale_along_axis(axis: Axis, factor: f32) -> AffineTransform3D {
    let f = factor as f64;
    let s = match axis {
        Axis::X => [f, 1.0, 1.0],
        Axis::Y => [1.0, f, 1.0],
        Axis::Z => [1.0, 1.0, f],
    };
    AffineTransform3D::from_non_uniform_scale(s)
}

/// Composes a rotation/scale operation `m` about a world-space
/// `pivot`, then applies it to `original` (the leaf's
/// parent-relative transform).
///
/// `parent_world` is the leaf's ancestor chain composed up to but
/// not including the leaf itself. When it is `Identity`, the
/// world-space "rotate/scale about `pivot`" reduces to the textbook
/// `T(pivot) ∘ m ∘ T(-pivot) ∘ original`. When the parent is
/// non-trivial (e.g. the placement translation `place_on_plate`
/// writes onto the scene root), the world pivot must be expressed
/// in the leaf's parent space first: otherwise the composition
/// applies the rotation/scale about a point offset by the parent's
/// own translation, displacing the object instead of holding it
/// fixed.
///
/// For a pure-translation parent (the only kind today's scene
/// shapes produce), the conversion is `pivot_local = pivot - δ_p`,
/// where `δ_p` is the parent's translation. The general form goes
/// through `parent_world.inverse()` applied as a point transform.
pub(super) fn compose_about_pivot(
    m: &AffineTransform3D,
    pivot: Vec3,
    parent_world: &Transform3D,
    original: &Transform3D,
) -> Transform3D {
    let pivot_local = world_pivot_to_local(pivot, parent_world);
    let to_pivot = AffineTransform3D::from_translation([
        pivot_local.x as f64,
        pivot_local.y as f64,
        pivot_local.z as f64,
    ]);
    let from_pivot = AffineTransform3D::from_translation([
        -pivot_local.x as f64,
        -pivot_local.y as f64,
        -pivot_local.z as f64,
    ]);
    let about_pivot = to_pivot.compose(&m.compose(&from_pivot));
    Transform3D::Affine(about_pivot).compose(original)
}

/// Express the world-space `pivot` in the leaf's parent space, i.e.
/// apply `parent_world.inverse()` as a point transform. Falls back
/// to the world `pivot` unchanged if the parent transform is
/// singular (cannot happen for valid scene graphs today: every
/// production parent is a pure translation, which is invertible).
fn world_pivot_to_local(pivot: Vec3, parent_world: &Transform3D) -> Vec3 {
    let Transform3D::Affine(p) = parent_world else {
        return pivot;
    };
    let Ok(inv) = p.inverse() else {
        return pivot;
    };
    let mapped = inv.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([
        pivot.x, pivot.y, pivot.z,
    ]));
    Vec3::from(mapped.coords())
}

#[cfg(test)]
mod unwrap_angle_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn small_step_does_not_wrap() {
        // No wrap: cumulative grows by the same amount as the raw
        // difference.
        let result = unwrap_angle(0.5, 0.5, 0.6);
        assert_approx_eq!(f32, result, 0.6, epsilon = 1e-6);
    }

    #[test]
    fn cw_wrap_through_negative_pi_adjusts_by_2pi() {
        // Last raw was just below π, this raw is just above -π. The
        // user dragged CCW past the discontinuity; the unwrapped
        // angle should keep growing past π.
        let pi = std::f32::consts::PI;
        let last_raw = pi - 0.1;
        let last_unwrapped = pi - 0.1;
        let raw_now = -pi + 0.1; // jumped by ~ -2π + 0.2 (raw)
        let result = unwrap_angle(last_raw, last_unwrapped, raw_now);
        assert_approx_eq!(f32, result, pi + 0.1, epsilon = 1e-5);
    }

    #[test]
    fn ccw_wrap_through_positive_pi_adjusts_by_minus_2pi() {
        // Symmetric case in the other direction: cursor jumped CCW
        // through +π → unwrapped angle should keep decreasing.
        let pi = std::f32::consts::PI;
        let last_raw = -pi + 0.1;
        let last_unwrapped = -pi + 0.1;
        let raw_now = pi - 0.1;
        let result = unwrap_angle(last_raw, last_unwrapped, raw_now);
        assert_approx_eq!(f32, result, -pi - 0.1, epsilon = 1e-5);
    }

    #[test]
    fn full_revolution_yields_continuous_angle() {
        // Simulate dragging CCW through a full 2π. Successive raw
        // readings: 0, π/2, π-ε, -π+ε, -π/2, -ε, 0, π/2, ... . The
        // unwrapped sequence should grow monotonically over a full
        // revolution.
        let pi = std::f32::consts::PI;
        let mut last_raw = 0.0_f32;
        let mut unwrapped = 0.0_f32;
        for raw in [pi * 0.5, pi - 0.01, -pi + 0.01, -pi * 0.5, -0.01, 0.01] {
            unwrapped = unwrap_angle(last_raw, unwrapped, raw);
            last_raw = raw;
        }
        // After a full revolution, unwrapped should be just past 2π.
        assert!(
            unwrapped > 2.0 * pi - 0.05 && unwrapped < 2.0 * pi + 0.05,
            "expected unwrapped ≈ 2π, got {unwrapped}"
        );
    }
}

#[cfg(test)]
mod apply_drag_move_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn ray_from(origin: Vec3, target: Vec3) -> picking::Ray {
        picking::Ray {
            origin,
            direction: (target - origin).normalize(),
        }
    }

    fn pivot_origin() -> Vec3 {
        Vec3::ZERO
    }

    /// Read the affine linear part as a row-major 3×3 of f64.
    fn read_linear(t: &Transform3D) -> [[f64; 3]; 3] {
        match t {
            Transform3D::Identity => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            Transform3D::Affine(a) => *a.linear(),
        }
    }

    fn read_translation_t(t: &Transform3D) -> [f64; 3] {
        match t {
            Transform3D::Identity => [0.0; 3],
            Transform3D::Affine(a) => *a.translation(),
        }
    }

    #[test]
    fn translate_op_composes_a_pure_translation() {
        // Press anchor on +X at (1, 0, 0); cursor moves the closest
        // point to (1.5, 0, 0). Object should end up translated by
        // +0.5 along X.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = DragOp::Translate {
            axis: Axis::X,
            anchor: Vec3::new(1.0, 0.0, 0.0),
        };
        // Construct a ray whose closest point on +X axis is (1.5, 0, 0).
        let ray = ray_from(Vec3::new(1.5, -3.0, 0.0), Vec3::new(1.5, 0.0, 0.0));
        let (new_t, _) =
            apply_drag_move(op, &ray, pivot, &Transform3D::Identity, &original, 0.0, 0.0).unwrap();
        let translation = read_translation_t(&new_t);
        assert_approx_eq!(f64, translation[0], 0.5, epsilon = 1e-4);
        assert_approx_eq!(f64, translation[1], 0.0, epsilon = 1e-4);
        assert_approx_eq!(f64, translation[2], 0.0, epsilon = 1e-4);
        // Linear part stays identity.
        let m = read_linear(&new_t);
        assert_approx_eq!(f64, m[0][0], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m[1][1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m[2][2], 1.0, epsilon = 1e-6);
    }

    #[test]
    fn scale_axis_op_composes_a_diagonal_scale_about_pivot() {
        // Press anchor at (1, 0, 0); current anchor at (2, 0, 0)
        // → factor 2 along X. Pivot at origin → translation stays 0.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = DragOp::ScaleAxis {
            axis: Axis::X,
            start_anchor: Vec3::new(1.0, 0.0, 0.0),
            current_anchor: Vec3::new(1.0, 0.0, 0.0),
            proportional: false,
            // Unused in the per-axis branch; supply zeros.
            press_screen: (0.0, 0.0),
            pivot_screen: (0.0, 0.0),
        };
        let ray = ray_from(Vec3::new(2.0, -3.0, 0.0), Vec3::new(2.0, 0.0, 0.0));
        let (new_t, _) =
            apply_drag_move(op, &ray, pivot, &Transform3D::Identity, &original, 0.0, 0.0).unwrap();
        let m = read_linear(&new_t);
        assert_approx_eq!(f64, m[0][0], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m[2][2], 1.0, epsilon = 1e-6);
        let translation = read_translation_t(&new_t);
        assert_approx_eq!(f64, translation[0], 0.0, epsilon = 1e-6);
    }

    #[test]
    fn scale_axis_op_with_proportional_uses_uniform_formula() {
        // Same `ScaleAxis` op; the only difference is the
        // `proportional` flag. With it set, the formula is the uniform
        // one: factor = current_dist / press_dist in screen pixels.
        // Press at 50 px from pivot, current at 100 px → factor 2.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = DragOp::ScaleAxis {
            axis: Axis::X,
            // `start_anchor` becomes irrelevant in the proportional
            // branch; supply something out-of-range to make the
            // contrast visible if a regression flips the dispatch
            // back to the per-axis formula.
            start_anchor: Vec3::new(99.0, 0.0, 0.0),
            current_anchor: Vec3::new(99.0, 0.0, 0.0),
            proportional: true,
            press_screen: (150.0, 100.0),
            pivot_screen: (100.0, 100.0),
        };
        let ray = ray_from(Vec3::new(0.0, -3.0, 0.0), Vec3::ZERO);
        let (new_t, _) = apply_drag_move(
            op,
            &ray,
            pivot,
            &Transform3D::Identity,
            &original,
            200.0,
            100.0,
        )
        .unwrap();
        let m = read_linear(&new_t);
        assert_approx_eq!(f64, m[0][0], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[2][2], 2.0, epsilon = 1e-4);
    }

    #[test]
    fn rotate_op_composes_a_rotation_about_pivot() {
        // Press angle 0 (cursor on +Y for the X-axis arc), current
        // angle +π/2 (cursor on +Z). The rotation is +π/2 about +X,
        // which takes +Y → +Z. Verify the linear part.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let start = 0.0_f32;
        let op = DragOp::Rotate {
            axis: Axis::X,
            start_angle: start,
            last_raw: start,
            current_unwrapped: start,
        };
        // Ray crosses the X arc plane at (0, 0, 1): a point on +Z
        // → angle π/2 in the X-axis basis (u=Y, v=Z).
        let ray = ray_from(Vec3::new(5.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));
        let (new_t, new_op) =
            apply_drag_move(op, &ray, pivot, &Transform3D::Identity, &original, 0.0, 0.0).unwrap();
        // The linear part rotates +Y onto +Z (right-handed about +X).
        let m = read_linear(&new_t);
        // R(X, π/2) maps (0, 1, 0) → (0, 0, 1):
        //   m[0][1] = 0, m[1][1] = 0, m[2][1] = 1.
        assert_approx_eq!(f64, m[0][1], 0.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 0.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[2][1], 1.0, epsilon = 1e-4);
        // The new op state should have advanced last_raw + unwrapped.
        if let DragOp::Rotate {
            current_unwrapped, ..
        } = new_op
        {
            assert_approx_eq!(
                f32,
                current_unwrapped,
                std::f32::consts::FRAC_PI_2,
                epsilon = 1e-4
            );
        } else {
            panic!("expected Rotate, got {new_op:?}");
        }
    }

    #[test]
    fn scale_axis_proportional_under_translated_parent_keeps_world_centre_fixed() {
        // Regression for the displacement-on-shift bug: the gizmo
        // pivot lives in world space; the leaf's transform is the
        // PARENT-relative local. With a root translation
        // `parent_world = T(40, 0, 0)` and an identity leaf carrying
        // a mesh centred on the LOCAL origin, the world centre of
        // the object is at `(40, 0, 0)`, which is also the press-
        // time gizmo pivot. A uniform scale by factor 2 about that
        // world pivot must produce a new leaf local whose world
        // image still has its centre at `(40, 0, 0)`. The check is
        // direct: the new leaf local, composed with parent_world,
        // maps the origin (the mesh centre) to `(40, 0, 0)`.
        //
        // Before the fix, `compose_about_pivot` would have applied
        // `T(40) ∘ scale ∘ T(-40)` on top of the identity leaf
        // local, treating the world pivot as a local pivot. The
        // resulting WORLD image of the origin came out at
        // `(80, 0, 0)`: the object shifted by `δ_root * (1 - factor)
        //  = 40 * (1 - 2) = -40`, i.e. ended up at world +80 if
        // composed past the parent's +40.
        let parent_world =
            Transform3D::Affine(AffineTransform3D::from_translation([40.0, 0.0, 0.0]));
        let original = Transform3D::Identity;
        let world_pivot = Vec3::new(40.0, 0.0, 0.0);
        let op = DragOp::ScaleAxis {
            axis: Axis::X,
            start_anchor: Vec3::new(41.0, 0.0, 0.0),
            current_anchor: Vec3::new(41.0, 0.0, 0.0),
            proportional: true,
            press_screen: (150.0, 100.0),
            pivot_screen: (100.0, 100.0),
        };
        // Pixel anchors: pivot at (100, 100), press at (150, 100)
        // (50 px out), cursor at (200, 100) (100 px out) → uniform
        // factor 2.
        let ray = ray_from(Vec3::new(40.0, -3.0, 0.0), Vec3::new(40.0, 0.0, 0.0));
        let (new_local, _) = apply_drag_move(
            op,
            &ray,
            world_pivot,
            &parent_world,
            &original,
            200.0,
            100.0,
        )
        .unwrap();

        // Image of the mesh's local origin under the new world
        // transform: `world_centre = parent_world ∘ new_local ∘ 0`.
        let world_centre = match parent_world.compose(&new_local) {
            Transform3D::Identity => Vec3::ZERO,
            Transform3D::Affine(a) => {
                let v = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([0.0, 0.0, 0.0]));
                Vec3::from(v.coords())
            }
        };
        assert_approx_eq!(f32, world_centre.x, 40.0, epsilon = 1e-4);
        assert_approx_eq!(f32, world_centre.y, 0.0, epsilon = 1e-4);
        assert_approx_eq!(f32, world_centre.z, 0.0, epsilon = 1e-4);
        // The linear part should still be a uniform 2x scale; the
        // entire effect of the parent's translation should be
        // confined to the translation channel of `new_local`.
        let m = read_linear(&new_local);
        assert_approx_eq!(f64, m[0][0], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[2][2], 2.0, epsilon = 1e-4);
    }

    #[test]
    fn rotate_under_translated_parent_keeps_world_centre_fixed() {
        // Sibling of the scale regression: the rotate composition
        // also goes through `compose_about_pivot`, so the same
        // parent-translation displacement would manifest as the
        // object orbiting around `(world_pivot + δ_root)` instead
        // of spinning in place. Verify that the rotated leaf's
        // world image of the mesh-local origin still sits at the
        // world pivot.
        let parent_world =
            Transform3D::Affine(AffineTransform3D::from_translation([40.0, 0.0, 0.0]));
        let original = Transform3D::Identity;
        let world_pivot = Vec3::new(40.0, 0.0, 0.0);
        let op = DragOp::Rotate {
            axis: Axis::Z,
            start_angle: 0.0,
            last_raw: 0.0,
            current_unwrapped: 0.0,
        };
        // Ray hits the Z-axis plane at the pivot (degenerate angle
        // 0); use a ray crossing y=+1 in the pivot's XY plane so
        // `rotate_drag_angle` returns π/2 (90° CCW).
        let ray = ray_from(Vec3::new(40.0, 1.0, 5.0), Vec3::new(40.0, 1.0, 0.0));
        let (new_local, _) =
            apply_drag_move(op, &ray, world_pivot, &parent_world, &original, 0.0, 0.0).unwrap();
        let world_centre = match parent_world.compose(&new_local) {
            Transform3D::Identity => Vec3::ZERO,
            Transform3D::Affine(a) => {
                let v = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([0.0, 0.0, 0.0]));
                Vec3::from(v.coords())
            }
        };
        assert_approx_eq!(f32, world_centre.x, 40.0, epsilon = 1e-4);
        assert_approx_eq!(f32, world_centre.y, 0.0, epsilon = 1e-4);
        assert_approx_eq!(f32, world_centre.z, 0.0, epsilon = 1e-4);
    }
}
