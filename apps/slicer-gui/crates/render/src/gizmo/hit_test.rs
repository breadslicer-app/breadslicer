/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Ray-vs-gizmo-handle hit tests for translate, rotate, and scale. All
//! functions take a world-space `Ray` and return the entry parameter `t`
//! (front-most) plus the picked axis or scale handle.

use glam::Vec3;

use crate::picking::{Ray, ray_hits_aabb_t};

use super::axis::{ALL_AXES, Axis, axis_basis};
#[cfg(test)]
use super::dimensions::MIDPOINT_FRACTION;
use super::dimensions::{TRANSLATE_HIT_S_MAX, TRANSLATE_HIT_S_MIN};
use super::tolerances::{LOCKOUT_COS, PLANE_PICK_MIN, ROTATE_LOCKOUT_COS};

/// Straight pieces a rotate ring is split into for its hit test.
const RING_HIT_SEGMENTS: u32 = 128;

/// A point of a ring this far behind the pivot (as a fraction of the ring
/// radius, seen from the eye) still counts as the front half.
const RING_FRONT_SLACK: f32 = 0.05;

/// Whether an axis is grabbable. Returns false when the camera is looking too
/// closely along the axis (see `LOCKOUT_COS`). The renderer fades such an
/// axis out, so the user sees why it is unresponsive.
pub(crate) fn axis_pickable(ray: &Ray, axis: Axis) -> bool {
    let a = axis.world_dir();
    a.dot(ray.direction).abs() < LOCKOUT_COS
}

/// Hit-test the three world-aligned move arrows against `ray`. Returns the
/// front-most pickable axis (smallest positive `t` on the ray) whose hit
/// cylinder the ray enters, paired with that `t`, or `None`. The whole arrow
/// grabs, from just past the centre ball to slightly beyond the cone tip.
///
/// The returned `t` is exposed so the controller can arbitrate front-most
/// across the hit-test categories (arrows, rings, scale cubes, centre ball).
pub fn hit_test(
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

/// Walks `ALL_AXES`, keeps only those passing `gate`, runs `intersect` for
/// each, and returns the front-most-`t` axis (smallest positive `t`). Shared
/// body of [`hit_test`], [`rotate_hit_test`], and [`scale_hit_test`].
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

/// Ray-vs-finite-cylinder. The cylinder is centred at `centre`, oriented along
/// the unit vector `axis_dir`, with parameter `s` along the axis ranging over
/// `[s_min, s_max]` and radius `radius` perpendicular to the axis. Returns the
/// smallest positive `t` at which the ray *enters* the cylinder from outside,
/// or `None` if it misses, originates inside, or hits behind the camera.
///
/// The intersection is the conjunction of two slabs: the s-range (cap-bounded)
/// and the radial-distance constraint (wall-bounded).
/// `t_in = max(t_s_lo, t_r_lo)`, `t_out = min(t_s_hi, t_r_hi)`; the ray is
/// inside iff `t_in ≤ t_out`. The hit `t` is `t_in` provided it is in front of
/// the ray's origin.
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
        // Ray parallel to the cylinder's axis. `s` is constant; the ray is in
        // the slab iff its constant `s` lies in range.
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
        // Ray parallel to the cylinder's axis (zero radial motion). Inside iff
        // its current radial distance is within `radius`.
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
    // Origin-inside or behind-the-camera entry: reject. The user must be
    // outside the hit volume to grab the handle.
    if t_in <= 0.0 {
        return None;
    }
    Some(t_in)
}

/// Whether a rotate ring is grabbable. Returns `false` when the camera ray
/// sits within 8° of the ring's plane: `|axis · ray.direction|` drops below
/// `cos(82°)` and the perpendicular-plane intersection, the basis of the drag
/// math, degenerates. The renderer fades the ring in that state so the user
/// sees why it is unresponsive.
pub(crate) fn arc_pickable(ray: &Ray, axis: Axis) -> bool {
    let a = axis.world_dir();
    a.dot(ray.direction).abs() >= ROTATE_LOCKOUT_COS
}

/// Closest approach of `ray` to the segment `a`–`b`: the distance between the
/// two, the ray parameter `t` there, and the point on the segment.
fn ray_segment_closest(ray: &Ray, a: Vec3, b: Vec3) -> (f32, f32, Vec3) {
    let e = b - a;
    let w0 = ray.origin - a;
    let d_dot_e = ray.direction.dot(e);
    let e_dot_e = e.dot(e);
    let d_dot_w = ray.direction.dot(w0);
    let e_dot_w = e.dot(w0);
    // The ray direction is unit, so `d · d = 1`.
    let denom = e_dot_e - d_dot_e * d_dot_e;
    let s = if denom.abs() < 1e-12 || e_dot_e < 1e-12 {
        0.0
    } else {
        ((e_dot_w - d_dot_e * d_dot_w) / denom).clamp(0.0, 1.0)
    };
    let t = d_dot_e * s - d_dot_w;
    let on_segment = a + e * s;
    let on_ray = ray.origin + ray.direction * t;
    ((on_ray - on_segment).length(), t, on_segment)
}

/// Hit-test the full rotate ring around `axis`: the circle of radius
/// `radius_world` through `pivot` in the plane perpendicular to the axis,
/// within `tolerance_world` of the ray. Only the half facing the eye grabs
/// (`toward_eye`, unit, from the pivot: the same split the renderer draws);
/// the back half is drawn faint and lets presses through to what is in front.
/// Returns the ray parameter `t` at the closest approach, or `None`.
pub(crate) fn ray_hits_ring(
    ray: &Ray,
    pivot: Vec3,
    axis: Axis,
    radius_world: f32,
    tolerance_world: f32,
    toward_eye: Vec3,
) -> Option<f32> {
    if radius_world <= 0.0 {
        return None;
    }
    let (u, v) = axis_basis(axis);
    let point = |i: u32| {
        let a = i as f32 / RING_HIT_SEGMENTS as f32 * std::f32::consts::TAU;
        pivot + (u * a.cos() + v * a.sin()) * radius_world
    };
    let mut best: Option<(f32, f32)> = None;
    let mut prev = point(0);
    for i in 1..=RING_HIT_SEGMENTS {
        let next = point(i);
        let (dist, t, at) = ray_segment_closest(ray, prev, next);
        prev = next;
        if dist > tolerance_world || t <= 0.0 {
            continue;
        }
        if (at - pivot).dot(toward_eye) < -RING_FRONT_SLACK * radius_world {
            continue;
        }
        if best.is_none_or(|(d, _)| dist < d) {
            best = Some((dist, t));
        }
    }
    best.map(|(_, t)| t)
}

/// Hit-test the three rotate rings against `ray`. Returns the front-most
/// pickable axis (smallest positive `t`) whose ring the ray passes close to,
/// paired with that `t`, or `None`. Uses `arc_pickable` as the lock-out.
pub fn rotate_hit_test(
    ray: &Ray,
    pivot: Vec3,
    radius_world: f32,
    tolerance_world: f32,
    toward_eye: Vec3,
) -> Option<(Axis, f32)> {
    front_most_axis(
        |a| arc_pickable(ray, a),
        |a| ray_hits_ring(ray, pivot, a, radius_world, tolerance_world, toward_eye),
    )
}

/// Ray-vs-AABB for one axis's scale cube. The cube is axis-aligned in world
/// space, centred at `pivot + axis_dir * shaft_length_world * cube_at`
/// (`MIDPOINT_FRACTION` in the outline style), with side length
/// `cube_world`. Returns the entry parameter `t` when the ray hits, or `None`.
fn ray_hits_axis_cube(
    ray: &Ray,
    pivot: Vec3,
    axis: Axis,
    shaft_length_world: f32,
    cube_world: f32,
    cube_at: f32,
) -> Option<f32> {
    let centre = pivot + axis.world_dir() * (shaft_length_world * cube_at);
    let half = Vec3::splat(cube_world * 0.5);
    ray_hits_aabb_t(ray, centre - half, centre + half)
}

/// Hit-test the three per-axis scale cubes against `ray`. Returns the
/// front-most pickable axis (smallest positive `t`) whose cube the ray
/// intersects, or `None`. The cubes inherit the translate lock-out (the
/// closest-point-on-axis drag math has the same instability as translate near
/// the camera-parallel-to-axis configuration).
pub fn scale_hit_test(
    ray: &Ray,
    pivot: Vec3,
    shaft_length_world: f32,
    axis_cube_world: f32,
    cube_at: f32,
) -> Option<(Axis, f32)> {
    front_most_axis(
        |a| axis_pickable(ray, a),
        |a| ray_hits_axis_cube(ray, pivot, a, shaft_length_world, axis_cube_world, cube_at),
    )
}

/// Where a plane square sits and how far around it a press still grabs it,
/// in world units: the centre of the square is `offset` along each of its two
/// axes from the pivot, on the side of each that faces the camera looking
/// along `view_dir` (see [`plane_quadrant`]), its side `side`, and `margin`
/// more on each side grabs.
#[derive(Clone, Copy, Debug)]
pub struct PlaneSquares {
    pub offset: f32,
    pub side: f32,
    pub margin: f32,
    pub view_dir: Vec3,
}

/// Below this `|axis · view|`, an axis counts as across the view and its
/// square stays on the positive side: no flipping back and forth on a view
/// along a plane of the axes (the presets).
const PLANE_FLIP_DEAD_ZONE: f32 = 0.05;

/// The quadrant of the plane perpendicular to `normal` where its square
/// sits: for each of the plane's two axes (in [`Axis::plane_axes`] order),
/// `-1` when that axis points away from the camera looking along `view_dir`,
/// `1` otherwise. The square faces the camera, clear of the arrows, which
/// keep to their positive axes: seen from the default view, a square in the
/// positive quadrant would sit behind the Z arrow.
pub fn plane_quadrant(normal: Axis, view_dir: Vec3) -> (f32, f32) {
    let side = |a: Axis| {
        if a.world_dir().dot(view_dir) > PLANE_FLIP_DEAD_ZONE {
            -1.0
        } else {
            1.0
        }
    };
    let (u, v) = normal.plane_axes();
    (side(u), side(v))
}

/// Whether the plane square perpendicular to `normal` is grabbable: not while
/// the ray runs close to its plane (see `PLANE_PICK_MIN`).
pub(crate) fn plane_pickable(ray: &Ray, normal: Axis) -> bool {
    normal.world_dir().dot(ray.direction).abs() >= PLANE_PICK_MIN
}

/// Hit-test the three plane squares (#207): the square of the plane through
/// `pivot` perpendicular to each axis, in the quadrant of its plane that
/// faces the camera ([`plane_quadrant`]). Returns the front-most grabbed
/// square's normal and the ray parameter where the ray cuts its plane.
pub fn plane_hit_test(ray: &Ray, pivot: Vec3, squares: PlaneSquares) -> Option<(Axis, f32)> {
    front_most_axis(
        |a| plane_pickable(ray, a),
        |normal| {
            let n = normal.world_dir();
            let denom = ray.direction.dot(n);
            if denom.abs() < 1e-6 {
                return None;
            }
            let t = (pivot - ray.origin).dot(n) / denom;
            if t <= 0.0 {
                return None;
            }
            let at = ray.origin + ray.direction * t - pivot;
            let (u, v) = normal.plane_axes();
            let (su, sv) = plane_quadrant(normal, squares.view_dir);
            let reach = squares.side * 0.5 + squares.margin;
            let inside =
                |a: Axis, sign: f32| (at.dot(a.world_dir()) - sign * squares.offset).abs() <= reach;
            (inside(u, su) && inside(v, sv)).then_some(t)
        },
    )
}

/// Hit-test the uniform-scale ball: a sphere of `radius_world` at `pivot`.
/// Returns the entry `t`, or `None` on a miss or from inside.
pub fn centre_hit_test(ray: &Ray, pivot: Vec3, radius_world: f32) -> Option<f32> {
    let q = ray.origin - pivot;
    let b = q.dot(ray.direction);
    let c = q.dot(q) - radius_world * radius_world;
    let disc = b * b - c;
    if disc < 0.0 || c <= 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    (t > 0.0).then_some(t)
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
        // Ray pointing along -X is just as locked-out as +X; the dot-product
        // magnitude check is sign-agnostic.
        assert!(!axis_pickable(&ray_along(-Vec3::X), Axis::X));
    }
}

#[cfg(test)]
mod ray_hits_finite_cylinder_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn perpendicular_ray_hits_cylinder_at_radius_distance() {
        // Cylinder centred at origin along +X, s ∈ [-1, 1], radius 0.05. Ray
        // crosses through (+0.5, 0, 0) along -Z; entry at z=+0.05.
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
        // Ray hits the X axis at x = 5; cylinder s ∈ [-1, 1] excludes it. Even
        // with the wider radius the s-test rejects.
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
        // Ray crosses (1.1, 0, 0), on the cone.
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
    fn the_whole_shaft_grabs_not_only_the_cone() {
        for s in [0.2f32, 0.5, 0.9, 1.1] {
            let ray = Ray {
                origin: Vec3::new(s, 0.0, 5.0),
                direction: -Vec3::Z,
            };
            assert_eq!(
                hit_test(&ray, Vec3::ZERO, 1.0, 0.05).map(|(a, _)| a),
                Some(Axis::X),
                "s = {s}"
            );
        }
        // Right at the centre, the ball takes the press, not the arrows.
        let ray = Ray {
            origin: Vec3::new(0.05, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(hit_test(&ray, Vec3::ZERO, 1.0, 0.05).is_none());
    }

    #[test]
    fn frontmost_handle_wins_when_screen_overlap() {
        // Both X and Y cones near the cone region. Construct a ray that hits
        // the X cone (s ≈ 1.1 along +X). The Y cone is around (0, 1.1, *),
        // which the ray (offset 1.1 in x) is far from: Y misses, X wins.
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
        // Ray pointing nearly along +X: X is locked out by the 8° threshold in
        // `axis_pickable`. A barely-perpendicular component still places the
        // ray close enough to the X handle that the cylinder math would
        // otherwise hit. The lock-out should override.
        let theta = 5.0_f32.to_radians();
        let dir = Vec3::new(theta.cos(), 0.0, theta.sin()).normalize();
        let ray = Ray {
            origin: Vec3::new(-1.0, 0.0, -theta.tan()),
            direction: dir,
        };
        // We don't assert non-None for a lock-out-disabled axis; we assert it's
        // not X. Y/Z might or might not be hit depending on the geometry; the
        // only contract is X is forbidden.
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
        // X-axis arc lives in the YZ plane. A ray along +X is perfectly
        // perpendicular to that plane, i.e. axis-parallel, so the arc faces the
        // camera squarely.
        assert!(arc_pickable(&ray_along(Vec3::X), Axis::X));
    }

    #[test]
    fn ray_in_arc_plane_is_locked_out() {
        // X-axis arc lives in the YZ plane. A ray along +Y lies exactly in that
        // plane; the arc is edge-on.
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
        // Ray along -X is just as pickable as +X; the dot-product magnitude is
        // sign-agnostic.
        assert!(arc_pickable(&ray_along(-Vec3::X), Axis::X));
    }
}

#[cfg(test)]
mod ray_hits_ring_tests {
    use super::*;

    fn ray_through(origin: Vec3, target: Vec3) -> Ray {
        Ray {
            origin,
            direction: (target - origin).normalize(),
        }
    }

    #[test]
    fn the_whole_front_half_of_the_ring_grabs_not_only_its_middle() {
        // Z ring of radius 1 seen from above at an angle: every point of the
        // ring on the eye's side grabs, at any angle around it.
        let eye = Vec3::new(0.0, -3.0, 4.0);
        for deg in [-170.0f32, -120.0, -90.0, -45.0, -10.0] {
            let a = deg.to_radians();
            let on_ring = Vec3::new(a.cos(), a.sin(), 0.0);
            let t = ray_hits_ring(
                &ray_through(eye, on_ring),
                Vec3::ZERO,
                Axis::Z,
                1.0,
                0.02,
                eye.normalize(),
            );
            let t = t.unwrap_or_else(|| panic!("{deg}° does not grab"));
            assert!((t - (on_ring - eye).length()).abs() < 0.03);
        }
    }

    #[test]
    fn the_back_half_of_the_ring_does_not_grab() {
        let eye = Vec3::new(0.0, -3.0, 4.0);
        for deg in [60.0f32, 90.0, 120.0] {
            let a = deg.to_radians();
            let on_ring = Vec3::new(a.cos(), a.sin(), 0.0);
            assert!(
                ray_hits_ring(
                    &ray_through(eye, on_ring),
                    Vec3::ZERO,
                    Axis::Z,
                    1.0,
                    0.02,
                    eye.normalize()
                )
                .is_none(),
                "{deg}° grabs from behind"
            );
        }
    }

    #[test]
    fn a_press_near_the_ring_grabs_within_the_tolerance_only() {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let near = Vec3::new(1.015, 0.0, 0.0);
        let far = Vec3::new(1.05, 0.0, 0.0);
        assert!(
            ray_hits_ring(
                &ray_through(eye, near),
                Vec3::ZERO,
                Axis::Z,
                1.0,
                0.02,
                Vec3::Z
            )
            .is_some()
        );
        assert!(
            ray_hits_ring(
                &ray_through(eye, far),
                Vec3::ZERO,
                Axis::Z,
                1.0,
                0.02,
                Vec3::Z
            )
            .is_none()
        );
        // The middle of the ring is empty.
        assert!(
            ray_hits_ring(
                &ray_through(eye, Vec3::ZERO),
                Vec3::ZERO,
                Axis::Z,
                1.0,
                0.02,
                Vec3::Z
            )
            .is_none()
        );
    }

    #[test]
    fn a_ring_follows_the_pivot_and_rejects_a_zero_radius() {
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let eye = pivot + Vec3::new(0.0, 0.0, 5.0);
        let target = pivot + Vec3::new(0.0, 1.0, 0.0);
        assert!(
            ray_hits_ring(
                &ray_through(eye, target),
                pivot,
                Axis::Z,
                1.0,
                0.02,
                Vec3::Z
            )
            .is_some()
        );
        assert!(
            ray_hits_ring(
                &ray_through(eye, target),
                pivot,
                Axis::Z,
                0.0,
                0.02,
                Vec3::Z
            )
            .is_none()
        );
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
        assert!(rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.05, ray.origin.normalize()).is_none());
    }

    #[test]
    fn ray_onto_the_x_ring_picks_x() {
        // From +X, slightly off, onto the X ring at +Y.
        let origin = Vec3::new(5.0, 0.1, 0.0);
        let ray = Ray {
            origin,
            direction: (Vec3::new(0.0, 1.0, 0.0) - origin).normalize(),
        };
        assert_eq!(
            rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.05, ray.origin.normalize()).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn locked_out_axis_is_skipped() {
        // X ring lives in the YZ plane; ray inside that plane is edge-on, so
        // `arc_pickable` rejects X.
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, 0.0),
            direction: -Vec3::Y,
        };
        assert_ne!(
            rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.05, ray.origin.normalize()).map(|(a, _)| a),
            Some(Axis::X)
        );
    }
}

#[cfg(test)]
mod ray_hits_axis_cube_tests {
    use super::*;

    #[test]
    fn cube_sits_on_the_shaft_not_at_the_tip() {
        let at = 2.0 * MIDPOINT_FRACTION;
        let ray_at_cube = Ray {
            origin: Vec3::new(at, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let ray_at_tip = Ray {
            origin: Vec3::new(2.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(
            ray_hits_axis_cube(
                &ray_at_cube,
                Vec3::ZERO,
                Axis::X,
                2.0,
                0.1,
                MIDPOINT_FRACTION
            )
            .is_some()
        );
        assert!(
            ray_hits_axis_cube(
                &ray_at_tip,
                Vec3::ZERO,
                Axis::X,
                2.0,
                0.1,
                MIDPOINT_FRACTION
            )
            .is_none()
        );
    }

    #[test]
    fn cube_centre_uses_axis_dir() {
        let ray = Ray {
            origin: Vec3::new(0.0, 4.0 * MIDPOINT_FRACTION, 5.0),
            direction: -Vec3::Z,
        };
        assert!(
            ray_hits_axis_cube(&ray, Vec3::ZERO, Axis::Y, 4.0, 0.1, MIDPOINT_FRACTION).is_some()
        );
    }

    #[test]
    fn ray_outside_cube_misses() {
        let ray = Ray {
            origin: Vec3::new(1.0, 5.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(
            ray_hits_axis_cube(&ray, Vec3::ZERO, Axis::X, 2.0, 0.1, MIDPOINT_FRACTION).is_none()
        );
    }

    #[test]
    fn pivot_translation_carries_cube() {
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let ray = Ray {
            origin: Vec3::new(10.0 + 2.0 * MIDPOINT_FRACTION, 20.0, 35.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_cube(&ray, pivot, Axis::X, 2.0, 0.1, MIDPOINT_FRACTION).is_some());
    }
}

#[cfg(test)]
mod scale_hit_test_tests {
    use super::*;

    #[test]
    fn ray_through_the_x_cube_returns_x() {
        let ray = Ray {
            origin: Vec3::new(2.0 * MIDPOINT_FRACTION, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, MIDPOINT_FRACTION).map(|(a, _)| a),
            Some(Axis::X)
        );
    }

    #[test]
    fn empty_space_returns_none() {
        let ray = Ray {
            origin: Vec3::new(100.0, 100.0, 100.0),
            direction: -Vec3::Z,
        };
        assert!(scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, MIDPOINT_FRACTION).is_none());
    }

    #[test]
    fn frontmost_t_wins_between_axis_cubes() {
        // A ray through both the Y and the Z cube, 45° off both axes: the Y
        // cube is nearer the origin of the ray.
        let m = 2.0 * MIDPOINT_FRACTION;
        let dir = Vec3::new(0.0, -1.0, 1.0).normalize();
        let ray = Ray {
            origin: Vec3::new(0.0, m, 0.0) - dir * 5.0,
            direction: dir,
        };
        assert_eq!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, MIDPOINT_FRACTION).map(|(a, _)| a),
            Some(Axis::Y)
        );
    }

    #[test]
    fn locked_out_axis_cube_is_skipped() {
        let theta = 5.0_f32.to_radians();
        let dir = Vec3::new(theta.cos(), 0.0, theta.sin()).normalize();
        let ray = Ray {
            origin: Vec3::new(-5.0, 0.0, 0.0),
            direction: dir,
        };
        assert_ne!(
            scale_hit_test(&ray, Vec3::ZERO, 2.0, 0.1, MIDPOINT_FRACTION).map(|(a, _)| a),
            Some(Axis::X)
        );
    }
}

#[cfg(test)]
mod centre_hit_test_tests {
    use super::*;

    #[test]
    fn the_ball_grabs_at_the_pivot_and_not_beside_it() {
        let pivot = Vec3::new(1.0, 2.0, 3.0);
        let hit = Ray {
            origin: pivot + Vec3::new(0.05, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let miss = Ray {
            origin: pivot + Vec3::new(0.2, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let t = centre_hit_test(&hit, pivot, 0.1).unwrap();
        assert!(t > 4.8 && t < 5.0);
        assert!(centre_hit_test(&miss, pivot, 0.1).is_none());
    }
}

#[cfg(test)]
mod plane_hit_test_tests {
    use super::*;

    /// Seen straight from above: every axis of a square is across the view
    /// or toward the camera, so each square sits on the positive side.
    const SQUARES: PlaneSquares = PlaneSquares {
        offset: 0.3,
        side: 0.2,
        margin: 0.03,
        view_dir: Vec3::new(0.0, 0.0, -1.0),
    };

    fn down_at(x: f32, y: f32) -> Ray {
        Ray {
            origin: Vec3::new(x, y, 5.0),
            direction: -Vec3::Z,
        }
    }

    #[test]
    fn the_xy_square_grabs_inside_and_at_its_margin_only() {
        // From above, the XY square (normal Z) spans 0.2..0.4 on X and Y.
        let hit = plane_hit_test(&down_at(0.3, 0.3), Vec3::ZERO, SQUARES);
        assert_eq!(hit.map(|(a, _)| a), Some(Axis::Z));
        assert!((hit.unwrap().1 - 5.0).abs() < 1e-5);
        assert!(plane_hit_test(&down_at(0.42, 0.25), Vec3::ZERO, SQUARES).is_some());
        assert!(plane_hit_test(&down_at(0.45, 0.25), Vec3::ZERO, SQUARES).is_none());
        // The other quadrants hold no square.
        assert!(plane_hit_test(&down_at(-0.3, 0.3), Vec3::ZERO, SQUARES).is_none());
        // Seen from above, the XZ and YZ squares are edge-on: not grabbed.
        assert!(plane_hit_test(&down_at(0.3, 0.0), Vec3::ZERO, SQUARES).is_none());
    }

    #[test]
    fn each_square_faces_the_camera() {
        // From -X, -Y and above, looking toward +X, +Y and down: every
        // square on the negative side of X and Y, and on +Z.
        let view = Vec3::new(0.5, 0.6, -0.62).normalize();
        assert_eq!(plane_quadrant(Axis::Z, view), (-1.0, -1.0));
        assert_eq!(plane_quadrant(Axis::Y, view), (-1.0, 1.0));
        assert_eq!(plane_quadrant(Axis::X, view), (-1.0, 1.0));
        // A preset view keeps the positive side of the axes across it.
        assert_eq!(plane_quadrant(Axis::Z, -Vec3::Z), (1.0, 1.0));
        let top = Vec3::new(0.0, 1e-3, -1.0).normalize();
        assert_eq!(plane_quadrant(Axis::Z, top), (1.0, 1.0));
        let squares = PlaneSquares {
            view_dir: view,
            ..SQUARES
        };
        let ray = Ray {
            origin: Vec3::new(-0.3, -0.3, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(
            plane_hit_test(&ray, Vec3::ZERO, squares).map(|(a, _)| a),
            Some(Axis::Z)
        );
    }

    #[test]
    fn a_square_seen_almost_edge_on_is_not_grabbed() {
        // 5° above the plate, aimed at the middle of the XY square.
        let a = 5f32.to_radians();
        let dir = Vec3::new(-a.cos(), 0.0, -a.sin());
        let target = Vec3::new(0.3, 0.3, 0.0);
        let ray = Ray {
            origin: target - dir * 5.0,
            direction: dir,
        };
        assert_ne!(
            plane_hit_test(&ray, Vec3::ZERO, SQUARES).map(|(a, _)| a),
            Some(Axis::Z)
        );
        assert!(!plane_pickable(&ray, Axis::Z));
    }

    #[test]
    fn the_squares_follow_the_pivot() {
        let pivot = Vec3::new(10.0, 20.0, 30.0);
        let ray = Ray {
            origin: pivot + Vec3::new(0.3, 5.0, 0.3),
            direction: -Vec3::Y,
        };
        assert_eq!(
            plane_hit_test(&ray, pivot, SQUARES).map(|(a, _)| a),
            Some(Axis::Y)
        );
    }
}
