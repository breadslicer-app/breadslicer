/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure-CPU helpers for the translate gizmo: pivot, world-aligned
//! axes, screen-constant scale, ray-vs-axis-handle hit test, and the
//! drag-time closest-point math. Renderer-agnostic so it can be
//! unit-tested without a `wgpu::Device`.

use glam::Vec3;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::scene::{Scene, SceneObjectContent, Transform3D};

use crate::camera::OrbitCamera;
use crate::picking::Ray;

/// World-aligned gizmo axis. The X/Y/Z directions are global +X, +Y,
/// +Z; a local-axis option is left as a future toggle.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const fn world_dir(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

pub const ALL_AXES: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

/// Screen-pixel length of the gizmo shaft. Drives `gizmo_scale`.
pub const HANDLE_PIXEL_LENGTH: f32 = 80.0;

/// Screen-pixel radius of the cylinder hit volume. The visual shaft
/// is thinner than this — the hit cylinder is fattened so a click
/// near (but not on) the shaft still registers.
pub const HANDLE_PIXEL_RADIUS: f32 = 6.0;

/// Cosine of the lock-out angle: when
/// `|axis_world · ray_dir| > LOCKOUT_COS`, the angle between the
/// camera ray and the axis is below 8° and the closest-point math
/// becomes unstable — `1 − (axis · ray_dir)²` drops below `sin²(8°)`
/// and a one-pixel cursor jitter would translate the object far more
/// than the user expects. Such an axis is dropped from the hit test
/// and rendered greyed so the user sees why it is unresponsive.
///
/// `cos(8°) ≈ 0.99027`.
const LOCKOUT_COS: f32 = 0.990_268_07;

/// Cosine of the rotate-arc lock-out's complementary threshold:
/// when `|axis_world · ray_dir| < ROTATE_LOCKOUT_COS`, the camera ray
/// is within 8° of the arc's plane. The ray-vs-arc-plane intersection
/// then becomes ill-defined and the angular position around the axis
/// jumps under one-pixel cursor jitter. Threshold magnitude matches
/// `LOCKOUT_COS` (8° margin); the geometric configuration is the
/// complementary one (axis perpendicular to ray instead of parallel).
///
/// `cos(82°) ≈ 0.13917`.
#[allow(dead_code)]
const ROTATE_LOCKOUT_COS: f32 = 0.139_173_1;

/// Half-extent of the rotate arc's azimuthal span around its
/// camera-facing midpoint. The arc subtends a full 90° centred on the
/// projection of the eye-to-pivot direction onto the axis-perpendicular
/// plane, so the hit test accepts any in-plane angle within ±π/4 of
/// that midpoint.
#[allow(dead_code)]
const ROTATE_ARC_HALF_SPAN: f32 = std::f32::consts::FRAC_PI_4;

/// World-space pivot for the gizmo: the local-AABB centre of the
/// object, transformed by the object's `Transform3D`. Returns `None`
/// when the object has no vertices, an out-of-bounds resource
/// reference, or `idx` is out of range — all of which render the
/// gizmo unattachable.
pub fn pivot_for_object(scene: &Scene, idx: usize) -> Option<Vec3> {
    let object = scene.objects.get(idx)?;
    let mesh = match &object.content {
        SceneObjectContent::Mesh(m) => m,
        SceneObjectContent::Resource(r) => scene.resources.get(*r)?,
    };
    let (lmin, lmax) = local_aabb_centre(mesh)?;
    let local_centre = (lmin + lmax) * 0.5;
    Some(match &object.transform {
        Transform3D::Identity => local_centre,
        Transform3D::Affine(a) => {
            let v = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([
                local_centre.x,
                local_centre.y,
                local_centre.z,
            ]));
            Vec3::from(v.0)
        }
    })
}

fn local_aabb_centre(mesh: &IndexedMesh) -> Option<(Vec3, Vec3)> {
    let mut iter = mesh.vertices.iter();
    let first = Vec3::from(iter.next()?.0);
    let mut min = first;
    let mut max = first;
    for v in iter {
        let v = Vec3::from(v.0);
        min = min.min(v);
        max = max.max(v);
    }
    Some((min, max))
}

/// World-units-per-pixel × the handle's screen-pixel length, so that
/// every handle subtends `HANDLE_PIXEL_LENGTH` pixels regardless of
/// camera distance. Uses the same `world_per_pixel` formula as the
/// camera's pan code path.
pub fn gizmo_scale(camera: &OrbitCamera, fov_y: f32, viewport_h: f32) -> f32 {
    let h = viewport_h.max(1.0);
    let world_per_pixel = 2.0 * camera.distance * (fov_y * 0.5).tan() / h;
    world_per_pixel * HANDLE_PIXEL_LENGTH
}

/// World-unit radius of the hit cylinder, derived the same way as
/// `gizmo_scale` so the hit volume keeps its ~6 px screen radius
/// regardless of camera distance.
pub fn hit_radius_world(camera: &OrbitCamera, fov_y: f32, viewport_h: f32) -> f32 {
    let h = viewport_h.max(1.0);
    let world_per_pixel = 2.0 * camera.distance * (fov_y * 0.5).tan() / h;
    world_per_pixel * HANDLE_PIXEL_RADIUS
}

/// Whether an axis is grabbable. Returns false when the camera is
/// looking too closely along the axis (see `LOCKOUT_COS`). The
/// renderer should also grey the axis tint when this returns false
/// so the user sees that the lock-out is intentional.
pub fn axis_pickable(ray: &Ray, axis: Axis) -> bool {
    let a = axis.world_dir();
    a.dot(ray.direction).abs() < LOCKOUT_COS
}

/// Hit-test the three world-aligned axis handles against `ray`.
/// Returns the front-most pickable axis (smallest positive `t` on
/// the ray) whose hit cylinder the ray intersects, or `None`.
///
/// `pivot` is the world-space gizmo origin; `length_world` is the
/// shaft length in world units (typically `gizmo_scale(...)`); the
/// shaft is finite from `pivot` to `pivot + axis_dir * length_world`,
/// extended slightly past the tip to fold in the visible cone cap.
pub fn hit_test(ray: &Ray, pivot: Vec3, length_world: f32, radius_world: f32) -> Option<Axis> {
    // Extend past the visual tip by 25% so the cone cap counts as a
    // shaft hit without needing a separate ray-vs-cone code path.
    let shaft_total = length_world * 1.25;
    let mut best: Option<(Axis, f32)> = None;
    for &axis in &ALL_AXES {
        if !axis_pickable(ray, axis) {
            continue;
        }
        if let Some(t) = ray_hits_axis_handle(ray, pivot, axis, shaft_total, radius_world) {
            match best {
                Some((_, prev_t)) if prev_t <= t => {}
                _ => best = Some((axis, t)),
            }
        }
    }
    best.map(|(a, _)| a)
}

/// Ray-vs-finite-cylinder for a single axis handle. The cylinder
/// extends from `pivot` (parameter 0 along the axis) to
/// `pivot + axis_dir * length_world` (parameter `length_world`).
/// Returns the smallest positive `t` on the ray at which it enters
/// the cylinder, or `None` if it misses or the entry point is past
/// the shaft's end / before its start.
pub fn ray_hits_axis_handle(
    ray: &Ray,
    pivot: Vec3,
    axis: Axis,
    length_world: f32,
    radius_world: f32,
) -> Option<f32> {
    let a = axis.world_dir();
    let oc = ray.origin - pivot;

    // Cylinder axis intersection in the standard form: solve
    // `(P + t·D − pivot − s·A)² = r²` for the smallest positive `t`.
    // Rearranged into a quadratic in `t` with the s-component
    // eliminated by projecting onto the plane perpendicular to A.
    let d_dot_a = ray.direction.dot(a);
    let oc_dot_a = oc.dot(a);

    let dp = ray.direction - a * d_dot_a; // perpendicular component
    let ocp = oc - a * oc_dot_a;

    let qa = dp.dot(dp);
    let qb = 2.0 * dp.dot(ocp);
    let qc = ocp.dot(ocp) - radius_world * radius_world;

    if qa.abs() < 1e-10 {
        // Ray runs exactly along the axis — handled by the lock-out
        // upstream, but defensively bail.
        return None;
    }

    let disc = qb * qb - 4.0 * qa * qc;
    if disc < 0.0 {
        return None;
    }
    let sqrt_disc = disc.sqrt();
    let inv_2qa = 1.0 / (2.0 * qa);

    // Two ray-cylinder intersection candidates; we want the smaller
    // positive one (front face of the cylinder).
    let t0 = (-qb - sqrt_disc) * inv_2qa;
    let t1 = (-qb + sqrt_disc) * inv_2qa;

    let mut chosen: Option<f32> = None;
    for &t in &[t0, t1] {
        if t <= 0.0 {
            continue;
        }
        // Verify the entry point sits within the finite shaft span.
        let s = oc_dot_a + t * d_dot_a;
        if s < 0.0 || s > length_world {
            continue;
        }
        chosen = Some(match chosen {
            Some(prev) if prev <= t => prev,
            _ => t,
        });
        break;
    }
    chosen
}

/// Closest point on the infinite world-space line `pivot + t * axis`
/// to the world-space ray. Used during a drag to convert the cursor
/// position into the pinned-to-axis world point.
///
/// Stable as long as `axis_pickable(ray, axis)` is true at gesture
/// start; if the camera rotates into the unstable region mid-drag,
/// the drag continues using the same (now-noisier) formula — we
/// don't switch policies inside a gesture, since a mid-drag jump in
/// world delta would be more confusing than a few noisy frames.
pub fn closest_point_on_axis(ray: &Ray, pivot: Vec3, axis: Axis) -> Vec3 {
    let a = axis.world_dir();
    let d = ray.direction;
    let oc = pivot - ray.origin;

    let d_dot_a = d.dot(a);
    let oc_dot_a = oc.dot(a);
    let oc_dot_d = oc.dot(d);

    let denom = 1.0 - d_dot_a * d_dot_a;
    // Caller is responsible for ensuring denom is non-tiny via the
    // `axis_pickable` lock-out at gesture start. Inside a
    // tolerated-but-marginal drag, we clamp denom to a small positive
    // floor rather than dividing by zero — keeps the formula
    // well-defined even at the exact lock-out edge.
    let denom = denom.max(1e-6);

    // Derivation: with `q = pivot − O`, the perpendicularity
    // conditions `(δ·D) = 0` and `(δ·a) = 0` solve to
    //   `t · (1 − (a·D)²) = q·D · (a·D) − q·a`.
    let t_axis = (oc_dot_d * d_dot_a - oc_dot_a) / denom;
    pivot + a * t_axis
}

/// Whether a rotate arc is grabbable. Returns `false` when the camera
/// ray sits within 8° of the arc's plane: `|axis · ray.direction|`
/// drops below `cos(82°)` and the perpendicular-plane intersection
/// — the basis of both hit-test and drag math — degenerates. The
/// renderer should grey the arc in that state so the user sees why
/// it is unresponsive.
#[allow(dead_code)]
pub fn arc_pickable(ray: &Ray, axis: Axis) -> bool {
    let a = axis.world_dir();
    a.dot(ray.direction).abs() >= ROTATE_LOCKOUT_COS
}

/// In-plane orthonormal basis around `axis`, right-handed about the
/// axis (`axis × u = v`). Used by every rotate primitive so the
/// angular position returned by [`axis_plane_angle`] grows CCW about
/// `+axis` (right-hand rule). Caller code that wants the user's
/// "screen clockwise = +rotation" convention flips the sign at
/// composition time, where the camera direction is also known.
#[allow(dead_code)]
fn axis_basis(axis: Axis) -> (Vec3, Vec3) {
    match axis {
        Axis::X => (Vec3::Y, Vec3::Z),
        Axis::Y => (Vec3::Z, Vec3::X),
        Axis::Z => (Vec3::X, Vec3::Y),
    }
}

/// Intersects `ray` with the plane through `pivot` perpendicular to
/// `axis`. Returns the world-space hit, or `None` if the ray is
/// parallel to the plane or hits behind the camera. The rotate
/// lock-out (`arc_pickable`) is what keeps `denom` away from zero
/// in the steady state; the small floor here is a defensive guard
/// for callers that bypass the lock-out (e.g. tests).
#[allow(dead_code)]
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
#[allow(dead_code)]
pub fn axis_plane_angle(point: Vec3, pivot: Vec3, axis: Axis) -> f32 {
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
/// [`arc_pickable`] is supposed to forbid; the controller is
/// expected to gate this call on that lock-out at gesture start.
#[allow(dead_code)]
pub fn rotate_drag_angle(ray: &Ray, pivot: Vec3, axis: Axis) -> Option<f32> {
    let hit = ray_hits_axis_plane(ray, pivot, axis)?;
    Some(axis_plane_angle(hit, pivot, axis))
}

/// Approximate ray-vs-90°-arc-of-torus, the rotate gizmo's hit test.
///
/// The arc is modelled as the locus of points on the plane through
/// `pivot` perpendicular to `axis`, at distance `arc_radius_world`
/// from `pivot`, restricted to the 90° azimuthal quadrant centred
/// on the camera-facing direction. The ray's perpendicular-plane
/// intersection is accepted iff the in-plane radius lies in
/// `[arc_radius − tube_radius, arc_radius + tube_radius]` *and* the
/// in-plane angle lies within `±π/4` of the camera-direction angle.
///
/// The full ray-vs-torus would also account for the tube's extent
/// *along* the axis; for a pixel-radius tube that thickness is
/// invisible and the simpler test is enough. The camera direction
/// is read from `ray.origin`, which `picking::ray_from_pixel` sets
/// to `camera.eye()`.
#[allow(dead_code)]
pub fn ray_hits_torus_arc(
    ray: &Ray,
    pivot: Vec3,
    axis: Axis,
    arc_radius_world: f32,
    tube_radius_world: f32,
) -> Option<f32> {
    let hit = ray_hits_axis_plane(ray, pivot, axis)?;
    let p = hit - pivot;
    let r = p.length();
    if (r - arc_radius_world).abs() > tube_radius_world {
        return None;
    }

    let a = axis.world_dir();
    let to_camera = ray.origin - pivot;
    let to_camera_in_plane = to_camera - a * to_camera.dot(a);
    if to_camera_in_plane.length() < 1e-6 {
        // Camera sits on the axis itself — `arc_pickable` is
        // expected to have rejected this configuration upstream.
        return None;
    }

    let (u, v) = axis_basis(axis);
    let camera_angle = to_camera_in_plane.dot(v).atan2(to_camera_in_plane.dot(u));
    let hit_angle = p.dot(v).atan2(p.dot(u));
    let two_pi = std::f32::consts::TAU;
    let pi = std::f32::consts::PI;
    let diff = (hit_angle - camera_angle + pi).rem_euclid(two_pi) - pi;
    if diff.abs() > ROTATE_ARC_HALF_SPAN {
        return None;
    }
    let t = (hit - ray.origin).dot(ray.direction);
    Some(t)
}

/// Hit-test the three world-aligned rotate arcs against `ray`.
/// Returns the front-most pickable arc (smallest positive `t`), or
/// `None`. Mirrors `hit_test`'s structure for the translate cones,
/// using `arc_pickable` as the lock-out and `ray_hits_torus_arc` as
/// the per-axis test. Callers compute `arc_radius_world` and
/// `tube_radius_world` from the per-frame gizmo scale — a typical
/// choice is `arc_radius_world = gizmo_scale * 0.85`, keeping the
/// arc inside the translate cone tip without occluding it.
#[allow(dead_code)]
pub fn rotate_hit_test(
    ray: &Ray,
    pivot: Vec3,
    arc_radius_world: f32,
    tube_radius_world: f32,
) -> Option<Axis> {
    let mut best: Option<(Axis, f32)> = None;
    for &axis in &ALL_AXES {
        if !arc_pickable(ray, axis) {
            continue;
        }
        if let Some(t) = ray_hits_torus_arc(ray, pivot, axis, arc_radius_world, tube_radius_world) {
            match best {
                Some((_, prev_t)) if prev_t <= t => {}
                _ => best = Some((axis, t)),
            }
        }
    }
    best.map(|(a, _)| a)
}

#[cfg(test)]
mod axis_tests {
    use super::*;

    #[test]
    fn world_dir_returns_basis_vectors() {
        assert_eq!(Axis::X.world_dir(), Vec3::X);
        assert_eq!(Axis::Y.world_dir(), Vec3::Y);
        assert_eq!(Axis::Z.world_dir(), Vec3::Z);
    }
}

#[cfg(test)]
mod pivot_tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;

    fn triangle_mesh(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new(v0), Vertex::new(v1), Vertex::new(v2)],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 1, 2],
            }],
        }
    }

    #[test]
    fn pivot_under_identity_is_local_aabb_centre() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(triangle_mesh(
                    [-1.0, -2.0, -3.0],
                    [3.0, 0.0, 0.0],
                    [-1.0, 4.0, 5.0],
                )),
            }],
        };
        let p = pivot_for_object(&scene, 0).unwrap();
        // local AABB = ([-1, -2, -3], [3, 4, 5]) → centre (1, 1, 1)
        assert_approx_eq!(f32, p.x, 1.0);
        assert_approx_eq!(f32, p.y, 1.0);
        assert_approx_eq!(f32, p.z, 1.0);
    }

    #[test]
    fn pivot_under_translation_shifts_with_object() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Affine(AffineTransform3D::from_translation([
                    10.0, 20.0, 30.0,
                ])),
                content: SceneObjectContent::Mesh(triangle_mesh(
                    [-1.0, -2.0, -3.0],
                    [3.0, 0.0, 0.0],
                    [-1.0, 4.0, 5.0],
                )),
            }],
        };
        let p = pivot_for_object(&scene, 0).unwrap();
        // Local centre (1, 1, 1) translated by (10, 20, 30).
        assert_approx_eq!(f32, p.x, 11.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.y, 21.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.z, 31.0, epsilon = 1e-5);
    }

    #[test]
    fn pivot_for_empty_mesh_is_none() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(IndexedMesh {
                    meta: None,
                    vertices: vec![],
                    faces: vec![],
                }),
            }],
        };
        assert!(pivot_for_object(&scene, 0).is_none());
    }

    #[test]
    fn pivot_for_out_of_range_index_is_none() {
        let scene = Scene::default();
        assert!(pivot_for_object(&scene, 99).is_none());
    }

    #[test]
    fn pivot_for_dangling_resource_is_none() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Resource(7),
            }],
        };
        assert!(pivot_for_object(&scene, 0).is_none());
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    fn cam_at_distance(d: f32) -> OrbitCamera {
        let mut c = OrbitCamera::new(Vec3::ZERO, 1.0);
        c.distance = d;
        c
    }

    #[test]
    fn gizmo_scale_grows_with_camera_distance() {
        let near = gizmo_scale(&cam_at_distance(2.0), 1.0, 480.0);
        let far = gizmo_scale(&cam_at_distance(20.0), 1.0, 480.0);
        // 10× farther → 10× larger world units per pixel → 10× scale.
        assert!(far > near * 9.0 && far < near * 11.0);
    }

    #[test]
    fn gizmo_scale_shrinks_with_viewport_height() {
        let small_h = gizmo_scale(&cam_at_distance(5.0), 1.0, 240.0);
        let big_h = gizmo_scale(&cam_at_distance(5.0), 1.0, 1080.0);
        assert!(big_h < small_h);
    }
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
        // Ray pointing along -X is just as locked-out as +X — the
        // dot-product magnitude check is sign-agnostic.
        assert!(!axis_pickable(&ray_along(-Vec3::X), Axis::X));
    }
}

#[cfg(test)]
mod ray_hits_axis_handle_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn perpendicular_ray_hits_x_handle_centre() {
        // Pivot at origin, X handle along +X, ray crosses (+0.5, 0, 0).
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let t = ray_hits_axis_handle(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05).unwrap();
        // Ray enters cylinder at z = +0.05 (radius), so t = 5 - 0.05.
        assert_approx_eq!(f32, t, 5.0 - 0.05, epsilon = 1e-4);
    }

    #[test]
    fn ray_outside_radius_misses() {
        let ray = Ray {
            origin: Vec3::new(0.5, 1.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_handle(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_past_shaft_end_misses() {
        // Ray hits the X axis at x = 5 (well past length 1).
        let ray = Ray {
            origin: Vec3::new(5.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_handle(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_before_pivot_along_axis_misses() {
        // Ray crosses x = -2 (before pivot at origin).
        let ray = Ray {
            origin: Vec3::new(-2.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_axis_handle(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_pointing_away_from_handle_misses() {
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: Vec3::Z, // away from origin
        };
        assert!(ray_hits_axis_handle(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05).is_none());
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
        let ray = Ray {
            origin: Vec3::new(0.5, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(hit_test(&ray, Vec3::ZERO, 1.0, 0.05), Some(Axis::X));
    }

    #[test]
    fn frontmost_handle_wins_when_screen_overlap() {
        // Two handles at the same world position is impossible
        // (they're orthogonal), but two rays converging on a corner
        // can hit two cylinders. Verify the closer one wins.
        // X handle from origin to +X; Y handle from origin to +Y.
        // A ray that enters the X handle's cylinder before the Y
        // handle's cylinder picks X.
        let ray = Ray {
            origin: Vec3::new(0.5, 0.05, 5.0),
            direction: -Vec3::Z,
        };
        // Both handles are roughly along the ray's path; the X
        // cylinder's centre line runs through (0.5, 0, *) so the
        // ray enters at z ≈ +0.05 - 0.05√(1−0.05²/0.05²) which is
        // simply z ≈ 0 for this configuration. The Y cylinder runs
        // through (0, 0.05, *) — the ray is offset 0.5 in x, well
        // outside the 0.05 radius, so the Y handle misses.
        // This reduces to "X wins, Y misses" — still a valid
        // front-most-wins assertion.
        assert_eq!(hit_test(&ray, Vec3::ZERO, 1.0, 0.05), Some(Axis::X));
    }

    #[test]
    fn locked_out_axis_is_not_picked() {
        // Ray pointing nearly along +X — X is locked out by the
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
        // depending on the geometry — the only contract is X is
        // forbidden.
        let picked = hit_test(&ray, Vec3::ZERO, 1.0, 0.05);
        assert_ne!(picked, Some(Axis::X));
    }
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
        let p = closest_point_on_axis(&ray, Vec3::ZERO, Axis::X);
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
        let p = closest_point_on_axis(&ray, Vec3::ZERO, Axis::X);
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
        let p0 = closest_point_on_axis(&r0, pivot, Axis::X);
        let p1 = closest_point_on_axis(&r1, pivot, Axis::X);
        let delta = p1 - p0;
        assert_approx_eq!(f32, delta.x, 2.5, epsilon = 1e-5);
        assert_approx_eq!(f32, delta.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, delta.z, 0.0, epsilon = 1e-5);
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
        // exactly in that plane — the arc is edge-on.
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
        // Ray along -X is just as pickable as +X — the
        // dot-product magnitude is sign-agnostic.
        assert!(arc_pickable(&ray_along(-Vec3::X), Axis::X));
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
        // and the ray hits the YZ plane at (0, 1, 0) — on the +u (Y)
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
mod ray_hits_torus_arc_tests {
    use super::*;

    #[test]
    fn ray_through_arc_midpoint_hits() {
        // X-axis arc: midpoint faces the camera. Camera at (5, 0, 0)
        // looking at the origin → eye-to-pivot in YZ plane is the
        // zero vector projection (camera is on-axis), which the
        // hit-test rejects. Use an off-axis camera instead.
        // Place camera at (5, 0.1, 0); the in-plane camera direction
        // is along +Y. Arc circle radius 1, ray crosses (0, 1, 0).
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        // Sanity: this ray hits the YZ plane at some t > 0 near (0, 1, 0).
        let result = ray_hits_torus_arc(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05);
        assert!(
            result.is_some(),
            "ray aimed at the X arc's midpoint should hit"
        );
    }

    #[test]
    fn ray_outside_radius_band_misses() {
        // Camera looking down +X at the YZ plane through origin; ray
        // crosses (0, 0.3, 0) — well inside the arc circle (radius 1)
        // and outside the tube's tolerance.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.3, 0.0),
            direction: -Vec3::X,
        };
        assert!(ray_hits_torus_arc(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_in_radius_band_but_outside_arc_quadrant_misses() {
        // Camera-facing side of the X arc is on +Y (when camera is
        // off-axis at (5, 0.1, 0)). A ray crossing (0, -1, 0) — the
        // *back* side, 180° away — should miss because it's outside
        // the 90° quadrant.
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, -1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert!(ray_hits_torus_arc(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_parallel_to_arc_plane_misses() {
        // Ray entirely in the YZ plane — no perpendicular-plane
        // intersection.
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_torus_arc(&ray, Vec3::ZERO, Axis::X, 1.0, 0.05).is_none());
    }
}

#[cfg(test)]
mod rotate_hit_test_tests {
    use super::*;

    #[test]
    fn empty_space_returns_none() {
        // Ray pointing nowhere near any arc.
        let ray = Ray {
            origin: Vec3::new(100.0, 100.0, 100.0),
            direction: Vec3::new(1.0, 0.0, 0.0),
        };
        assert!(rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.05).is_none());
    }

    #[test]
    fn ray_through_x_arc_picks_x() {
        let ray = Ray {
            origin: Vec3::new(5.0, 0.1, 0.0),
            direction: (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(5.0, 0.1, 0.0)).normalize(),
        };
        assert_eq!(rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.05), Some(Axis::X));
    }

    #[test]
    fn locked_out_axis_is_skipped() {
        // X arc is in the YZ plane; ray sits inside that plane → the
        // X arc must be locked out and skipped. Y/Z arcs may or may
        // not be hit; the only contract is X is forbidden.
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, 0.0),
            direction: -Vec3::Y,
        };
        assert_ne!(rotate_hit_test(&ray, Vec3::ZERO, 1.0, 0.05), Some(Axis::X));
    }
}
