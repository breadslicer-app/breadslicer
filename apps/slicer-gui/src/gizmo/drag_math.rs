/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Drag-time gizmo math: closest-point on axis, plane intersection,
//! plane angle, scale-factor formulas. Renderer-agnostic so it can
//! be unit-tested without a `wgpu::Device`.

use glam::Vec3;

use crate::picking::Ray;

use super::axis::{Axis, axis_basis};
use super::tolerances::{EPS_SCALE, LOCKOUT_COS};

/// Minimum value of `1 - (axis · ray_dir)²` (= `sin²(angle)`) for the
/// closest-point formula to be well-conditioned: matches the 8°
/// margin already enforced by `axis_pickable`/`LOCKOUT_COS` on the
/// hit-test side, so any ray accepted by hit-test sits above this
/// floor and the helper never has to bail mid-press.
const DENOM_FLOOR: f32 = 1.0 - LOCKOUT_COS * LOCKOUT_COS;

/// Closest point on the infinite world-space line `pivot + t * axis`
/// to the world-space ray. Used during a drag to convert the cursor
/// position into the pinned-to-axis world point.
///
/// Returns `None` when the ray is within `8°` of axis-parallel: the
/// `1 - (a·D)²` denominator collapses there and `t` becomes
/// arbitrarily large, which would teleport the dragged object across
/// the scene. The press path is gated by `axis_pickable` (same
/// threshold) so `None` is only reachable when the camera snaps into
/// the lock-out region mid-drag; the controller treats `None` as
/// "no usable delta this frame" and leaves the running transform
/// unchanged until the camera leaves the unstable cone.
pub fn closest_point_on_axis(ray: &Ray, pivot: Vec3, axis: Axis) -> Option<Vec3> {
    let a = axis.world_dir();
    let d = ray.direction;
    let oc = pivot - ray.origin;

    let d_dot_a = d.dot(a);
    let oc_dot_a = oc.dot(a);
    let oc_dot_d = oc.dot(d);

    let denom = 1.0 - d_dot_a * d_dot_a;
    if denom < DENOM_FLOOR {
        return None;
    }

    // Derivation: with `q = pivot - O`, the perpendicularity
    // conditions `(δ·D) = 0` and `(δ·a) = 0` solve to
    //   `t · (1 - (a·D)²) = q·D · (a·D) - q·a`.
    let t_axis = (oc_dot_d * d_dot_a - oc_dot_a) / denom;
    Some(pivot + a * t_axis)
}

/// Intersects `ray` with the plane through `pivot` perpendicular to
/// `axis`. Returns the world-space hit, or `None` if the ray is
/// parallel to the plane or hits behind the camera. The rotate
/// lock-out (`arc_pickable`) is what keeps `denom` away from zero
/// in the steady state; the small floor here is a defensive guard
/// for callers that bypass the lock-out (e.g. tests).
fn ray_hits_axis_plane(ray: &Ray, pivot: Vec3, axis: Axis) -> Option<Vec3> {
    let a = axis.world_dir();
    let denom = ray.direction.dot(a);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (pivot - ray.origin).dot(a) / denom;
    if t <= 0.0 {
        return None;
    }
    Some(ray.origin + ray.direction * t)
}

/// Angular position (radians, in `[-π, +π]`) of `point` around
/// `axis` using the right-handed basis from `axis_basis`. A point on
/// `pivot + u` returns 0; on `pivot + v` returns `π/2`; on
/// `pivot - u` returns ±π.
fn axis_plane_angle(point: Vec3, pivot: Vec3, axis: Axis) -> f32 {
    let (u, v) = axis_basis(axis);
    let p = point - pivot;
    p.dot(v).atan2(p.dot(u))
}

/// Cursor-ray → angular position around `axis` for the rotate drag.
/// `Drag::Rotate` captures one of these at press time as
/// `start_angle` and recomputes it every move event; the controller
/// composes `delta_angle = current - start` (with an unwrap counter
/// to follow the cursor across the ±π discontinuity) into a rotation
/// `R(axis, delta_angle)` about the world-space pivot.
///
/// Returns `None` only for the degenerate cases that
/// [`super::hit_test::arc_pickable`] is supposed to forbid; the controller is
/// expected to gate this call on that lock-out at gesture start.
pub fn rotate_drag_angle(ray: &Ray, pivot: Vec3, axis: Axis) -> Option<f32> {
    let hit = ray_hits_axis_plane(ray, pivot, axis)?;
    Some(axis_plane_angle(hit, pivot, axis))
}

/// Per-axis scale factor for a drag whose press anchor was
/// `start_anchor` and whose current anchor is `current_anchor`.
/// Both anchors are world-space points on the axis line through
/// `pivot` (typically produced by `closest_point_on_axis`). The raw
/// ratio `current_proj / start_proj` is clamped into
/// `[EPS_SCALE, 1/EPS_SCALE]`, which also rejects negative ratios;
/// mirroring through the gizmo is disallowed.
///
/// Returns `1.0` (a no-op factor) when the press anchor coincides
/// with the pivot along the axis: the ratio is undefined there
/// and the drag should not move the object until the cursor leaves
/// the pivot's neighbourhood.
pub fn scale_factor_along_axis(
    start_anchor: Vec3,
    current_anchor: Vec3,
    pivot: Vec3,
    axis: Axis,
) -> f32 {
    let a = axis.world_dir();
    let start_proj = (start_anchor - pivot).dot(a);
    let current_proj = (current_anchor - pivot).dot(a);
    if start_proj.abs() < 1e-6 {
        return 1.0;
    }
    (current_proj / start_proj).clamp(EPS_SCALE, 1.0 / EPS_SCALE)
}

/// Uniform scale factor for a drag of the per-axis cube with the
/// proportional-scale modifier (Shift) held. Driven by the
/// screen-space distance between the cursor and the projected pivot:
/// the factor is the ratio of the current distance to the press
/// distance, clamped into `[EPS_SCALE, 1/EPS_SCALE]`.
///
/// `pivot_screen_*` must be the projection of the world-space
/// pivot into the same logical-pixel coordinate space as the
/// cursor positions. The controller computes that projection from
/// its camera + viewport once per move event.
///
/// Returns `1.0` when the press lands within `1e-6` pixels of the
/// projected pivot; the ratio is undefined there.
pub fn uniform_scale_factor(
    pivot_screen_x: f32,
    pivot_screen_y: f32,
    x_press: f32,
    y_press: f32,
    x_now: f32,
    y_now: f32,
) -> f32 {
    let dx0 = x_press - pivot_screen_x;
    let dy0 = y_press - pivot_screen_y;
    let start_dist_sq = dx0 * dx0 + dy0 * dy0;
    if start_dist_sq < 1e-12 {
        return 1.0;
    }
    let dx1 = x_now - pivot_screen_x;
    let dy1 = y_now - pivot_screen_y;
    let current_dist_sq = dx1 * dx1 + dy1 * dy1;
    let raw = (current_dist_sq / start_dist_sq).sqrt();
    raw.clamp(EPS_SCALE, 1.0 / EPS_SCALE)
}

#[cfg(test)]
mod closest_point_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ray_intersecting_axis_returns_intersection() {
        // Ray crosses X-axis at x = 2.
        let ray = Ray {
            origin: Vec3::new(2.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let p = closest_point_on_axis(&ray, Vec3::ZERO, Axis::X).unwrap();
        assert_approx_eq!(f32, p.x, 2.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn ray_perpendicular_returns_foot_of_perpendicular() {
        // Ray along +Z passing through (3, 4, 0). X-axis is the line
        // y=z=0; foot of perpendicular from (3, 4, *) onto X-axis is
        // (3, 0, 0).
        let ray = Ray {
            origin: Vec3::new(3.0, 4.0, -5.0),
            direction: Vec3::Z,
        };
        let p = closest_point_on_axis(&ray, Vec3::ZERO, Axis::X).unwrap();
        assert_approx_eq!(f32, p.x, 3.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn cursor_delta_along_screen_yields_axis_delta() {
        // The whole point of the formula: dragging the cursor along
        // the X handle's screen projection should produce a clean
        // world-space X translation, not contaminated by Y/Z.
        let pivot = Vec3::ZERO;
        let r0 = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let r1 = Ray {
            origin: Vec3::new(2.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let p0 = closest_point_on_axis(&r0, pivot, Axis::X).unwrap();
        let p1 = closest_point_on_axis(&r1, pivot, Axis::X).unwrap();
        let delta = p1 - p0;
        assert_approx_eq!(f32, delta.x, 2.5, epsilon = 1e-5);
        assert_approx_eq!(f32, delta.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, delta.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn ray_parallel_to_axis_returns_none() {
        // Ray direction equals the axis direction: `1 - (a·D)² = 0`.
        // The closest-point formula is undefined (every point on the
        // axis is equally close); without the bail-out, the clamped
        // denominator amplifies any q·D residual into a multi-metre
        // translation. The helper must refuse.
        let ray = Ray {
            origin: Vec3::new(0.0, 1.0, 0.0),
            direction: Vec3::X,
        };
        assert!(closest_point_on_axis(&ray, Vec3::ZERO, Axis::X).is_none());
    }

    #[test]
    fn ray_antiparallel_to_axis_returns_none() {
        // Same degeneracy as the parallel case: `(a·D)² = 1` whether
        // the ray points along +axis or -axis.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.0, 1.0),
            direction: -Vec3::X,
        };
        assert!(closest_point_on_axis(&ray, Vec3::ZERO, Axis::X).is_none());
    }

    #[test]
    fn ray_inside_lockout_cone_returns_none() {
        // Ray angled 5° off +X (well inside the 8° lock-out). The
        // unguarded formula would still divide by the tiny denominator
        // and produce a hit far from the press point; the bail-out is
        // what keeps a translate drag from teleporting after a
        // mid-drag camera snap into the lock-out cone.
        let theta = 5.0_f32.to_radians();
        let dir = Vec3::new(theta.cos(), theta.sin(), 0.0).normalize();
        let ray = Ray {
            origin: Vec3::new(-5.0, 0.1, 0.0),
            direction: dir,
        };
        assert!(closest_point_on_axis(&ray, Vec3::ZERO, Axis::X).is_none());
    }

    #[test]
    fn ray_just_outside_lockout_cone_returns_some() {
        // The complementary safety check: a ray 9° off the axis sits
        // outside the 8° lock-out and must remain pickable, so the
        // bail-out can't shadow the legitimate working region.
        let theta = 9.0_f32.to_radians();
        let dir = Vec3::new(theta.cos(), theta.sin(), 0.0).normalize();
        let ray = Ray {
            origin: Vec3::new(-5.0, 0.0, 0.0),
            direction: dir,
        };
        assert!(closest_point_on_axis(&ray, Vec3::ZERO, Axis::X).is_some());
    }
}

#[cfg(test)]
mod axis_plane_angle_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn point_on_u_axis_is_zero() {
        // For Axis::X, basis is u=Y, v=Z; point on +Y at angle 0.
        let p = Vec3::new(0.0, 1.0, 0.0);
        assert_approx_eq!(
            f32,
            axis_plane_angle(p, Vec3::ZERO, Axis::X),
            0.0,
            epsilon = 1e-6
        );
    }

    #[test]
    fn point_on_v_axis_is_quarter_turn() {
        // For Axis::X, basis is u=Y, v=Z; point on +Z at angle π/2.
        let p = Vec3::new(0.0, 0.0, 1.0);
        assert_approx_eq!(
            f32,
            axis_plane_angle(p, Vec3::ZERO, Axis::X),
            std::f32::consts::FRAC_PI_2,
            epsilon = 1e-6
        );
    }

    #[test]
    fn pivot_translation_does_not_change_angle() {
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let p = pivot + Vec3::new(0.0, 0.0, 1.0);
        assert_approx_eq!(
            f32,
            axis_plane_angle(p, pivot, Axis::X),
            std::f32::consts::FRAC_PI_2,
            epsilon = 1e-6
        );
    }

    #[test]
    fn rh_rule_holds_for_each_axis() {
        // For each axis, the basis (u, v) is right-handed about +axis.
        // Y-axis: u=Z, v=X; point on +Z is at angle 0.
        assert_approx_eq!(
            f32,
            axis_plane_angle(Vec3::Z, Vec3::ZERO, Axis::Y),
            0.0,
            epsilon = 1e-6
        );
        // Z-axis: u=X, v=Y; point on +X is at angle 0.
        assert_approx_eq!(
            f32,
            axis_plane_angle(Vec3::X, Vec3::ZERO, Axis::Z),
            0.0,
            epsilon = 1e-6
        );
    }
}

#[cfg(test)]
mod rotate_drag_angle_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ray_through_pivot_plane_returns_in_plane_angle() {
        // X-axis arc plane = YZ. Camera looks along -X from (5, 1, 0)
        // and the ray hits the YZ plane at (0, 1, 0): on the +u (Y)
        // axis, angle 0.
        let ray = Ray {
            origin: Vec3::new(5.0, 1.0, 0.0),
            direction: -Vec3::X,
        };
        let theta = rotate_drag_angle(&ray, Vec3::ZERO, Axis::X).unwrap();
        assert_approx_eq!(f32, theta, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn ray_parallel_to_plane_returns_none() {
        // X-axis arc plane = YZ. A ray along +Y has zero +X component,
        // so it never crosses the YZ plane (apart from the degenerate
        // case where the ray's origin lies in the plane, which the
        // implementation also rejects via the `t > 0` check).
        let ray = Ray {
            origin: Vec3::new(1.0, 0.0, 0.5),
            direction: Vec3::Y,
        };
        assert!(rotate_drag_angle(&ray, Vec3::ZERO, Axis::X).is_none());
    }

    #[test]
    fn ray_pointing_away_returns_none() {
        // Plane is at x=0; ray sits at x=1 going +X (away from plane).
        let ray = Ray {
            origin: Vec3::new(1.0, 0.0, 0.0),
            direction: Vec3::X,
        };
        assert!(rotate_drag_angle(&ray, Vec3::ZERO, Axis::X).is_none());
    }

    #[test]
    fn moving_cursor_around_pivot_yields_increasing_angle() {
        // Two rays crossing the YZ plane on the camera-facing side
        // (camera at +X), one at angle 0 (point on +Y), one rotated
        // ~30° CCW about +X (point at (0, cos 30°, sin 30°)).
        let r0 = Ray {
            origin: Vec3::new(5.0, 1.0, 0.0),
            direction: -Vec3::X,
        };
        let theta = 30.0_f32.to_radians();
        let r1 = Ray {
            origin: Vec3::new(5.0, theta.cos(), theta.sin()),
            direction: -Vec3::X,
        };
        let a0 = rotate_drag_angle(&r0, Vec3::ZERO, Axis::X).unwrap();
        let a1 = rotate_drag_angle(&r1, Vec3::ZERO, Axis::X).unwrap();
        assert_approx_eq!(f32, a1 - a0, theta, epsilon = 1e-4);
    }
}

#[cfg(test)]
mod scale_factor_along_axis_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ratio_of_anchor_projections_drives_factor() {
        // Press anchor at x = 1, current anchor at x = 2 → factor 2.
        let factor = scale_factor_along_axis(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, 2.0, epsilon = 1e-6);
    }

    #[test]
    fn off_axis_anchor_components_are_ignored() {
        // Only the projection onto the axis matters; y/z drift is
        // irrelevant.
        let factor = scale_factor_along_axis(
            Vec3::new(1.0, 100.0, -200.0),
            Vec3::new(0.5, 999.0, 500.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, 0.5, epsilon = 1e-6);
    }

    #[test]
    fn pivot_translation_does_not_change_factor() {
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let factor = scale_factor_along_axis(
            pivot + Vec3::new(2.0, 0.0, 0.0),
            pivot + Vec3::new(3.0, 0.0, 0.0),
            pivot,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, 1.5, epsilon = 1e-6);
    }

    #[test]
    fn factor_clamps_at_lower_bound() {
        // Press at x = 1, current at x = +tiny → ratio under EPS,
        // clamped to EPS_SCALE.
        let factor = scale_factor_along_axis(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1e-9, 0.0, 0.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, EPS_SCALE, epsilon = 1e-6);
    }

    #[test]
    fn negative_ratio_is_clamped_not_mirrored() {
        // Cursor crosses the pivot to the other side: raw ratio is
        // negative, which the clamp turns into the lower bound.
        // Mirroring through the gizmo is disallowed.
        let factor = scale_factor_along_axis(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-2.0, 0.0, 0.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, EPS_SCALE, epsilon = 1e-6);
    }

    #[test]
    fn factor_clamps_at_upper_bound() {
        // start_proj > 1e-6 (the press-at-pivot guard) but small
        // enough that the raw ratio exceeds `1/EPS_SCALE`.
        let factor = scale_factor_along_axis(
            Vec3::new(1e-4, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::ZERO,
            Axis::X,
        );
        assert_approx_eq!(f32, factor, 1.0 / EPS_SCALE, epsilon = 1e-3);
    }

    #[test]
    fn press_at_pivot_returns_unit_factor() {
        // start_proj ≈ 0 → factor undefined; the no-op fallback is
        // 1.0 so the drag doesn't move the object until the cursor
        // leaves the pivot's neighbourhood.
        let factor =
            scale_factor_along_axis(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO, Axis::X);
        assert_approx_eq!(f32, factor, 1.0, epsilon = 1e-6);
    }
}

#[cfg(test)]
mod uniform_scale_factor_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ratio_of_distances_drives_factor() {
        // Press 50 px away, now 100 px away → factor 2.
        let factor = uniform_scale_factor(0.0, 0.0, 50.0, 0.0, 100.0, 0.0);
        assert_approx_eq!(f32, factor, 2.0, epsilon = 1e-6);
    }

    #[test]
    fn factor_uses_euclidean_distance() {
        // Press at (3, 4) → distance 5. Now at (6, 8) → distance 10.
        // Factor = 10 / 5 = 2.
        let factor = uniform_scale_factor(0.0, 0.0, 3.0, 4.0, 6.0, 8.0);
        assert_approx_eq!(f32, factor, 2.0, epsilon = 1e-6);
    }

    #[test]
    fn pivot_offset_does_not_change_factor() {
        let factor = uniform_scale_factor(100.0, 200.0, 150.0, 200.0, 200.0, 200.0);
        // Press 50 px away, now 100 px away → factor 2.
        assert_approx_eq!(f32, factor, 2.0, epsilon = 1e-6);
    }

    #[test]
    fn factor_clamps_at_lower_bound() {
        // Cursor moves toward the pivot; ratio approaches zero,
        // clamps at EPS_SCALE.
        let factor = uniform_scale_factor(0.0, 0.0, 100.0, 0.0, 1e-9, 0.0);
        assert_approx_eq!(f32, factor, EPS_SCALE, epsilon = 1e-6);
    }

    #[test]
    fn factor_clamps_at_upper_bound() {
        let factor = uniform_scale_factor(0.0, 0.0, 1e-3, 0.0, 1e6, 0.0);
        assert_approx_eq!(f32, factor, 1.0 / EPS_SCALE, epsilon = 1e-3);
    }

    #[test]
    fn press_at_pivot_returns_unit_factor() {
        // Press lands exactly on the projected pivot → no defined
        // factor; helper returns 1.0 so the drag is a no-op.
        let factor = uniform_scale_factor(0.0, 0.0, 0.0, 0.0, 50.0, 0.0);
        assert_approx_eq!(f32, factor, 1.0, epsilon = 1e-6);
    }
}
