/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! CPU ray-pick from a logical viewport pixel to a `Scene` leaf identifier.
//!
//! 3-stage strategy:
//!
//! 1. **World AABB pre-pass.** Each object's world-space AABB (Arvo's method on
//!    the local AABB and the object's `Transform3D`) is slab-tested against the
//!    ray. Objects whose AABB the ray misses are eliminated without ever
//!    touching their triangles.
//!
//! 2. **Object-space cluster traversal.** Survivors invert the ray once into
//!    object space, then slab-test each
//!    [`TriangleClusters`](sc_mesh_spatial::TriangleClusters) AABB against the
//!    local ray. Clusters the ray misses skip the linear Möller-Trumbore (MT)
//!    walk entirely.
//!
//! 3. **Sign-aware Möller-Trumbore on cluster survivors.** Faces in surviving
//!    clusters are tested in object space. The renderer's
//!    `cull_mode: Some(Face::Back)` with `front_face: FrontFace::Ccw` is
//!    matched by accepting hits whose MT determinant has the same sign as the
//!    linear part's determinant: a transform with `det < 0` reverses
//!    world-space winding, so the object-space front face is the world-space
//!    back face, and vice versa.
//!
//! Iterating in object space needs 1 ray inversion per object instead of 3
//! `Mat4 * Vec3` transforms per triangle. The cluster slab tests then reduce
//! the linear MT walk to about `O(F / cluster_size)` AABB tests, for F faces,
//! plus `O(cluster_size)` MT tests per hit cluster.

use glam::{Vec3, Vec4};
use sc_mesh_core::Aabb;
use sc_mesh_core::aabb::arvo_transformed_aabb;
use sc_mesh_core::geometry::DEGENERACY_SIN_SQ_THRESHOLD;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::{ObjectQuery, SceneSpatial};
use sc_scene::NodeId;

use crate::camera::OrbitCamera;

/// Minimum `t` for a hit in front of the ray origin, checked at the end of
/// Möller-Trumbore; it rejects self-intersections at `t` close to 0.
/// `local_ray_for_object` also uses it as the minimum length of the
/// object-space ray direction.
const EPSILON: f32 = 1e-7;

/// Scale-invariant threshold on `(det / (|edge1| · |edge2|))²` for the
/// Möller-Trumbore front-face and degeneracy gate. The picker's determinant
/// `det = edge1 · (direction × edge2)` uses a unit `direction`, so
/// `|det| <= |edge1| · |edge2|` and the squared ratio, which has no unit,
/// cannot exceed 1. Sharing mesh-core's `DEGENERACY_SIN_SQ_THRESHOLD` makes the
/// gate independent of mesh scale: `|det|` shrinks with the square of the edge
/// length, so an absolute floor on `det` rejects valid front-face hits on small
/// triangles, for example a model exported in metres but loaded as millimetres.
/// f32 form because the picker's MT determinant and edge lengths are f32.
const DET_SIN_SQ_THRESHOLD: f32 = DEGENERACY_SIN_SQ_THRESHOLD as f32;

/// Object-space ray plus the conversion factor needed to bring an MT `t` back
/// to world space. `det_sign` is `+1.0` when the linear part of the object's
/// world transform has a positive determinant, `-1.0` when negative; it flips
/// the MT front-face filter for mirrored objects, so the picker agrees with the
/// renderer's CCW culling.
struct LocalRay {
    origin: Vec3,
    direction: Vec3,
    /// `t_world = t_local / world_to_local_scale`. `1.0` for the identity
    /// transform.
    world_to_local_scale: f32,
    det_sign: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Ray {
    pub origin: Vec3,
    /// Unit length: `ray_from_pixel` and `ray_hits_object_query` normalise it,
    /// and the scale-invariant triangle gate and `ray_z_plane_intersection`
    /// rely on it.
    pub direction: Vec3,
}

/// Builds a world-space ray from a logical-pixel cursor position.
///
/// `(x, y)` are in Slint's logical viewport coordinates: top-left origin, y
/// growing downward, *not* multiplied by `scale_factor`.
/// `(viewport_w, viewport_h)` use the same units. `fov_y` is the vertical FOV
/// in radians, matching the value the renderer feeds into
/// `Mat4::perspective_rh`.
pub(crate) fn ray_from_pixel(
    camera: &OrbitCamera,
    fov_y: f32,
    viewport_w: f32,
    viewport_h: f32,
    x: f32,
    y: f32,
) -> Ray {
    let (origin, direction) = camera.screen_ray(fov_y, viewport_w, viewport_h, x, y);
    Ray { origin, direction }
}

/// Projects a world-space point onto the viewport in logical-pixel coordinates
/// (top-left origin, y growing downward; same convention as `ray_from_pixel`'s
/// input). Returns `None` when the point is behind the camera.
pub(crate) fn project_world_to_screen(
    camera: &OrbitCamera,
    fov_y: f32,
    viewport_w: f32,
    viewport_h: f32,
    world_point: Vec3,
) -> Option<(f32, f32)> {
    let aspect = viewport_w / viewport_h.max(1.0);
    let view = camera.view_matrix();
    // The near and far depths are placeholders: they only affect z, which the
    // perspective divide below discards.
    let proj = camera.projection(fov_y, aspect, 0.01, 100.0);
    let clip = (proj * view) * Vec4::new(world_point.x, world_point.y, world_point.z, 1.0);
    if clip.w <= 0.0 || (world_point - camera.eye()).dot(camera.target - camera.eye()) <= 0.0 {
        return None;
    }
    let ndc_x = clip.x / clip.w;
    let ndc_y = clip.y / clip.w;
    let screen_x = (ndc_x + 1.0) * 0.5 * viewport_w;
    // Slint-y grows downward and NDC-y grows upward, so y flips.
    let screen_y = (1.0 - ndc_y) * 0.5 * viewport_h;
    Some((screen_x, screen_y))
}

/// Intersects a ray with the axis-aligned plane `Z = z` and returns the
/// `(x, y)` world coordinates of the hit. `None` when the ray is nearly
/// parallel to the plane or when the plane sits behind the ray origin
/// (intersection at `t < 0`, which the paste path treats as "the cursor is
/// pointed away from the build platform").
pub(crate) fn ray_z_plane_intersection(ray: &Ray, z: f32) -> Option<(f32, f32)> {
    // `direction` is unit length, so this guard does not depend on scale: a
    // `direction.z` below 1e-7 means the ray is nearly parallel to the plane
    // and the hit is too far away to be a useful placement anchor.
    const EPSILON_DIR_Z: f32 = 1e-7;
    if ray.direction.z.abs() < EPSILON_DIR_Z {
        return None;
    }
    let t = (z - ray.origin.z) / ray.direction.z;
    if t < 0.0 {
        return None;
    }
    let hit = ray.origin + ray.direction * t;
    Some((hit.x, hit.y))
}

/// Picks the closest leaf the ray hits, by stable [`NodeId`]. Returns `None` if
/// every reachable leaf misses (whether by AABB pre-pass or triangle test).
/// Leaves are visited in the pre-order of [`SceneSpatial::leaf_id_at_index`];
/// on equal `t` the first leaf visited wins.
pub(crate) fn pick_object(ray: &Ray, spatial: &SceneSpatial) -> Option<NodeId> {
    pick_object_where(ray, spatial, |_| true).map(|(node_id, _)| node_id)
}

/// [`pick_object`] over the leaves `keep` accepts, with the hit's ray
/// parameter `t`.
pub(crate) fn pick_object_where(
    ray: &Ray,
    spatial: &SceneSpatial,
    keep: impl Fn(NodeId) -> bool,
) -> Option<(NodeId, f32)> {
    let mut best: Option<(NodeId, f32)> = None;
    let mut idx = 0usize;
    while let Some(node_id) = spatial.leaf_id_at_index(idx) {
        idx += 1;
        if !keep(node_id) {
            continue;
        }
        let Some(query) = spatial.leaf(node_id) else {
            continue;
        };
        let Some(local) = query.local_aabb() else {
            continue;
        };
        let (min, max) = world_aabb_from_local(local, &query.world_transform());
        if !ray_hits_aabb(ray, min, max) {
            continue;
        }
        if let Some(t) = ray_hits_object_query(ray, &query) {
            match best {
                Some((_, prev_t)) if prev_t <= t => {}
                _ => best = Some((node_id, t)),
            }
        }
    }
    best
}

/// Tests the ray against an axis-aligned box with the slab method. A box
/// entirely behind the ray origin is a miss.
pub(crate) fn ray_hits_aabb(ray: &Ray, min: Vec3, max: Vec3) -> bool {
    ray_hits_aabb_t(ray, min, max).is_some()
}

/// Returns the ray parameter at which the ray enters the box (`t_enter`), from
/// the slab method, or `None` if the ray misses or the box is behind the
/// origin. Origin-inside-box returns 0 as the entry parameter. Used by the
/// gizmo cube hit tests to feed front-most-`t` arbitration alongside the
/// cylinder and arc tests.
pub(crate) fn ray_hits_aabb_t(ray: &Ray, min: Vec3, max: Vec3) -> Option<f32> {
    // Direction components of 0 give ±inf in `inv_dir`, which pass through the
    // min/max chain: the slab is a hit when the origin is inside it and a miss
    // when it is not.
    let inv_dir = Vec3::new(
        1.0 / ray.direction.x,
        1.0 / ray.direction.y,
        1.0 / ray.direction.z,
    );
    let t_low = (min - ray.origin) * inv_dir;
    let t_high = (max - ray.origin) * inv_dir;
    let t_enter = t_low.min(t_high).max_element();
    let t_exit = t_low.max(t_high).min_element();
    if t_exit < t_enter || t_exit < 0.0 {
        return None;
    }
    Some(t_enter.max(0.0))
}

/// Intersects the ray with a triangle by the sign-aware Möller-Trumbore method.
/// `det_sign = +1.0` is the classic "front face = CCW winding" rule that
/// matches the renderer's
/// `front_face: FrontFace::Ccw, cull_mode: Some(Face::Back)` pipeline state.
/// `det_sign = -1.0` flips the front-face filter, which is what the picker
/// needs for objects whose `Transform3D` has a negative-determinant linear
/// part: the world-space winding has been reversed, so the object-space
/// triangle's *back* face is the renderer's *front* face.
pub(crate) fn ray_hits_triangle_oriented(
    ray: &Ray,
    v0: Vec3,
    v1: Vec3,
    v2: Vec3,
    det_sign: f32,
) -> Option<f32> {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = ray.direction.cross(edge2);
    let det = edge1.dot(h);
    let det_oriented = det * det_sign;
    // The barycentric and `t > EPSILON` checks do not depend on the sign, so
    // only this filter uses `det_sign`.
    if det_oriented <= 0.0 {
        return None;
    }
    let edges_mag_sq = edge1.length_squared() * edge2.length_squared();
    if det_oriented * det_oriented < DET_SIN_SQ_THRESHOLD * edges_mag_sq {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = ray.origin - v0;
    let u = inv_det * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = inv_det * ray.direction.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = inv_det * edge2.dot(q);
    if t > EPSILON { Some(t) } else { None }
}

/// Returns the closest front-face hit `t` for the object behind `query`, in
/// world-space ray parameters, or `None` when the ray misses it. Malformed
/// objects (singular linear part on the object's transform, empty mesh, or
/// out-of-range face vertex indices) also give `None`: the picker treats them
/// as a miss and reports no error.
pub(crate) fn ray_hits_object_query(ray: &Ray, query: &ObjectQuery<'_>) -> Option<f32> {
    ray_hit_face(ray, query).map(|(t, _)| t)
}

/// The world normal of the face the ray meets first on `query`, turned to
/// face the ray (out of the surface it hits), with the hit's `t`.
pub(crate) fn ray_hit_normal(ray: &Ray, query: &ObjectQuery<'_>) -> Option<(f32, Vec3)> {
    let (t, face) = ray_hit_face(ray, query)?;
    let mesh = query.mesh();
    let world = query.world_transform();
    let [a, b, c] = mesh.faces().get(face)?.vertices.map(|v| {
        mesh.vertices()
            .get(usize::from(v))
            .map(|p| Vec3::from(world.apply_to_vertex(*p).coords()))
    });
    let (a, b, c) = (a?, b?, c?);
    let n = (b - a).cross(c - a).normalize_or_zero();
    if n == Vec3::ZERO {
        return None;
    }
    Some((t, if n.dot(ray.direction) > 0.0 { -n } else { n }))
}

/// [`ray_hits_object_query`] with the index of the face hit.
fn ray_hit_face(ray: &Ray, query: &ObjectQuery<'_>) -> Option<(f32, usize)> {
    let local = local_ray_for_object(ray, &query.world_transform())?;
    let local_ray = Ray {
        origin: local.origin,
        direction: local.direction,
    };
    let clusters = query.clusters()?;
    let mesh = query.mesh();
    let vertices = mesh.vertices();
    let faces = mesh.faces();
    let mut best: Option<(f32, usize)> = None;
    for (face_range, aabb) in clusters.iter() {
        let aabb_min = Vec3::from(aabb.min().coords());
        let aabb_max = Vec3::from(aabb.max().coords());
        if !ray_hits_aabb(&local_ray, aabb_min, aabb_max) {
            continue;
        }
        for (index, face) in faces[face_range.start as usize..face_range.end as usize]
            .iter()
            .enumerate()
        {
            let index = face_range.start as usize + index;
            let v0 = Vec3::from(vertices.get(usize::from(face.vertices[0]))?.coords());
            let v1 = Vec3::from(vertices.get(usize::from(face.vertices[1]))?.coords());
            let v2 = Vec3::from(vertices.get(usize::from(face.vertices[2]))?.coords());
            if let Some(t_local) =
                ray_hits_triangle_oriented(&local_ray, v0, v1, v2, local.det_sign)
            {
                let t = t_local / local.world_to_local_scale;
                best = Some(match best {
                    Some(prev) if prev.0 <= t => prev,
                    _ => (t, index),
                });
            }
        }
    }
    best
}

/// Builds the object-space form of `ray` for `transform`. Returns `None` when
/// the transform's linear part is singular; the picker treats that as a
/// malformed object and skips it. For an identity transform the local ray
/// equals the world ray, with `world_to_local_scale = 1` and `det_sign = +1`.
fn local_ray_for_object(ray: &Ray, transform: &Transform3D) -> Option<LocalRay> {
    match transform {
        Transform3D::Identity => Some(LocalRay {
            origin: ray.origin,
            direction: ray.direction,
            world_to_local_scale: 1.0,
            det_sign: 1.0,
        }),
        Transform3D::Affine(a) => {
            let inv = a.inverse().ok()?;
            let inv_l = inv.linear();
            let inv_t = inv.translation();
            let apply = |p: Vec3, with_translation: bool| -> Vec3 {
                let tx = if with_translation {
                    inv_t[0] as f32
                } else {
                    0.0
                };
                let ty = if with_translation {
                    inv_t[1] as f32
                } else {
                    0.0
                };
                let tz = if with_translation {
                    inv_t[2] as f32
                } else {
                    0.0
                };
                Vec3::new(
                    (inv_l[0][0] as f32) * p.x
                        + (inv_l[0][1] as f32) * p.y
                        + (inv_l[0][2] as f32) * p.z
                        + tx,
                    (inv_l[1][0] as f32) * p.x
                        + (inv_l[1][1] as f32) * p.y
                        + (inv_l[1][2] as f32) * p.z
                        + ty,
                    (inv_l[2][0] as f32) * p.x
                        + (inv_l[2][1] as f32) * p.y
                        + (inv_l[2][2] as f32) * p.z
                        + tz,
                )
            };
            let local_origin = apply(ray.origin, true);
            let local_dir_unnorm = apply(ray.direction, false);
            let scale = local_dir_unnorm.length();
            // A non-singular inverse cannot map a unit direction to zero, but
            // its f32 image can be tiny or non-finite, for example for an
            // object scaled up by more than 1e7. Such an object is treated as a
            // miss.
            if !scale.is_finite() || scale <= EPSILON {
                return None;
            }
            let local_dir_unit = local_dir_unnorm / scale;
            let l = a.linear();
            let det = (l[0][0] * (l[1][1] * l[2][2] - l[1][2] * l[2][1])
                - l[0][1] * (l[1][0] * l[2][2] - l[1][2] * l[2][0])
                + l[0][2] * (l[1][0] * l[2][1] - l[1][1] * l[2][0])) as f32;
            if det == 0.0 {
                return None;
            }
            Some(LocalRay {
                origin: local_origin,
                direction: local_dir_unit,
                world_to_local_scale: scale,
                det_sign: det.signum(),
            })
        }
    }
}

/// Applies Arvo's transformed-AABB formula to a cached object-space AABB and
/// the object's transform. Used by [`pick_object`]'s pre-pass to slab-test
/// world-space AABBs without re-walking vertex data.
fn world_aabb_from_local(local: Aabb, transform: &Transform3D) -> (Vec3, Vec3) {
    let lmin = Vec3::from(local.min().coords());
    let lmax = Vec3::from(local.max().coords());
    match transform {
        Transform3D::Identity => (lmin, lmax),
        Transform3D::Affine(a) => arvo(lmin, lmax, a),
    }
}

/// Applies Arvo's transformed-AABB formula in f32, delegating to mesh-core's
/// generic implementation. For picking, f32 is adequate: the AABB only gates
/// the per-triangle test, which itself is f32 throughout. `AffineTransform3D`
/// stores its linear part and translation as f64, so the f32 cast happens once
/// per call rather than per term.
fn arvo(lmin: Vec3, lmax: Vec3, a: &AffineTransform3D) -> (Vec3, Vec3) {
    let m = a.linear();
    let t = a.translation();
    let m_f32: [[f32; 3]; 3] = [
        [m[0][0] as f32, m[0][1] as f32, m[0][2] as f32],
        [m[1][0] as f32, m[1][1] as f32, m[1][2] as f32],
        [m[2][0] as f32, m[2][1] as f32, m[2][2] as f32],
    ];
    let t_f32 = [t[0] as f32, t[1] as f32, t[2] as f32];

    let (new_min, new_max) =
        arvo_transformed_aabb::<f32>(lmin.to_array(), lmax.to_array(), &m_f32, &t_f32);
    (Vec3::from(new_min), Vec3::from(new_max))
}

#[cfg(test)]
mod ray_from_pixel_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn camera_at_origin() -> OrbitCamera {
        // Target at origin, radius 1. The orbit camera defaults to a
        // non-axis-aligned view (yaw and pitch ≠ 0); fine for our checks that
        // use the camera's own basis.
        OrbitCamera::new(Vec3::ZERO, 1.0)
    }

    #[test]
    fn direction_is_unit_length() {
        let cam = camera_at_origin();
        let ray = ray_from_pixel(&cam, std::f32::consts::FRAC_PI_3, 200.0, 100.0, 37.5, 19.0);
        assert_approx_eq!(f32, ray.direction.length(), 1.0, epsilon = 1e-5);
    }

    #[test]
    fn center_pixel_aims_at_camera_forward() {
        let cam = camera_at_origin();
        let forward = (cam.target - cam.eye()).normalize();
        let ray = ray_from_pixel(&cam, std::f32::consts::FRAC_PI_3, 200.0, 100.0, 100.0, 50.0);
        let cos = ray.direction.dot(forward);
        assert_approx_eq!(f32, cos, 1.0, epsilon = 1e-5);
    }

    #[test]
    fn origin_is_camera_eye() {
        let cam = camera_at_origin();
        let ray = ray_from_pixel(&cam, std::f32::consts::FRAC_PI_3, 100.0, 100.0, 50.0, 50.0);
        let eye = cam.eye();
        assert_approx_eq!(f32, ray.origin.x, eye.x, epsilon = 1e-5);
        assert_approx_eq!(f32, ray.origin.y, eye.y, epsilon = 1e-5);
        assert_approx_eq!(f32, ray.origin.z, eye.z, epsilon = 1e-5);
    }

    #[test]
    fn corner_rays_respect_aspect_ratio() {
        // For a wide viewport, a horizontal-edge ray must subtend a wider angle
        // from forward than a vertical-edge ray of the same NDC magnitude.
        // Compare cos(angle); the wider angle has the smaller cos.
        let cam = camera_at_origin();
        let fov = std::f32::consts::FRAC_PI_3;
        let w = 200.0;
        let h = 100.0;
        let forward = (cam.target - cam.eye()).normalize();

        let right_edge = ray_from_pixel(&cam, fov, w, h, w - 0.5, h * 0.5);
        let top_edge = ray_from_pixel(&cam, fov, w, h, w * 0.5, 0.5);
        let cos_right = right_edge.direction.dot(forward);
        let cos_top = top_edge.direction.dot(forward);
        assert!(
            cos_right < cos_top,
            "horizontal-edge ray must be wider than vertical-edge ray for a 2:1 viewport \
             (cos_right={cos_right}, cos_top={cos_top})"
        );
    }
}

#[cfg(test)]
mod ray_hits_aabb_tests {
    use super::*;

    fn ray_along_x() -> Ray {
        Ray {
            origin: Vec3::new(-10.0, 0.0, 0.0),
            direction: Vec3::X,
        }
    }

    #[test]
    fn hits_box_in_path() {
        let r = ray_along_x();
        assert!(ray_hits_aabb(
            &r,
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0)
        ));
    }

    #[test]
    fn misses_box_off_axis() {
        let r = ray_along_x();
        assert!(!ray_hits_aabb(
            &r,
            Vec3::new(-1.0, 5.0, -1.0),
            Vec3::new(1.0, 7.0, 1.0)
        ));
    }

    #[test]
    fn misses_box_behind_origin() {
        let r = ray_along_x();
        // Box is at negative x, far behind the ray's origin.
        assert!(!ray_hits_aabb(
            &r,
            Vec3::new(-100.0, -1.0, -1.0),
            Vec3::new(-50.0, 1.0, 1.0)
        ));
    }

    #[test]
    fn hits_box_containing_origin() {
        let r = ray_along_x();
        assert!(ray_hits_aabb(
            &r,
            Vec3::new(-20.0, -1.0, -1.0),
            Vec3::new(20.0, 1.0, 1.0)
        ));
    }
}

#[cfg(test)]
mod ray_hits_triangle_oriented_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn ccw_triangle() -> (Vec3, Vec3, Vec3) {
        // CCW when viewed from +Z. Front face faces +Z.
        (
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
    }

    #[test]
    fn hits_canonical_triangle_at_expected_t() {
        let (v0, v1, v2) = ccw_triangle();
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let t = ray_hits_triangle_oriented(&ray, v0, v1, v2, 1.0).expect("ray should hit");
        assert_approx_eq!(f32, t, 5.0, epsilon = 1e-5);
    }

    #[test]
    fn rejects_back_face_hit() {
        // Same triangle, ray comes from -Z. Hits the back face.
        let (v0, v1, v2) = ccw_triangle();
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, -5.0),
            direction: Vec3::Z,
        };
        assert!(
            ray_hits_triangle_oriented(&ray, v0, v1, v2, 1.0).is_none(),
            "back-face hits must be rejected to match the renderer's cull mode"
        );
    }

    #[test]
    fn rejects_ray_pointing_away() {
        let (v0, v1, v2) = ccw_triangle();
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: Vec3::Z, // away from triangle
        };
        assert!(ray_hits_triangle_oriented(&ray, v0, v1, v2, 1.0).is_none());
    }

    #[test]
    fn rejects_parallel_ray() {
        let (v0, v1, v2) = ccw_triangle();
        // Parallel to the triangle's plane (z = 0) and offset by some y.
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, 5.0),
            direction: Vec3::X,
        };
        assert!(ray_hits_triangle_oriented(&ray, v0, v1, v2, 1.0).is_none());
    }

    #[test]
    fn rejects_ray_outside_triangle() {
        let (v0, v1, v2) = ccw_triangle();
        let ray = Ray {
            origin: Vec3::new(10.0, 10.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_triangle_oriented(&ray, v0, v1, v2, 1.0).is_none());
    }
}

#[cfg(test)]
mod pick_object_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_scene::{LeafPayload, Scene};

    fn unit_triangle_mesh() -> IndexedMesh {
        // CCW when viewed from +Z, ~unit-sized.
        IndexedMesh::new(
            None,
            vec![
                Vertex::new([-0.5, -0.5, 0.0]),
                Vertex::new([0.5, -0.5, 0.0]),
                Vertex::new([0.0, 0.5, 0.0]),
            ],
            vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        )
    }

    fn translated(translation: [f32; 3]) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation([
            translation[0] as f64,
            translation[1] as f64,
            translation[2] as f64,
        ]))
    }

    /// Returns a loader-faithful empty graph: a single identity-Group root that
    /// will host the leaves attached by helpers below.
    fn graph_with_group_root() -> (Scene, NodeId) {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    fn add_leaf_under(g: &mut Scene, parent: NodeId, mesh: IndexedMesh, transform: Transform3D) {
        let leaf = g
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("add leaf");
        g.add_child(parent, leaf).expect("attach leaf");
    }

    fn spatial_with_unit_triangles(translations: &[[f32; 3]]) -> SceneSpatial {
        let (mut g, root) = graph_with_group_root();
        for t in translations {
            add_leaf_under(&mut g, root, unit_triangle_mesh(), translated(*t));
        }
        SceneSpatial::from_graph(g)
    }

    #[test]
    fn ray_hits_object_uses_object_transform() {
        // Same triangle, 2 scenes: 1 identity, 1 translated by +X. Ray points
        // at the +X position; only the translated copy should be hit.
        let id_spatial = spatial_with_unit_triangles(&[[0.0, 0.0, 0.0]]);
        let shifted_spatial = spatial_with_unit_triangles(&[[10.0, 0.0, 0.0]]);
        // Ray goes through (10, 0, 0) toward -Z.
        let ray = Ray {
            origin: Vec3::new(10.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let id_leaf = id_spatial.leaf_id_at_index(0).unwrap();
        let shifted_leaf = shifted_spatial.leaf_id_at_index(0).unwrap();
        assert!(ray_hits_object_query(&ray, &id_spatial.leaf(id_leaf).unwrap()).is_none());
        assert!(
            ray_hits_object_query(&ray, &shifted_spatial.leaf(shifted_leaf).unwrap()).is_some()
        );
    }

    #[test]
    fn empty_scene_picks_nothing() {
        let spatial = SceneSpatial::from_graph(Scene::new());
        let ray = Ray {
            origin: Vec3::ZERO,
            direction: Vec3::X,
        };
        assert!(pick_object(&ray, &spatial).is_none());
    }

    #[test]
    fn returns_none_when_ray_misses_every_object() {
        let spatial = spatial_with_unit_triangles(&[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);
        // Ray starts at y = 100 and points along +Y, away from both triangles.
        let ray = Ray {
            origin: Vec3::new(0.0, 100.0, 5.0),
            direction: Vec3::Y,
        };
        assert!(pick_object(&ray, &spatial).is_none());
    }

    #[test]
    fn closest_hit_wins_among_overlapping_along_ray() {
        // 3 triangles along -Z at z = 0, z = -2, z = -4.
        let spatial =
            spatial_with_unit_triangles(&[[0.0, 0.0, 0.0], [0.0, 0.0, -2.0], [0.0, 0.0, -4.0]]);
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        // The z = 0 triangle (pre-order leaf 0) is closest.
        let closest = spatial.leaf_id_at_index(0).unwrap();
        assert_eq!(pick_object(&ray, &spatial), Some(closest));
    }

    #[test]
    fn aabb_pre_pass_skips_off_ray_leaves() {
        // 2 triangles: 1 in the ray's path, 1 off to the side (+Y by a wide
        // margin). The off-axis one's AABB is missed by the ray, so the picker
        // eliminates it without running its triangle test. Functionally we
        // observe that the picked leaf is the on-axis one.
        let spatial = spatial_with_unit_triangles(&[[0.0, 50.0, 0.0], [0.0, 0.0, 0.0]]);
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let on_axis_id = spatial.leaf_id_at_index(1).unwrap();
        assert_eq!(pick_object(&ray, &spatial), Some(on_axis_id));

        // Confirm the AABB rejector specifically rules out the off-axis object:
        // its world-space AABB should miss the ray.
        let q = spatial.leaf(spatial.leaf_id_at_index(0).unwrap()).unwrap();
        let (min, max) = world_aabb_from_local(q.local_aabb().unwrap(), &q.world_transform());
        assert!(
            !ray_hits_aabb(&ray, min, max),
            "off-axis object's world AABB must miss the on-axis ray"
        );
    }

    #[test]
    fn back_face_rejection_propagates_through_pick_object() {
        // A single CCW-front-facing triangle at the origin, with the ray
        // approaching from the back side. The triangle test rejects this, so
        // the pick should return None even though the AABB is intersected.
        let spatial = spatial_with_unit_triangles(&[[0.0, 0.0, 0.0]]);
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, -5.0),
            direction: Vec3::Z,
        };
        let q = spatial.leaf(spatial.leaf_id_at_index(0).unwrap()).unwrap();
        let (min, max) = world_aabb_from_local(q.local_aabb().unwrap(), &q.world_transform());
        assert!(
            ray_hits_aabb(&ray, min, max),
            "AABB should be intersected: that's the whole point of this test"
        );
        assert!(
            pick_object(&ray, &spatial).is_none(),
            "back-face hit must propagate as a no-pick"
        );
    }

    #[test]
    fn mirrored_object_keeps_picker_in_agreement_with_renderer() {
        // The triangle is CCW when viewed from +Z in object space, so its
        // object-space front face is +Z. A non-uniform scale of [-1, 1, 1] has
        // det = -1 and reverses world-space winding; the *world-space* front
        // face becomes -Z. The picker must pick that up: a ray from +Z hits the
        // world-space back face and must be rejected; a ray from -Z hits the
        // world-space front face and must succeed.
        let (mut g, root) = graph_with_group_root();
        add_leaf_under(
            &mut g,
            root,
            unit_triangle_mesh(),
            Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0])),
        );
        let spatial = SceneSpatial::from_graph(g);
        let from_above = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let from_below = Ray {
            origin: Vec3::new(0.0, 0.0, -5.0),
            direction: Vec3::Z,
        };
        let q = spatial.leaf(spatial.leaf_id_at_index(0).unwrap()).unwrap();
        let (min, max) = world_aabb_from_local(q.local_aabb().unwrap(), &q.world_transform());
        assert!(
            ray_hits_aabb(&from_above, min, max) && ray_hits_aabb(&from_below, min, max),
            "both rays must intersect the world AABB; otherwise the test isn't \
             exercising the sign-aware MT path"
        );
        assert!(
            pick_object(&from_above, &spatial).is_none(),
            "world-space back face on a mirrored object must be culled"
        );
        let only_leaf = spatial.leaf_id_at_index(0).unwrap();
        assert_eq!(
            pick_object(&from_below, &spatial),
            Some(only_leaf),
            "world-space front face on a mirrored object must hit"
        );
    }

    #[test]
    fn sub_millimetre_triangle_is_pickable() {
        // Pins that a front-face hit on a sub-millimetre triangle is accepted.
        // For a unit-direction ray `|det|` scales as `edge_length²`, so an
        // absolute floor on `det` such as 1e-7 would reject this hit, as for a
        // model exported in metres but loaded as millimetres; the gate on
        // `(det / (|edge1| · |edge2|))²` does not depend on scale.
        //
        // The triangle is a CCW patch 1e-4 mm wide in the z = 0 plane; `|det|`
        // for a unit-direction ray along -Z is about 1e-8.
        let s = 5e-5_f32;
        let mesh = IndexedMesh::new(
            None,
            vec![
                Vertex::new([-s, -s, 0.0]),
                Vertex::new([s, -s, 0.0]),
                Vertex::new([0.0, s, 0.0]),
            ],
            vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        );
        let (mut g, root) = graph_with_group_root();
        add_leaf_under(&mut g, root, mesh, Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(g);
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 1.0),
            direction: -Vec3::Z,
        };
        let only_leaf = spatial.leaf_id_at_index(0).unwrap();
        assert_eq!(pick_object(&ray, &spatial), Some(only_leaf));
    }

    #[test]
    fn sub_millimetre_back_face_is_rejected() {
        // Companion to `sub_millimetre_triangle_is_pickable`: with the
        // scale-invariant gate, the sign of `det * det_sign` carries the
        // front-face filter on its own (`det_oriented <= 0.0`). This pins that
        // the cull fires at small scale, so a change that drops or weakens the
        // sign check fails here.
        let s = 5e-5_f32;
        let mesh = IndexedMesh::new(
            None,
            vec![
                Vertex::new([-s, -s, 0.0]),
                Vertex::new([s, -s, 0.0]),
                Vertex::new([0.0, s, 0.0]),
            ],
            vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        );
        let (mut g, root) = graph_with_group_root();
        add_leaf_under(&mut g, root, mesh, Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(g);
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, -1.0),
            direction: Vec3::Z,
        };
        assert!(pick_object(&ray, &spatial).is_none());
    }
}

/// Pins that the picker's f32 `arvo` agrees with `Aabb::transformed_arvo`
/// (f64). Both call the generic `arvo_transformed_aabb`, at different
/// precisions. For the picker's use (gating a per-triangle f32 test) the f32
/// result must agree with the f64 result within a bound that scales with the
/// AABB diagonal, also on mirrored and rotated transforms.
#[cfg(test)]
mod arvo_consistency_tests {
    use super::arvo;
    use float_cmp::assert_approx_eq;
    use glam::Vec3;
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::{Aabb, Vertex};

    /// Asserts that `arvo` and `Aabb::transformed_arvo` agree on each axis,
    /// within a tolerance scaled by the world-AABB diagonal. f32 rounding in
    /// the centre and half-extent `mul_add` chains accumulates about 1e-5
    /// relative error; the 1e-3 absolute floor covers small AABBs near the
    /// origin.
    fn assert_arvo_agrees(local: Aabb, t: AffineTransform3D) {
        let f64_world = local.transformed_arvo(&t);
        let lmin = Vec3::from(local.min().coords());
        let lmax = Vec3::from(local.max().coords());
        let (f32_min, f32_max) = arvo(lmin, lmax, &t);

        let f64_min = f64_world.min().coords();
        let f64_max = f64_world.max().coords();
        let diag = ((f64_max[0] - f64_min[0]).powi(2)
            + (f64_max[1] - f64_min[1]).powi(2)
            + (f64_max[2] - f64_min[2]).powi(2))
        .sqrt();
        let eps = (diag * 1e-4).max(1e-3);

        for axis in 0..3 {
            assert_approx_eq!(f32, f32_min[axis], f64_min[axis], epsilon = eps);
            assert_approx_eq!(f32, f32_max[axis], f64_max[axis], epsilon = eps);
        }
    }

    fn unit_aabb() -> Aabb {
        Aabb::from_min_max(
            Vertex::new([-1.0, -1.0, -1.0]),
            Vertex::new([1.0, 1.0, 1.0]),
        )
    }

    #[test]
    fn identity_translation_agrees() {
        assert_arvo_agrees(unit_aabb(), AffineTransform3D::identity());
    }

    #[test]
    fn pure_translation_agrees() {
        assert_arvo_agrees(
            unit_aabb(),
            AffineTransform3D::from_translation([10.0, -5.0, 2.5]),
        );
    }

    #[test]
    fn axis_aligned_scale_agrees() {
        assert_arvo_agrees(
            unit_aabb(),
            AffineTransform3D::from_non_uniform_scale([2.0, 0.5, 3.0]),
        );
    }

    #[test]
    fn rotation_z_quarter_turn_agrees() {
        assert_arvo_agrees(
            unit_aabb(),
            AffineTransform3D::from_rotation_z(std::f64::consts::FRAC_PI_4),
        );
    }

    #[test]
    fn arbitrary_axis_rotation_agrees() {
        assert_arvo_agrees(
            unit_aabb(),
            AffineTransform3D::from_axis_angle([1.0, 2.0, 3.0], 0.7).unwrap(),
        );
    }

    #[test]
    fn mirror_negative_determinant_agrees() {
        // det = -1 path; both implementations apply the same |M[i][j]| step, so
        // the mirror should be invisible to the AABB result.
        assert_arvo_agrees(
            unit_aabb(),
            AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]),
        );
    }

    #[test]
    fn combined_translate_rotate_scale_agrees() {
        let t = AffineTransform3D::from_translation([2.0, -1.0, 4.0])
            .compose(&AffineTransform3D::from_rotation_z(0.6))
            .compose(&AffineTransform3D::from_non_uniform_scale([1.5, 0.75, 2.0]));
        assert_arvo_agrees(unit_aabb(), t);
    }

    #[test]
    fn off_origin_aabb_agrees() {
        // AABB whose corners are all positive: the centre is not at the origin,
        // so the centre-and-half-extents formula has both a non-trivial centre
        // transform and a non-trivial half-extent computation.
        let off_origin = Aabb::from_min_max(
            Vertex::new([10.0, 20.0, 30.0]),
            Vertex::new([12.0, 25.0, 33.0]),
        );
        let t = AffineTransform3D::from_rotation_z(std::f64::consts::FRAC_PI_3)
            .compose(&AffineTransform3D::from_non_uniform_scale([1.5, 1.5, 1.5]));
        assert_arvo_agrees(off_origin, t);
    }
}

#[cfg(test)]
mod ray_z_plane_intersection_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn vertical_ray_hits_z_plane_at_xy_of_origin() {
        let ray = Ray {
            origin: Vec3::new(3.0, 4.0, 10.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        let (x, y) = ray_z_plane_intersection(&ray, 0.0).expect("vertical ray hits Z=0");
        assert_approx_eq!(f32, x, 3.0, epsilon = 1e-5);
        assert_approx_eq!(f32, y, 4.0, epsilon = 1e-5);
    }

    #[test]
    fn forward_diagonal_ray_finds_correct_xy() {
        // Origin 10 units above plane, direction (1, 0, -1)/sqrt(2):
        // t = (0 - 10) / (-1/sqrt(2)) = 10*sqrt(2). hit.x = 10.
        let dir = Vec3::new(1.0, 0.0, -1.0).normalize();
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 10.0),
            direction: dir,
        };
        let (x, y) = ray_z_plane_intersection(&ray, 0.0).expect("diagonal ray hits Z=0");
        assert_approx_eq!(f32, x, 10.0, epsilon = 1e-4);
        assert_approx_eq!(f32, y, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn ray_parallel_to_plane_returns_none() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: Vec3::new(1.0, 0.0, 0.0),
        };
        assert!(ray_z_plane_intersection(&ray, 0.0).is_none());
    }

    #[test]
    fn plane_behind_camera_returns_none() {
        // Origin below plane and aimed downward: the plane sits behind the ray,
        // so the intersection is at t < 0.
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, -2.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        assert!(ray_z_plane_intersection(&ray, 0.0).is_none());
    }

    #[test]
    fn arbitrary_z_plane_is_honoured() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 0.0),
            direction: Vec3::new(0.0, 0.0, 1.0),
        };
        let (x, y) = ray_z_plane_intersection(&ray, 7.5).expect("upward ray hits Z=7.5");
        assert_approx_eq!(f32, x, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, y, 0.0, epsilon = 1e-5);
    }
}
