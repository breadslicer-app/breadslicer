/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Ray-vs-gizmo-handle hit tests for translate, rotate, and scale.
//! All functions take a world-space `Ray` and return the entry
//! parameter `t` (front-most) plus the picked axis or scale handle.

use glam::Vec3;

use crate::picking::{Ray, ray_hits_aabb_t};

use super::axis::{ALL_AXES, Axis};
use super::dimensions::{MIDPOINT_FRACTION, TRANSLATE_HIT_S_MAX, TRANSLATE_HIT_S_MIN};
use super::tolerances::{LOCKOUT_COS, ROTATE_LOCKOUT_COS};

/// Whether an axis is grabbable. Returns false when the camera is
/// looking too closely along the axis (see `LOCKOUT_COS`). The
/// renderer should also grey the axis tint when this returns false
/// so the user sees that the lock-out is intentional.
pub(crate) fn axis_pickable(ray: &Ray, axis: Axis) -> bool {
    let a = axis.world_dir();
    a.dot(ray.direction).abs() < LOCKOUT_COS
}

/// Hit-test the three world-aligned axis handles against `ray`.
/// Returns the front-most pickable axis (smallest positive `t` on
/// the ray) whose cone cylinder the ray intersects, paired with that
/// `t`, or `None`. The active region is the cone tip *only*; the
/// shaft line below the cone is decorative.
///
/// The returned `t` is exposed so the controller can arbitrate
/// front-most across the three hit-test categories (translate cones,
/// rotate cones, scale midpoint cubes).
pub(crate) fn hit_test(
    ray: &Ray,
    pivot: Vec3,
    length_world: f32,
    radius_world: f32,
) -> Option<(Axis, f32)> {
    let s_min = length_world * TRANSLATE_HIT_S_MIN;
    let s_max = length_world * TRANSLATE_HIT_S_MAX;
    front_most_axis(
        |a| axis_pickable(ray, a),
        |a| ray_hits_finite_cylinder(ray, pivot, a.world_dir(), s_min, s_max, radius_world),
    )
}

/// Walks `ALL_AXES`, keeps only those passing `gate`, runs `intersect`
/// for each, and returns the front-most-`t` axis (smallest positive
/// `t`). Shared body of [`hit_test`], [`rotate_hit_test`], and the
/// per-axis pass of [`scale_hit_test`].
fn front_most_axis<G, I>(gate: G, intersect: I) -> Option<(Axis, f32)>
where
    G: Fn(Axis) -> bool,
    I: Fn(Axis) -> Option<f32>,
{
    let mut best: Option<(Axis, f32)> = None;
    for &axis in &ALL_AXES {
        if !gate(axis) {
            continue;
        }
        if let Some(t) = intersect(axis) {
            match best {
                Some((_, prev_t)) if prev_t <= t => {}
                _ => best = Some((axis, t)),
            }
        }
    }
    best
}

/// Ray-vs-finite-cylinder. The cylinder is centred at `centre`,
/// oriented along the unit vector `axis_dir`, with parameter `s`
/// along the axis ranging over `[s_min, s_max]` and radius `radius`
/// perpendicular to the axis. Returns the smallest positive `t` at
/// which the ray *enters* the cylinder from outside, or `None` if it
/// misses, originates inside, or hits behind the camera.
///
/// Used for both the translate cone (centre = pivot, s ∈ [1, 1.25]
/// times shaft length) and the rotate two-cone spindle (centre = arc
/// midpoint, s ∈ ±cone half-length around 0).
///
/// The intersection is the conjunction of two slabs: the s-range
/// (cap-bounded) and the radial-distance constraint (wall-bounded).
/// `t_in = max(t_s_lo, t_r_lo)`, `t_out = min(t_s_hi, t_r_hi)`; the
/// ray is inside iff `t_in ≤ t_out`. The hit `t` is `t_in` provided
/// it is in front of the ray's origin.
fn ray_hits_finite_cylinder(
    ray: &Ray,
    centre: Vec3,
    axis_dir: Vec3,
    s_min: f32,
    s_max: f32,
    radius: f32,
) -> Option<f32> {
    let q = ray.origin - centre;
    let d_dot_a = ray.direction.dot(axis_dir);
    let q_dot_a = q.dot(axis_dir);

    // Slab 1: cap-bounded s-range.
    let (t_s_lo, t_s_hi) = if d_dot_a.abs() < 1e-10 {
        // Ray parallel to the cylinder's axis. `s` is constant; the
        // ray is in the slab iff its constant `s` lies in range.
        if q_dot_a < s_min || q_dot_a > s_max {
            return None;
        }
        (f32::NEG_INFINITY, f32::INFINITY)
    } else {
        let t1 = (s_min - q_dot_a) / d_dot_a;
        let t2 = (s_max - q_dot_a) / d_dot_a;
        if t1 < t2 { (t1, t2) } else { (t2, t1) }
    };

    // Slab 2: radial distance ≤ radius (wall constraint).
    let dp = ray.direction - axis_dir * d_dot_a;
    let qp = q - axis_dir * q_dot_a;
    let qa = dp.dot(dp);
    let qb = 2.0 * dp.dot(qp);
    let qc = qp.dot(qp) - radius * radius;
    let (t_r_lo, t_r_hi) = if qa.abs() < 1e-10 {
        // Ray parallel to the cylinder's axis (zero radial motion).
        // Inside iff its current radial distance is within `radius`.
        if qc > 0.0 {
            return None;
        }
        (f32::NEG_INFINITY, f32::INFINITY)
    } else {
        let disc = qb * qb - 4.0 * qa * qc;
        if disc < 0.0 {
            return None;
        }
        let sqrt_disc = disc.sqrt();
        let inv_2qa = 1.0 / (2.0 * qa);
        let t1 = (-qb - sqrt_disc) * inv_2qa;
        let t2 = (-qb + sqrt_disc) * inv_2qa;
        if t1 < t2 { (t1, t2) } else { (t2, t1) }
    };

    let t_in = t_s_lo.max(t_r_lo);
    let t_out = t_s_hi.min(t_r_hi);
    if t_in > t_out || t_out <= 0.0 {
        return None;
    }
    // Origin-inside or behind-the-camera entry: reject. The user must
    // be outside the hit volume to grab the handle.
    if t_in <= 0.0 {
        return None;
    }
    Some(t_in)
}

/// Whether a rotate arc is grabbable. Returns `false` when the camera
/// ray sits within 8° of the arc's plane: `|axis · ray.direction|`
/// drops below `cos(82°)` and the perpendicular-plane intersection,
/// the basis of both hit-test and drag math, degenerates. The
/// renderer should grey the arc in that state so the user sees why
/// it is unresponsive.
pub(crate) fn arc_pickable(ray: &Ray, axis: Axis) -> bool {
    let a = axis.world_dir();
    a.dot(ray.direction).abs() >= ROTATE_LOCKOUT_COS
}

/// World-space midpoint of the rotate arc for `axis`, i.e. the
/// position of the two-cone spindle's shared base. The arc is the
/// 90° quadrant of the major circle (radius `arc_radius_world` in
/// the axis-perpendicular plane through `pivot`) centred on the
/// projection of the eye-to-pivot direction onto that plane.
///
/// Returns `None` when either:
/// - `arc_radius_world <= 0.0`: the midpoint would collapse onto
///   `pivot`, and the caller's `(midpoint - pivot).normalize()`
///   would produce NaN.
/// - the camera sits on the axis itself: the in-plane direction
///   degenerates, and the controller's `arc_pickable` gate is
///   expected to have rejected the axis upstream.
fn arc_midpoint_world(ray: &Ray, pivot: Vec3, axis: Axis, arc_radius_world: f32) -> Option<Vec3> {
    if arc_radius_world <= 0.0 {
        return None;
    }
    let a = axis.world_dir();
    let to_camera = ray.origin - pivot;
    let in_plane = to_camera - a * to_camera.dot(a);
    let len = in_plane.length();
    if len < 1e-6 {
        return None;
    }
    Some(pivot + in_plane * (arc_radius_world / len))
}

/// Hit-test the rotate handle for one axis. The handle is two cones
/// sharing their bases at the arc's camera-facing midpoint, tips
/// pointing along ±tangent (the arc tangent at that midpoint, which
/// is perpendicular to both the rotation axis and the radial
/// direction from `pivot`). Approximated as a single finite cylinder
/// bounding both cones (centre at the midpoint, oriented along the
/// tangent, extending ±`cone_length_world`, radius
/// `cone_radius_world`).
///
/// Returns the entry parameter `t` if the ray crosses the bounding
/// cylinder, or `None` if it misses. The torus *tube* itself is no
/// longer a click target; it's a visual aid that connects the
/// two-cone spindle around the rotation axis.
pub(crate) fn ray_hits_rotate_cones(
    ray: &Ray,
    pivot: Vec3,
    axis: Axis,
    arc_radius_world: f32,
    cone_length_world: f32,
    cone_radius_world: f32,
) -> Option<f32> {
    let midpoint = arc_midpoint_world(ray, pivot, axis, arc_radius_world)?;
    let radial = (midpoint - pivot).normalize();
    let tangent = axis.world_dir().cross(radial);
    ray_hits_finite_cylinder(
        ray,
        midpoint,
        tangent,
        -cone_length_world,
        cone_length_world,
        cone_radius_world,
    )
}

/// Hit-test the three world-aligned rotate handles against `ray`.
/// Returns the front-most pickable axis (smallest positive `t`)
/// whose rotate-cone spindle the ray intersects, paired with that
/// `t`, or `None`. Mirrors `hit_test`'s structure for the translate
/// cones, using `arc_pickable` as the lock-out and
/// `ray_hits_rotate_cones` as the per-axis test.
///
/// `arc_radius_world` is the major-circle radius (typically
/// `gizmo_scale * ROTATE_ARC_RADIUS_FRACTION`); `cone_length_world`
/// is each cone's length along the axis; `cone_radius_world` is the
/// shared cone base radius; both follow the per-frame world-per-
/// pixel ratio so the cones keep a constant on-screen size.
pub(crate) fn rotate_hit_test(
    ray: &Ray,
    pivot: Vec3,
    arc_radius_world: f32,
    cone_length_world: f32,
    cone_radius_world: f32,
) -> Option<(Axis, f32)> {
    front_most_axis(
        |a| arc_pickable(ray, a),
        |a| {
            ray_hits_rotate_cones(
                ray,
                pivot,
                a,
                arc_radius_world,
                cone_length_world,
                cone_radius_world,
            )
        },
    )
}

/// Ray-vs-AABB for one axis's midpoint scale cube. The cube is
/// axis-aligned in world space, centred at
/// `pivot + axis_dir * shaft_length_world * MIDPOINT_FRACTION`,
/// with side length `cube_world`. Returns the entry parameter `t`
/// when the ray hits, or `None`.
fn ray_hits_axis_cube(
    ray: &Ray,
    pivot: Vec3,
    axis: Axis,
    shaft_length_world: f32,
    cube_world: f32,
) -> Option<f32> {
    let centre = pivot + axis.world_dir() * (shaft_length_world * MIDPOINT_FRACTION);
    let half = Vec3::splat(cube_world * 0.5);
    ray_hits_aabb_t(ray, centre - half, centre + half)
}

/// Hit-test the three per-axis scale midpoint cubes against `ray`.
/// Returns the front-most pickable axis (smallest positive `t`) whose
/// midpoint cube the ray intersects, or `None`. The per-axis cubes
/// inherit the translate lock-out (the closest-point-on-axis drag
/// math has the same instability as translate near the camera-
/// parallel-to-axis configuration).
pub(crate) fn scale_hit_test(
    ray: &Ray,
    pivot: Vec3,
    shaft_length_world: f32,
    axis_cube_world: f32,
) -> Option<(Axis, f32)> {
    front_most_axis(
        |a| axis_pickable(ray, a),
        |a| ray_hits_axis_cube(ray, pivot, a, shaft_length_world, axis_cube_world),
    )
}

#[cfg(test)]
mod axis_pickable_tests {
    use super::*;

    fn ray_along(d: Vec3) -> Ray {
        Ray {
            origin: Vec3::ZERO,
            direction: d.normalize(),
        }
    }

    #[test]
    fn perpendicular_ray_is_pickable() {
        // Ray along +Z, axis along +X: perfectly perpendicular.
        assert!(axis_pickable(&ray_along(Vec3::Z), Axis::X));
    }

    #[test]
    fn ray_exactly_along_axis_is_locked_out() {
        assert!(!axis_pickable(&ray_along(Vec3::X), Axis::X));
    }

    #[test]
    fn ray_within_threshold_is_locked_out() {
        // 5° off-axis: still inside the 8° lock-out.
        let theta = 5.0_f32.to_radians();
        let d = Vec3::new(theta.cos(), theta.sin(), 0.0);
        assert!(!axis_pickable(&ray_along(d), Axis::X));
    }

    #[test]
    fn ray_just_outside_threshold_is_pickable() {
        // 9° off-axis: outside the 8° lock-out, should be grabbable.
        let theta = 9.0_f32.to_radians();
        let d = Vec3::new(theta.cos(), theta.sin(), 0.0);
        assert!(axis_pickable(&ray_along(d), Axis::X));
    }

    #[test]
    fn lockout_is_symmetric_in_axis_direction() {
        // Ray pointing along -X is just as locked-out as +X; the
        // dot-product magnitude check is sign-agnostic.
        assert!(!axis_pickable(&ray_along(-Vec3::X), Axis::X));
    }
}

#[cfg(test)]
mod ray_hits_finite_cylinder_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn perpendicular_ray_hits_cylinder_at_radius_distance() {
        // Cylinder centred at origin along +X, s ∈ [-1, 1], radius 0.05.
        // Ray crosses through (+0.5, 0, 0) along -Z; entry at z=+0.05.
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let t = ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, -1.0, 1.0, 0.05).unwrap();
        assert_approx_eq!(f32, t, 5.0 - 0.05, epsilon = 1e-4);
    }

    #[test]
    fn ray_outside_radius_misses() {
        let ray = Ray {
            origin: Vec3::new(0.5, 1.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, -1.0, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_outside_s_range_misses() {
        // Ray hits the X axis at x = 5; cylinder s ∈ [-1, 1] excludes
        // it. Even with the wider radius the s-test rejects.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, -1.0, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_pointing_away_from_cylinder_misses() {
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: Vec3::Z, // away from origin
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, -1.0, 1.0, 0.05).is_none());
    }

    #[test]
    fn translate_cone_region_rejects_shaft_clicks() {
        // Translate hit uses s ∈ [length, length·1.25]; clicks on
        // the shaft (s in [0, length]) must miss. Pin the contract.
        // length = 1.0; ray crosses (0.5, 0, *) at s = 0.5.
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, 1.0, 1.25, 0.05).is_none());
    }

    #[test]
    fn translate_cone_region_accepts_cone_clicks() {
        // Same cylinder, ray crosses at s = 1.1 (inside cone region).
        let ray = Ray {
            origin: Vec3::new(1.1, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_finite_cylinder(&ray, Vec3::ZERO, Vec3::X, 1.0, 1.25, 0.05).is_some());
    }
}

#[cfg(test)]
mod hit_test_tests {
    use super::*;

    #[test]
    fn empty_space_returns_none() {
        let ray = Ray {
            origin: Vec3::new(100.0, 100.0, 100.0),
            direction: -Vec3::Z,
        };
        assert!(hit_test(&ray, Vec3::ZERO, 1.0, 0.05).is_none());
    }

    #[test]
    fn x_axis_ray_picks_x_handle() {
        // Ray crosses (1.1, 0, 0), inside the cone region
        // (s ∈ [1.0, 1.25]).
        let ray = Ray {
            origin: Vec3::new(1.1, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            hit_test(&ray, Vec3::ZERO, 1.0, 0.05).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn shaft_line_clicks_miss_translate_handle() {
        // Click on the visible shaft line (s ∈ [0, 1.0)) must NOT
        // start a translate drag; the shaft is decorative, only the
        // cone tip is the active translate handle. The midpoint
        // (s = 0.5) belongs to the per-axis scale cube anyway.
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(hit_test(&ray, Vec3::ZERO, 1.0, 0.05).is_none());
    }

    #[test]
    fn frontmost_handle_wins_when_screen_overlap() {
        // Both X and Y cones near the cone region. Construct a ray
        // that hits the X cone (s ≈ 1.1 along +X). The Y cone is
        // around (0, 1.1, *), which the ray (offset 1.1 in x) is
        // far from: Y misses, X wins.
        let ray = Ray {
            origin: Vec3::new(1.1, 0.05, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            hit_test(&ray, Vec3::ZERO, 1.0, 0.05).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn locked_out_axis_is_not_picked() {
        // Ray pointing nearly along +X: X is locked out by the
        // 8° threshold in `axis_pickable`.
        // A barely-perpendicular component still places the ray
        // close enough to the X handle that the cylinder math would
        // otherwise hit. The lock-out should override.
        let theta = 5.0_f32.to_radians();
        let dir = Vec3::new(theta.cos(), 0.0, theta.sin()).normalize();
        let ray = Ray {
            origin: Vec3::new(-1.0, 0.0, -theta.tan()),
            direction: dir,
        };
        // We don't assert non-None for a lock-out-disabled axis;
        // we assert it's not X. Y/Z might or might not be hit
        // depending on the geometry; the only contract is X is
        // forbidden.
        let picked = hit_test(&ray, Vec3::ZERO, 1.0, 0.05).map(|(a, _)| a);
        assert_ne!(picked, Some(Axis::X));
    }
}

#[cfg(test)]
mod arc_pickable_tests {
    use super::*;

    fn ray_along(d: Vec3) -> Ray {
        Ray {
            origin: Vec3::ZERO,
            direction: d.normalize(),
        }
    }

    #[test]
    fn ray_perpendicular_to_arc_plane_is_pickable() {
        // X-axis arc lives in the YZ plane. A ray along +X is
        // perfectly perpendicular to that plane, i.e. axis-parallel,
        // so the arc faces the camera squarely.
        assert!(arc_pickable(&ray_along(Vec3::X), Axis::X));
    }

    #[test]
    fn ray_in_arc_plane_is_locked_out() {
        // X-axis arc lives in the YZ plane. A ray along +Y lies
        // exactly in that plane; the arc is edge-on.
        assert!(!arc_pickable(&ray_along(Vec3::Y), Axis::X));
        assert!(!arc_pickable(&ray_along(Vec3::Z), Axis::X));
    }

    #[test]
    fn ray_within_5_degrees_of_arc_plane_is_locked_out() {
        // 5° off the YZ plane (the X arc's plane) → 85° from +X.
        // |axis · ray| = sin(5°) ≈ 0.087 < cos(82°) ≈ 0.139.
        let theta = 5.0_f32.to_radians();
        let d = Vec3::new(theta.sin(), theta.cos(), 0.0);
        assert!(!arc_pickable(&ray_along(d), Axis::X));
    }

    #[test]
    fn ray_just_outside_threshold_is_pickable() {
        // 9° off the YZ plane → 81° from +X.
        // |axis · ray| = sin(9°) ≈ 0.156 > cos(82°) ≈ 0.139.
        let theta = 9.0_f32.to_radians();
        let d = Vec3::new(theta.sin(), theta.cos(), 0.0);
        assert!(arc_pickable(&ray_along(d), Axis::X));
    }

    #[test]
    fn lockout_is_symmetric_in_axis_direction() {
        // Ray along -X is just as pickable as +X; the
        // dot-product magnitude is sign-agnostic.
        assert!(arc_pickable(&ray_along(-Vec3::X), Axis::X));
    }
}

#[cfg(test)]
mod ray_hits_rotate_cones_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn ray_through_camera_facing_midpoint_hits() {
        // X-axis arc midpoint faces the camera. Place camera at
        // (5, 0.1, 0): the in-plane camera direction (in YZ) is +Y,
        // so the arc midpoint is at world (0, 1, 0) for arc_radius=1.
        // The two rotate cones share their base there, tips at +X and
        // -X with cone_length = 0.18. A ray crossing (0, 1, 0) along
        // -X enters the bounding cylinder.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert!(ray_hits_rotate_cones(&ray, Vec3::ZERO, Axis::X, 1.0, 0.18, 0.07).is_some());
    }

    #[test]
    fn arc_midpoint_falls_back_when_camera_on_axis() {
        // Camera exactly on +X axis → in-plane camera projection is
        // zero → the helper returns `None`, and the hit test follows
        // suit.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.0, 0.0),
            direction: -Vec3::X,
        };
        assert!(arc_midpoint_world(&ray, Vec3::ZERO, Axis::X, 1.0).is_none());
        assert!(ray_hits_rotate_cones(&ray, Vec3::ZERO, Axis::X, 1.0, 0.18, 0.07).is_none());
    }

    #[test]
    fn arc_midpoint_rejects_zero_radius() {
        // Zero radius collapses the midpoint onto the pivot, which
        // would make the caller's `(midpoint - pivot).normalize()`
        // produce NaN. The helper must reject this up front.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert!(arc_midpoint_world(&ray, Vec3::ZERO, Axis::X, 0.0).is_none());
    }

    #[test]
    fn arc_midpoint_rejects_negative_radius() {
        // A negative radius is geometrically nonsensical and would
        // place the midpoint on the far side of the pivot from the
        // camera. The helper must reject it.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert!(arc_midpoint_world(&ray, Vec3::ZERO, Axis::X, -1.0).is_none());
    }

    #[test]
    fn rotate_cones_reject_zero_radius_without_nan() {
        // Regression: a zero `arc_radius_world` used to
        // make `ray_hits_rotate_cones` compute
        // `(pivot - pivot).normalize()` (NaN), then feed those NaNs
        // through the cylinder math. The fix in `arc_midpoint_world`
        // makes the `?` short-circuit on the first line, so the hit
        // test returns `None` deterministically.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert!(ray_hits_rotate_cones(&ray, Vec3::ZERO, Axis::X, 0.0, 0.18, 0.07).is_none());
    }

    #[test]
    fn ray_far_from_midpoint_misses() {
        // Camera at (5, 0.1, 0), arc midpoint at world (0, 1, 0). A
        // ray crossing (0, -1, 0), the *back* side of the rotation
        // axis, 180° away, is far from the midpoint cone spindle.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, -1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert!(ray_hits_rotate_cones(&ray, Vec3::ZERO, Axis::X, 1.0, 0.18, 0.07).is_none());
    }

    #[test]
    fn arc_midpoint_lies_on_camera_facing_radial() {
        // Sanity for the helper: with camera at (0, 0, 5), the X
        // arc's in-plane camera direction is +Z, so the midpoint
        // sits at (0, 0, R).
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let m = arc_midpoint_world(&ray, Vec3::ZERO, Axis::X, 1.5).unwrap();
        assert_approx_eq!(f32, m.x, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, m.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, m.z, 1.5, epsilon = 1e-6);
    }
}

#[cfg(test)]
mod rotate_hit_test_tests {
    use super::*;

    #[test]
    fn empty_space_returns_none() {
        let ray = Ray {
            origin: Vec3::new(100.0, 100.0, 100.0),
            direction: Vec3::new(1.0, 0.0, 0.0),
        };
        assert!(rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.18, 0.07).is_none());
    }

    #[test]
    fn ray_through_x_arc_midpoint_picks_x() {
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert_eq!(
            rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.18, 0.07).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn locked_out_axis_is_skipped() {
        // X arc lives in the YZ plane; ray inside that plane is
        // edge-on, so `arc_pickable` rejects X. Y/Z may or may not
        // be hit; the only contract is X is forbidden.
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, 0.0),
            direction: -Vec3::Y,
        };
        assert_ne!(
            rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.18, 0.07).map(|(a, _)| a),
            Some(Axis::X)
        );
    }
}

#[cfg(test)]
mod ray_hits_axis_cube_tests {
    use super::*;

    #[test]
    fn cube_sits_at_shaft_midpoint_not_tip() {
        // X-axis cube with shaft length 2.0 must be centred at x = 1.0
        // (midpoint), *not* x = 2.0 (tip). Ray pointing through (1, 0)
        // hits; ray pointing through (2, 0) misses (cube extents are
        // [0.95, 1.05] for cube_world = 0.1).
        let ray_at_midpoint = Ray {
            origin: Vec3::new(1.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let ray_at_tip = Ray {
            origin: Vec3::new(2.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray_at_midpoint, Vec3::ZERO, Axis::X, 2.0, 0.1).is_some());
        assert!(ray_hits_axis_cube(&ray_at_tip, Vec3::ZERO, Axis::X, 2.0, 0.1).is_none());
    }

    #[test]
    fn cube_centre_uses_axis_dir() {
        // Y-axis cube on a shaft of length 4 sits at (0, 2, 0).
        let ray = Ray {
            origin: Vec3::new(0.0, 2.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray, Vec3::ZERO, Axis::Y, 4.0, 0.1).is_some());
    }

    #[test]
    fn ray_outside_cube_misses() {
        let ray = Ray {
            origin: Vec3::new(1.0, 5.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray, Vec3::ZERO, Axis::X, 2.0, 0.1).is_none());
    }

    #[test]
    fn pivot_translation_carries_cube() {
        // Scene-space pivot at (10, 20, 30) → X cube centred at
        // (10 + 1, 20, 30) for shaft length 2.
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let ray = Ray {
            origin: Vec3::new(11.0, 20.0, 35.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray, pivot, Axis::X, 2.0, 0.1).is_some());
    }
}

#[cfg(test)]
mod scale_hit_test_tests {
    use super::*;

    #[test]
    fn ray_through_x_midpoint_returns_x_axis_handle() {
        let ray = Ray {
            origin: Vec3::new(1.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn empty_space_returns_none() {
        let ray = Ray {
            origin: Vec3::new(100.0, 100.0, 100.0),
            direction: -Vec3::Z,
        };
        assert!(scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1).is_none());
    }

    #[test]
    fn frontmost_t_wins_between_axis_cubes() {
        // A ray hitting both Y and Z midpoint cubes: starts at
        // (0, 5, -4), direction toward (0, 1, 0) then continuing on
        // through (0, 0, 1). Y cube hits first (smaller `t`).
        // Direction is 45° off both Y and Z so neither axis is
        // locked out.
        let dir = Vec3::new(0.0, -1.0, 1.0).normalize();
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, -4.0),
            direction: dir,
        };
        assert_eq!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1).map(|(a, _)| a),
            Some(Axis::Y)
        );
    }

    #[test]
    fn locked_out_axis_cube_is_skipped() {
        // Ray nearly along +X: X axis is lock-out, the X cube must
        // not be returned even if the ray geometrically crosses it.
        let theta = 5.0_f32.to_radians();
        let dir = Vec3::new(theta.cos(), 0.0, theta.sin()).normalize();
        let ray = Ray {
            origin: Vec3::new(-5.0, 0.0, 0.0),
            direction: dir,
        };
        assert_ne!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1).map(|(a, _)| a),
            Some(Axis::X)
        );
    }
}
