/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure drag-math FSM for the transform gizmo. The data types
//! (`GizmoState`, `DragOp`, `HandleHit`) and the per-frame compose
//! functions (`apply_drag_move`, `re_anchor_scale_axis`) are
//! controller-agnostic — they take the current cursor / camera /
//! pivot inputs and return the new transform plus the (possibly
//! mutated) op state. The `ViewportController` owns the `Cell`s and
//! drives the Slint callbacks; everything in this module is a free
//! function.

use glam::Vec3;
use sc_mesh_core::scene::Transform3D;
use sc_mesh_core::transform::AffineTransform3D;

use crate::camera::OrbitCamera;
use crate::gizmo::{self, Axis};
use crate::picking;

/// Gizmo input FSM. `Idle` is the resting state (no drag in
/// progress). `Drag` captures the operation (translate / rotate /
/// scale-axis / scale-uniform), the object's transform at gesture
/// start, the world-space pivot, and the object index. The running
/// transform is composed each frame *from the captured original*,
/// not from the running transform — composing-from-original keeps
/// long drags numerically clean (no float drift across hundreds of
/// move events).
#[derive(Copy, Clone, Debug)]
pub(super) enum GizmoState {
    Idle,
    Drag {
        op: DragOp,
        original: Transform3D,
        pivot: Vec3,
        object_idx: usize,
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
/// - `ScaleUniform` composes a uniform scale about the pivot. Started
///   only by pressing the centre cube — the per-axis-cube-with-Shift
///   path goes through `ScaleAxis { proportional: true }` instead, so
///   releasing Shift can revert it without losing the originating
///   axis. `press_screen` is the cursor position at press;
///   `pivot_screen` is the projected pivot in pixel coords (also
///   captured at press, so subsequent moves use a stable reference).
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
        proportional: bool,
        press_screen: (f32, f32),
        pivot_screen: (f32, f32),
    },
    ScaleUniform {
        press_screen: (f32, f32),
        pivot_screen: (f32, f32),
    },
}

/// Categorised winner of the press-time gizmo hit-test. Used by
/// the controller to map across-category arbitration into the
/// matching `DragOp` variant.
pub(super) enum HandleHit {
    Translate(Axis),
    Rotate(Axis),
    ScaleAxis(Axis),
    ScaleCentre,
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
/// updated) `DragOp` state — Rotate's unwrap counter mutates per
/// frame; the others are pure functions of the press capture and the
/// current cursor.
///
/// Returns `None` if the cursor produces no usable delta this frame
/// (e.g. rotate ray slipped into the lock-out region between move
/// events) — the controller leaves the running transform unchanged
/// in that case rather than snapping the object.
pub(super) fn apply_drag_move(
    op: DragOp,
    ray: &picking::Ray,
    pivot: Vec3,
    original: &Transform3D,
    cursor_x: f32,
    cursor_y: f32,
) -> Option<(Transform3D, DragOp)> {
    match op {
        DragOp::Translate { axis, anchor } => {
            let new_anchor = gizmo::closest_point_on_axis(ray, pivot, axis);
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
            let raw = gizmo::rotate_drag_angle(ray, pivot, axis)?;
            let new_unwrapped = unwrap_angle(last_raw, current_unwrapped, raw);
            let delta_angle = new_unwrapped - start_angle;
            let new_op = DragOp::Rotate {
                axis,
                start_angle,
                last_raw: raw,
                current_unwrapped: new_unwrapped,
            };
            // R(axis, delta_angle) about the world-space pivot:
            //   T(pivot) ∘ R ∘ T(-pivot) ∘ original.
            let rotation =
                axis_rotation(axis, delta_angle).map(|r| compose_about_pivot(&r, pivot, original));
            rotation.map(|new_t| (new_t, new_op))
        }
        DragOp::ScaleAxis {
            axis,
            start_anchor,
            proportional,
            press_screen,
            pivot_screen,
        } => {
            let scale_mat = if proportional {
                let factor = gizmo::uniform_scale_factor(
                    pivot_screen.0,
                    pivot_screen.1,
                    press_screen.0,
                    press_screen.1,
                    cursor_x,
                    cursor_y,
                );
                AffineTransform3D::from_uniform_scale(factor as f64)
            } else {
                let current_anchor = gizmo::closest_point_on_axis(ray, pivot, axis);
                let factor =
                    gizmo::scale_factor_along_axis(start_anchor, current_anchor, pivot, axis);
                scale_along_axis(axis, factor)
            };
            Some((compose_about_pivot(&scale_mat, pivot, original), op))
        }
        DragOp::ScaleUniform {
            press_screen,
            pivot_screen,
        } => {
            let factor = gizmo::uniform_scale_factor(
                pivot_screen.0,
                pivot_screen.1,
                press_screen.0,
                press_screen.1,
                cursor_x,
                cursor_y,
            );
            let scale_mat = AffineTransform3D::from_uniform_scale(factor as f64);
            Some((compose_about_pivot(&scale_mat, pivot, original), op))
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
/// camera) — the caller leaves the drag unchanged in that case.
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
    let start_anchor = gizmo::closest_point_on_axis(&ray, ctx.pivot, axis);
    let new_op = DragOp::ScaleAxis {
        axis,
        start_anchor,
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
/// difference exceeds π, atan2 has wrapped through ±π — adjust by
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
/// right-handed. Returns `None` if the axis happens to be zero —
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
/// `pivot`, then applies it to `original`:
///   `T(pivot) ∘ m ∘ T(-pivot) ∘ original`.
pub(super) fn compose_about_pivot(
    m: &AffineTransform3D,
    pivot: Vec3,
    original: &Transform3D,
) -> Transform3D {
    let to_pivot =
        AffineTransform3D::from_translation([pivot.x as f64, pivot.y as f64, pivot.z as f64]);
    let from_pivot =
        AffineTransform3D::from_translation([-pivot.x as f64, -pivot.y as f64, -pivot.z as f64]);
    let about_pivot = to_pivot.compose(&m.compose(&from_pivot));
    Transform3D::Affine(about_pivot).compose(original)
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
        // readings: 0, π/2, π−ε, −π+ε, −π/2, −ε, 0, π/2, ... . The
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
        let (new_t, _) = apply_drag_move(op, &ray, pivot, &original, 0.0, 0.0).unwrap();
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
            proportional: false,
            // Unused in the per-axis branch — supply zeros.
            press_screen: (0.0, 0.0),
            pivot_screen: (0.0, 0.0),
        };
        let ray = ray_from(Vec3::new(2.0, -3.0, 0.0), Vec3::new(2.0, 0.0, 0.0));
        let (new_t, _) = apply_drag_move(op, &ray, pivot, &original, 0.0, 0.0).unwrap();
        let m = read_linear(&new_t);
        assert_approx_eq!(f64, m[0][0], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m[2][2], 1.0, epsilon = 1e-6);
        let translation = read_translation_t(&new_t);
        assert_approx_eq!(f64, translation[0], 0.0, epsilon = 1e-6);
    }

    #[test]
    fn scale_axis_op_with_proportional_uses_uniform_formula() {
        // Same `ScaleAxis` op — the only difference is the
        // `proportional` flag. With it set, the formula is the uniform
        // one: factor = current_dist / press_dist in screen pixels.
        // Press at 50 px from pivot, current at 100 px → factor 2.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = DragOp::ScaleAxis {
            axis: Axis::X,
            // `start_anchor` becomes irrelevant in the proportional
            // branch — supply something out-of-range to make the
            // contrast visible if a regression flips the dispatch
            // back to the per-axis formula.
            start_anchor: Vec3::new(99.0, 0.0, 0.0),
            proportional: true,
            press_screen: (150.0, 100.0),
            pivot_screen: (100.0, 100.0),
        };
        let ray = ray_from(Vec3::new(0.0, -3.0, 0.0), Vec3::ZERO);
        let (new_t, _) = apply_drag_move(op, &ray, pivot, &original, 200.0, 100.0).unwrap();
        let m = read_linear(&new_t);
        assert_approx_eq!(f64, m[0][0], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[1][1], 2.0, epsilon = 1e-4);
        assert_approx_eq!(f64, m[2][2], 2.0, epsilon = 1e-4);
    }

    #[test]
    fn scale_uniform_op_composes_a_uniform_diagonal_about_pivot() {
        // Press at distance 50 px from projected pivot, current at
        // 100 px → factor 2.
        let pivot = pivot_origin();
        let original = Transform3D::Identity;
        let op = DragOp::ScaleUniform {
            press_screen: (150.0, 100.0), // 50 px right of pivot
            pivot_screen: (100.0, 100.0),
        };
        // The ray itself doesn't enter the formula for ScaleUniform —
        // factor is purely from screen distances. Use any ray.
        let ray = ray_from(Vec3::new(0.0, -3.0, 0.0), Vec3::ZERO);
        let (new_t, _) = apply_drag_move(op, &ray, pivot, &original, 200.0, 100.0).unwrap();
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
        // Ray crosses the X arc plane at (0, 0, 1) — a point on +Z
        // → angle π/2 in the X-axis basis (u=Y, v=Z).
        let ray = ray_from(Vec3::new(5.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));
        let (new_t, new_op) = apply_drag_move(op, &ray, pivot, &original, 0.0, 0.0).unwrap();
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
}
