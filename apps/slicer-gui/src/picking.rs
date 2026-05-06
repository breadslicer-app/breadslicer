/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! CPU ray-pick from a logical viewport pixel to a `Scene` object index.
//!
//! Three-stage strategy:
//!
//! 1. **World AABB pre-pass.** Each object's world-space AABB (Arvo's
//!    method on the local AABB and the object's `Transform3D`) is
//!    slab-tested against the ray. Objects whose AABB the ray misses
//!    are eliminated without ever touching their triangles.
//!
//! 2. **Object-space cluster traversal.** Survivors invert the ray
//!    once into object space, then slab-test each
//!    [`TriangleClusters`](sc_mesh_core::IndexedMesh::triangle_clusters)
//!    AABB against the local ray. Clusters the ray misses skip
//!    the linear MT walk entirely.
//!
//! 3. **Sign-aware Möller–Trumbore on cluster survivors.** Faces in
//!    surviving clusters are tested in object space. The renderer's
//!    `cull_mode: Some(Face::Back)` with `front_face: FrontFace::Ccw`
//!    is matched by accepting hits whose MT determinant has the same
//!    sign as the linear part's determinant: a transform with
//!    `det < 0` reverses world-space winding, so the object-space
//!    front face is the world-space back face — and vice versa.
//!
//! Iterating in object space turns a per-triangle 3 × `Mat4 * Vec3`
//! transform (the previous world-space variant) into a per-object
//! single ray-inversion. The cluster slab-tests then prune the linear
//! MT walk to roughly `O(F / cluster_size)` AABB tests plus
//! `O(cluster_size)` MT tests on hit clusters.

use glam::{Mat4, Vec3, Vec4};
use sc_mesh_core::Aabb;
use sc_mesh_core::scene::Transform3D;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_spatial::{ObjectQuery, SceneSpatial};

use crate::camera::OrbitCamera;

/// Tolerance for the determinant in Möller–Trumbore. Hits with a
/// determinant smaller than this in absolute value are treated as
/// grazing / back-facing and rejected. f32 ulps near 1.0 are around
/// 1.19e-7, so 1e-7 is a few ulps above noise.
const EPSILON: f32 = 1e-7;

/// Object-space ray plus the conversion factor needed to bring an MT
/// `t` back to world space. `det_sign` is `+1.0` when the parent
/// transform's linear part has positive determinant, `-1.0` when
/// negative — used to flip the MT front-face filter for mirrored
/// objects so the picker keeps agreeing with the renderer's CCW
/// culling.
struct LocalRay {
    origin: Vec3,
    direction: Vec3,
    /// `t_world = t_local / world_to_local_scale`. `1.0` for the
    /// identity transform.
    world_to_local_scale: f32,
    det_sign: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    /// Always unit length (constructed by [`ray_from_pixel`]).
    pub direction: Vec3,
}

/// Builds a world-space ray from a logical-pixel cursor position.
///
/// `(x, y)` are in Slint's logical viewport coordinates: top-left
/// origin, y growing downward, *not* multiplied by `scale_factor`.
/// `(viewport_w, viewport_h)` use the same units. `fov_y` is the
/// vertical FOV in radians, matching the value the renderer feeds
/// into `Mat4::perspective_rh`.
pub fn ray_from_pixel(
    camera: &OrbitCamera,
    fov_y: f32,
    viewport_w: f32,
    viewport_h: f32,
    x: f32,
    y: f32,
) -> Ray {
    let aspect = viewport_w / viewport_h;
    let tan_half = (fov_y * 0.5).tan();

    // NDC: x ∈ [-1, +1] (right), y ∈ [-1, +1] (up). Slint's pixel y
    // grows downward, so it is flipped here.
    let ndc_x = (2.0 * x / viewport_w) - 1.0;
    let ndc_y = 1.0 - (2.0 * y / viewport_h);

    // Right-handed camera-space direction with the camera looking down
    // -Z. `aspect * tan_half` opens the horizontal extent so that a
    // pixel at `ndc_x = ±1` lands on the screen edge after projection.
    let cs_dir = Vec3::new(ndc_x * aspect * tan_half, ndc_y * tan_half, -1.0).normalize();

    // World-space transform. Direction has w = 0 (no translation).
    let inv_view = camera.view_matrix().inverse();
    let world_dir = inv_view * Vec4::new(cs_dir.x, cs_dir.y, cs_dir.z, 0.0);
    let world_dir = Vec3::new(world_dir.x, world_dir.y, world_dir.z).normalize();

    Ray {
        origin: camera.eye(),
        direction: world_dir,
    }
}

/// Projects a world-space point onto the viewport in logical-pixel
/// coordinates (top-left origin, y growing downward — same convention
/// as `ray_from_pixel`'s input). Returns `None` when the point is
/// behind the camera.
///
/// Used by the gizmo's uniform-scale drag, where the press distance
/// is measured in screen pixels relative to the projected pivot. The
/// near/far depths used here are placeholders: the projection
/// matrix's near/far only affect z, and we discard the z component
/// after the perspective divide.
pub fn project_world_to_screen(
    camera: &OrbitCamera,
    fov_y: f32,
    viewport_w: f32,
    viewport_h: f32,
    world_point: Vec3,
) -> Option<(f32, f32)> {
    let aspect = viewport_w / viewport_h.max(1.0);
    let view = camera.view_matrix();
    let proj = Mat4::perspective_rh(fov_y, aspect, 0.01, 100.0);
    let clip = (proj * view) * Vec4::new(world_point.x, world_point.y, world_point.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc_x = clip.x / clip.w;
    let ndc_y = clip.y / clip.w;
    let screen_x = (ndc_x + 1.0) * 0.5 * viewport_w;
    // Slint-y grows downward, NDC-y grows upward → flip.
    let screen_y = (1.0 - ndc_y) * 0.5 * viewport_h;
    Some((screen_x, screen_y))
}

/// Picks the closest object the ray hits, by index into
/// `spatial.scene().objects`. Returns `None` if every object misses
/// (whether by AABB pre-pass or triangle test).
pub fn pick_object(ray: &Ray, spatial: &SceneSpatial) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for idx in 0..spatial.scene().objects.len() {
        let Some(query) = spatial.object(idx) else {
            continue;
        };
        let Some(local) = query.local_aabb() else {
            continue;
        };
        let (min, max) = world_aabb_from_local(local, &query.object().transform);
        if !ray_hits_aabb(ray, min, max) {
            continue;
        }
        if let Some(t) = ray_hits_object_query(ray, &query) {
            match best {
                Some((_, prev_t)) if prev_t <= t => {}
                _ => best = Some((idx, t)),
            }
        }
    }
    best.map(|(idx, _)| idx)
}

/// Slab test against an axis-aligned box. Branchless variant: zero
/// direction components produce ±inf, which propagate harmlessly
/// through the min/max chain when the ray's origin is inside the
/// slab, and reject when it isn't. Hits with `t_exit < 0` (box
/// entirely behind the ray) are rejected.
pub(crate) fn ray_hits_aabb(ray: &Ray, min: Vec3, max: Vec3) -> bool {
    ray_hits_aabb_t(ray, min, max).is_some()
}

/// Slab test variant that returns the ray parameter at which the
/// ray enters the box (`t_enter`), or `None` if the ray misses or
/// the box is behind the origin. Origin-inside-box returns 0 as the
/// entry parameter. Used by the gizmo cube hit tests to feed
/// front-most-`t` arbitration alongside the cylinder / arc tests.
pub(crate) fn ray_hits_aabb_t(ray: &Ray, min: Vec3, max: Vec3) -> Option<f32> {
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

/// Sign-aware Möller–Trumbore intersection. `det_sign = +1.0` is the
/// classic "front face = CCW winding" rule that matches the renderer's
/// `front_face: FrontFace::Ccw, cull_mode: Some(Face::Back)` pipeline
/// state. `det_sign = -1.0` flips the front-face filter, which is
/// what the picker needs for objects whose `Transform3D` has a
/// negative-determinant linear part: the world-space winding has been
/// reversed, so the object-space triangle's *back* face is the
/// renderer's *front* face. The barycentric and `t > EPSILON` checks
/// are sign-symmetric, so only the determinant filter at the top
/// changes.
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
    if det * det_sign < EPSILON {
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

/// Closest front-face hit `t` for the object behind `query`, in
/// world-space ray parameters. Returns `None` for malformed scenes
/// (singular linear part on the object's transform, empty mesh, or
/// out-of-range face vertex indices). The picker treats every such
/// case as a clean miss rather than surfacing an error.
pub(crate) fn ray_hits_object_query(ray: &Ray, query: &ObjectQuery<'_>) -> Option<f32> {
    let local = local_ray_for_object(ray, &query.object().transform)?;
    let local_ray = Ray {
        origin: local.origin,
        direction: local.direction,
    };
    let clusters = query.clusters()?;
    let mesh = query.mesh();
    let vertices = mesh.vertices();
    let faces = mesh.faces();
    let mut best: Option<f32> = None;
    for (face_range, aabb) in clusters.iter() {
        let aabb_min = Vec3::from(aabb.min().coords());
        let aabb_max = Vec3::from(aabb.max().coords());
        if !ray_hits_aabb(&local_ray, aabb_min, aabb_max) {
            continue;
        }
        for face in &faces[face_range.start as usize..face_range.end as usize] {
            let v0 = Vec3::from(vertices.get(usize::from(face.vertices[0]))?.coords());
            let v1 = Vec3::from(vertices.get(usize::from(face.vertices[1]))?.coords());
            let v2 = Vec3::from(vertices.get(usize::from(face.vertices[2]))?.coords());
            if let Some(t_local) =
                ray_hits_triangle_oriented(&local_ray, v0, v1, v2, local.det_sign)
            {
                let t = t_local / local.world_to_local_scale;
                best = Some(match best {
                    Some(prev) if prev <= t => prev,
                    _ => t,
                });
            }
        }
    }
    best
}

/// Build the object-space form of `ray` for `transform`. Returns
/// `None` when the transform's linear part is singular — the picker
/// treats that as a malformed object and skips it. For an identity
/// transform the local ray equals the world ray, with
/// `world_to_local_scale = 1` and `det_sign = +1`.
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
            // A finite, non-singular `inverse()` cannot produce a zero
            // direction from a unit world direction, so `scale > 0`
            // here; guard anyway to keep the division explicit.
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

/// Apply Arvo's transformed-AABB formula to a cached object-space
/// AABB and the object's transform. Used by [`pick_object`]'s pre-pass
/// to slab-test world-space AABBs without re-walking vertex data.
fn world_aabb_from_local(local: Aabb, transform: &Transform3D) -> (Vec3, Vec3) {
    let lmin = Vec3::from(local.min().coords());
    let lmax = Vec3::from(local.max().coords());
    match transform {
        Transform3D::Identity => (lmin, lmax),
        Transform3D::Affine(a) => arvo(lmin, lmax, a),
    }
}

/// Arvo's transformed-AABB formula in f32. Mirrors `Aabb::transformed_arvo`
/// (which works in f64 for vertex-AABB precision); for picking, f32 is
/// adequate — the AABB only gates the triangle test, which itself is
/// f32 throughout.
fn arvo(lmin: Vec3, lmax: Vec3, a: &AffineTransform3D) -> (Vec3, Vec3) {
    let center = (lmin + lmax) * 0.5;
    let half = (lmax - lmin) * 0.5;

    let m = a.linear();
    let t = a.translation();

    let new_center = Vec3::new(
        (m[0][0] as f32) * center.x
            + (m[0][1] as f32) * center.y
            + (m[0][2] as f32) * center.z
            + (t[0] as f32),
        (m[1][0] as f32) * center.x
            + (m[1][1] as f32) * center.y
            + (m[1][2] as f32) * center.z
            + (t[1] as f32),
        (m[2][0] as f32) * center.x
            + (m[2][1] as f32) * center.y
            + (m[2][2] as f32) * center.z
            + (t[2] as f32),
    );
    let new_half = Vec3::new(
        (m[0][0] as f32).abs() * half.x
            + (m[0][1] as f32).abs() * half.y
            + (m[0][2] as f32).abs() * half.z,
        (m[1][0] as f32).abs() * half.x
            + (m[1][1] as f32).abs() * half.y
            + (m[1][2] as f32).abs() * half.z,
        (m[2][0] as f32).abs() * half.x
            + (m[2][1] as f32).abs() * half.y
            + (m[2][2] as f32).abs() * half.z,
    );

    (new_center - new_half, new_center + new_half)
}

#[cfg(test)]
mod ray_from_pixel_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn camera_at_origin() -> OrbitCamera {
        // Target at origin, radius 1. The orbit camera defaults to a
        // non-axis-aligned view (yaw/pitch ≠ 0); fine for our checks
        // that use the camera's own basis.
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
        // For a wide viewport, a horizontal-edge ray must subtend a
        // wider angle from forward than a vertical-edge ray of the
        // same NDC magnitude. Compare cos(angle) — the wider angle
        // has the smaller cos.
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
    use super::*;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent};

    fn unit_triangle_at(translation: [f32; 3]) -> SceneObject {
        // CCW when viewed from +Z, ~unit-sized.
        let mesh = IndexedMesh::new(
            None,
            vec![
                Vertex::new([-0.5, -0.5, 0.0]),
                Vertex::new([0.5, -0.5, 0.0]),
                Vertex::new([0.0, 0.5, 0.0]),
            ],
            vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        );
        SceneObject {
            meta: None,
            transform: Transform3D::Affine(AffineTransform3D::from_translation([
                translation[0] as f64,
                translation[1] as f64,
                translation[2] as f64,
            ])),
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn spatial_for(scene: Scene) -> SceneSpatial {
        SceneSpatial::new(scene)
    }

    #[test]
    fn ray_hits_object_uses_object_transform() {
        // Same triangle, two scenes — one identity, one translated by
        // +X. Ray points at the +X position; only the translated copy
        // should be hit.
        let id_spatial = spatial_for(Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: unit_triangle_at([0.0, 0.0, 0.0]).content,
            }],
        });
        let shifted_spatial = spatial_for(Scene {
            meta: None,
            resources: vec![],
            objects: vec![unit_triangle_at([10.0, 0.0, 0.0])],
        });
        // Ray goes through (10, 0, 0) toward -Z.
        let ray = Ray {
            origin: Vec3::new(10.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_object_query(&ray, &id_spatial.object(0).unwrap()).is_none());
        assert!(ray_hits_object_query(&ray, &shifted_spatial.object(0).unwrap()).is_some());
    }

    #[test]
    fn empty_scene_picks_nothing() {
        let spatial = spatial_for(Scene::default());
        let ray = Ray {
            origin: Vec3::ZERO,
            direction: Vec3::X,
        };
        assert!(pick_object(&ray, &spatial).is_none());
    }

    #[test]
    fn returns_none_when_ray_misses_every_object() {
        let spatial = spatial_for(Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                unit_triangle_at([0.0, 0.0, 0.0]),
                unit_triangle_at([10.0, 0.0, 0.0]),
            ],
        });
        // Ray points to +Y from far above; no triangle is in its path.
        let ray = Ray {
            origin: Vec3::new(0.0, 100.0, 5.0),
            direction: Vec3::Y,
        };
        assert!(pick_object(&ray, &spatial).is_none());
    }

    #[test]
    fn closest_hit_wins_among_overlapping_along_ray() {
        // Three triangles along -Z at z = 0, z = -2, z = -4.
        let spatial = spatial_for(Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                unit_triangle_at([0.0, 0.0, 0.0]),
                unit_triangle_at([0.0, 0.0, -2.0]),
                unit_triangle_at([0.0, 0.0, -4.0]),
            ],
        });
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        // The z = 0 triangle is closest.
        assert_eq!(pick_object(&ray, &spatial), Some(0));
    }

    #[test]
    fn aabb_pre_pass_skips_off_ray_objects() {
        // Two triangles. One in the ray's path, one off to the side
        // (+Y by a wide margin). The off-axis one's AABB is missed by
        // the ray, so the picker eliminates it without running its
        // triangle test. Functionally we observe that the picked
        // index is the on-axis one.
        let on_axis = unit_triangle_at([0.0, 0.0, 0.0]);
        let off_axis = unit_triangle_at([0.0, 50.0, 0.0]);
        let spatial = spatial_for(Scene {
            meta: None,
            resources: vec![],
            objects: vec![off_axis, on_axis],
        });
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(pick_object(&ray, &spatial), Some(1));

        // Confirm the AABB rejector specifically rules out the
        // off-axis object — its world-space AABB should miss the ray.
        let q = spatial.object(0).unwrap();
        let (min, max) = world_aabb_from_local(q.local_aabb().unwrap(), &q.object().transform);
        assert!(
            !ray_hits_aabb(&ray, min, max),
            "off-axis object's world AABB must miss the on-axis ray"
        );
    }

    #[test]
    fn back_face_rejection_propagates_through_pick_object() {
        // A single CCW-front-facing triangle at the origin, with the
        // ray approaching from the back side. The triangle test
        // rejects this, so the pick should return None even though
        // the AABB is intersected.
        let spatial = spatial_for(Scene {
            meta: None,
            resources: vec![],
            objects: vec![unit_triangle_at([0.0, 0.0, 0.0])],
        });
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, -5.0),
            direction: Vec3::Z,
        };
        let q = spatial.object(0).unwrap();
        let (min, max) = world_aabb_from_local(q.local_aabb().unwrap(), &q.object().transform);
        assert!(
            ray_hits_aabb(&ray, min, max),
            "AABB should be intersected — that's the whole point of this test"
        );
        assert!(
            pick_object(&ray, &spatial).is_none(),
            "back-face hit must propagate as a no-pick"
        );
    }

    #[test]
    fn mirrored_object_keeps_picker_in_agreement_with_renderer() {
        // The triangle is CCW when viewed from +Z in object space, so
        // its object-space front face is +Z. A non-uniform scale of
        // [-1, 1, 1] has det = -1 and reverses world-space winding —
        // the *world-space* front face becomes -Z. The picker must
        // pick that up: a ray from +Z hits the world-space back face
        // and must be rejected; a ray from -Z hits the world-space
        // front face and must succeed.
        let mut object = unit_triangle_at([0.0, 0.0, 0.0]);
        object.transform =
            Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]));
        let spatial = spatial_for(Scene {
            meta: None,
            resources: vec![],
            objects: vec![object],
        });
        let from_above = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        let from_below = Ray {
            origin: Vec3::new(0.0, 0.0, -5.0),
            direction: Vec3::Z,
        };
        let q = spatial.object(0).unwrap();
        let (min, max) = world_aabb_from_local(q.local_aabb().unwrap(), &q.object().transform);
        assert!(
            ray_hits_aabb(&from_above, min, max) && ray_hits_aabb(&from_below, min, max),
            "both rays must intersect the world AABB; otherwise the test isn't \
             exercising the sign-aware MT path"
        );
        assert!(
            pick_object(&from_above, &spatial).is_none(),
            "world-space back face on a mirrored object must be culled"
        );
        assert_eq!(
            pick_object(&from_below, &spatial),
            Some(0),
            "world-space front face on a mirrored object must hit"
        );
    }

    #[test]
    fn dangling_resource_index_is_treated_as_a_no_op() {
        // The picker shouldn't surface errors for malformed scenes —
        // it just skips the object. The renderer's scene-build path
        // already errors on dangling indices, so the picker only sees
        // such scenes if something else has gone wrong upstream.
        let spatial = spatial_for(Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Resource(99),
            }],
        });
        let ray = Ray {
            origin: Vec3::ZERO,
            direction: Vec3::X,
        };
        assert!(pick_object(&ray, &spatial).is_none());
    }
}

/// Cross-precision consistency between the picker's f32 `arvo` and
/// `Aabb::transformed_arvo` (f64). The two implement the same Arvo
/// closed-form formula at different precisions; for the picker's
/// use (gating a per-triangle f32 test) the f32 result must agree
/// with the f64 reference within a precision bound that scales with
/// the AABB diagonal.
///
/// Until they share an implementation, this test pins the
/// equivalence so a drift between the two surfaces immediately
/// rather than at render time on a mirrored / non-axis-aligned
/// scene.
#[cfg(test)]
mod arvo_consistency_tests {
    use super::arvo;
    use float_cmp::assert_approx_eq;
    use glam::Vec3;
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::{Aabb, Vertex};

    /// Per-axis tolerance scaled by the world-AABB diagonal.
    /// f32 cancellation in the corner-by-corner half-extent
    /// `mul_add` chain accumulates ~1e-5 relative; 1e-3 absolute
    /// floor handles small AABBs near the origin.
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
        // det = -1 path; both implementations apply the same |M[i][j]|
        // step, so the mirror should be invisible to the AABB result.
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
        // AABB whose corners are all positive — the centre is not at
        // the origin, so the centre-and-half-extents formula has both
        // a non-trivial centre transform and a non-trivial half-extent
        // computation.
        let off_origin = Aabb::from_min_max(
            Vertex::new([10.0, 20.0, 30.0]),
            Vertex::new([12.0, 25.0, 33.0]),
        );
        let t = AffineTransform3D::from_rotation_z(std::f64::consts::FRAC_PI_3)
            .compose(&AffineTransform3D::from_non_uniform_scale([1.5, 1.5, 1.5]));
        assert_arvo_agrees(off_origin, t);
    }
}
