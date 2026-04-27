/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! CPU ray-pick from a logical viewport pixel to a `Scene` object index.
//!
//! Two-stage strategy:
//!
//! 1. **AABB pre-pass.** Each object's world-space AABB (Arvo's method
//!    on the local AABB and the object's `Transform3D`) is slab-tested
//!    against the ray. Objects whose AABB the ray misses are
//!    eliminated without ever touching their triangles.
//!
//! 2. **Triangle test on survivors.** Each surviving object's faces
//!    are walked; for each face the three vertices are transformed to
//!    world space using the object's transform and run through
//!    Möller–Trumbore. Back-face hits are rejected so the picker
//!    matches the renderer's `cull_mode: Some(Face::Back)` (with
//!    `front_face: FrontFace::Ccw`). The smallest positive `t` across
//!    all survivors wins.
//!
//! Triangles are iterated in world space rather than inverting the
//! object's transform onto the ray. This costs two matrix-multiplies
//! per triangle but avoids inverting per-object (and the
//! singular-linear-part special case that comes with it). It also
//! makes a transform with a negative determinant naturally agree with
//! the renderer: the world-space winding flips, MT's `det > 0` check
//! flips the front/back classification, and the picker rejects what
//! the renderer culls.

use glam::{Mat4, Vec3, Vec4};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::scene::{Scene, SceneObjectContent, Transform3D};
use sc_mesh_core::transform::AffineTransform3D;

use crate::camera::OrbitCamera;

/// Tolerance for the determinant in Möller–Trumbore. Hits with a
/// determinant smaller than this in absolute value are treated as
/// grazing / back-facing and rejected. f32 ulps near 1.0 are around
/// 1.19e-7, so 1e-7 is a few ulps above noise.
const EPSILON: f32 = 1e-7;

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

/// Picks the closest object the ray hits, by index into
/// `scene.objects`. Returns `None` if every object misses (whether by
/// AABB pre-pass or triangle test).
pub fn pick_object(ray: &Ray, scene: &Scene) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (idx, _) in scene.objects.iter().enumerate() {
        let Some((min, max)) = world_aabb_for_object(scene, idx) else {
            continue;
        };
        if !ray_hits_aabb(ray, min, max) {
            continue;
        }
        if let Some(t) = ray_hits_object(ray, scene, idx) {
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

/// Möller–Trumbore intersection of a ray with a single triangle,
/// front-face only. The CCW-winding-is-front convention matches the
/// renderer's `front_face: FrontFace::Ccw, cull_mode: Some(Face::Back)`
/// pipeline state — `det > EPSILON` corresponds to `D · n < 0`, i.e.
/// the ray pointing into the surface from in front of it.
pub(crate) fn ray_hits_triangle(ray: &Ray, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = ray.direction.cross(edge2);
    let det = edge1.dot(h);
    if det < EPSILON {
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

/// Closest front-face hit `t` for the object at `idx`, computed in
/// world space. Resolves `Resource` references against
/// `scene.resources`; returns `None` if the resource index is out of
/// bounds (treated like an AABB miss — the picker doesn't surface
/// errors for malformed scenes).
pub(crate) fn ray_hits_object(ray: &Ray, scene: &Scene, idx: usize) -> Option<f32> {
    let object = scene.objects.get(idx)?;
    let mesh = match &object.content {
        SceneObjectContent::Mesh(m) => m,
        SceneObjectContent::Resource(r) => scene.resources.get(*r)?,
    };
    let to_world = transform_to_mat4(&object.transform);
    let mut best: Option<f32> = None;
    for face in &mesh.faces {
        let v0 = world_vertex(mesh, face.vertices[0], &to_world)?;
        let v1 = world_vertex(mesh, face.vertices[1], &to_world)?;
        let v2 = world_vertex(mesh, face.vertices[2], &to_world)?;
        if let Some(t) = ray_hits_triangle(ray, v0, v1, v2) {
            best = Some(match best {
                Some(prev) if prev <= t => prev,
                _ => t,
            });
        }
    }
    best
}

fn world_vertex(mesh: &IndexedMesh, vidx: u32, to_world: &Mat4) -> Option<Vec3> {
    let v = mesh.vertices.get(vidx as usize)?;
    Some(to_world.transform_point3(Vec3::from(v.0)))
}

fn world_aabb_for_object(scene: &Scene, idx: usize) -> Option<(Vec3, Vec3)> {
    let object = scene.objects.get(idx)?;
    let mesh = match &object.content {
        SceneObjectContent::Mesh(m) => m,
        SceneObjectContent::Resource(r) => scene.resources.get(*r)?,
    };
    let (lmin, lmax) = local_aabb(mesh)?;
    Some(match &object.transform {
        Transform3D::Identity => (lmin, lmax),
        Transform3D::Affine(a) => arvo(lmin, lmax, a),
    })
}

fn local_aabb(mesh: &IndexedMesh) -> Option<(Vec3, Vec3)> {
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

/// Arvo's transformed-AABB formula in f32. Mirrors `scene::arvo_world_aabb`
/// (which works in f64 for vertex-AABB precision); for picking, f32 is
/// adequate — the AABB only gates the triangle test, which itself is
/// f32 throughout.
fn arvo(lmin: Vec3, lmax: Vec3, a: &AffineTransform3D) -> (Vec3, Vec3) {
    let center = (lmin + lmax) * 0.5;
    let half = (lmax - lmin) * 0.5;

    let m = &a.linear;
    let t = &a.translation;

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

fn transform_to_mat4(t: &Transform3D) -> Mat4 {
    match t {
        Transform3D::Identity => Mat4::IDENTITY,
        Transform3D::Affine(a) => {
            let l = &a.linear;
            let t = &a.translation;
            // Row-major → column-major (glam) at the f64 → f32 boundary,
            // matching `mesh_render::affine_to_mat4`.
            Mat4::from_cols(
                Vec4::new(l[0][0] as f32, l[1][0] as f32, l[2][0] as f32, 0.0),
                Vec4::new(l[0][1] as f32, l[1][1] as f32, l[2][1] as f32, 0.0),
                Vec4::new(l[0][2] as f32, l[1][2] as f32, l[2][2] as f32, 0.0),
                Vec4::new(t[0] as f32, t[1] as f32, t[2] as f32, 1.0),
            )
        }
    }
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
mod ray_hits_triangle_tests {
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
        let t = ray_hits_triangle(&ray, v0, v1, v2).expect("ray should hit");
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
            ray_hits_triangle(&ray, v0, v1, v2).is_none(),
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
        assert!(ray_hits_triangle(&ray, v0, v1, v2).is_none());
    }

    #[test]
    fn rejects_parallel_ray() {
        let (v0, v1, v2) = ccw_triangle();
        // Parallel to the triangle's plane (z = 0) and offset by some y.
        let ray = Ray {
            origin: Vec3::new(0.0, 5.0, 5.0),
            direction: Vec3::X,
        };
        assert!(ray_hits_triangle(&ray, v0, v1, v2).is_none());
    }

    #[test]
    fn rejects_ray_outside_triangle() {
        let (v0, v1, v2) = ccw_triangle();
        let ray = Ray {
            origin: Vec3::new(10.0, 10.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_triangle(&ray, v0, v1, v2).is_none());
    }
}

#[cfg(test)]
mod pick_object_tests {
    use super::*;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;

    fn unit_triangle_at(translation: [f32; 3]) -> SceneObject {
        // CCW when viewed from +Z, ~unit-sized.
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([-0.5, -0.5, 0.0]),
                Vertex::new([0.5, -0.5, 0.0]),
                Vertex::new([0.0, 0.5, 0.0]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 1, 2],
            }],
        };
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

    #[test]
    fn ray_hits_object_uses_object_transform() {
        // Same triangle, two scenes — one identity, one translated by
        // +X. Ray points at the +X position; only the translated copy
        // should be hit.
        let id_scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: unit_triangle_at([0.0, 0.0, 0.0]).content,
            }],
        };
        let shifted_scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![unit_triangle_at([10.0, 0.0, 0.0])],
        };
        // Ray goes through (10, 0, 0) toward -Z.
        let ray = Ray {
            origin: Vec3::new(10.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert!(ray_hits_object(&ray, &id_scene, 0).is_none());
        assert!(ray_hits_object(&ray, &shifted_scene, 0).is_some());
    }

    #[test]
    fn empty_scene_picks_nothing() {
        let scene = Scene::default();
        let ray = Ray {
            origin: Vec3::ZERO,
            direction: Vec3::X,
        };
        assert!(pick_object(&ray, &scene).is_none());
    }

    #[test]
    fn returns_none_when_ray_misses_every_object() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                unit_triangle_at([0.0, 0.0, 0.0]),
                unit_triangle_at([10.0, 0.0, 0.0]),
            ],
        };
        // Ray points to +Y from far above; no triangle is in its path.
        let ray = Ray {
            origin: Vec3::new(0.0, 100.0, 5.0),
            direction: Vec3::Y,
        };
        assert!(pick_object(&ray, &scene).is_none());
    }

    #[test]
    fn closest_hit_wins_among_overlapping_along_ray() {
        // Three triangles along -Z at z = 0, z = -2, z = -4.
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                unit_triangle_at([0.0, 0.0, 0.0]),
                unit_triangle_at([0.0, 0.0, -2.0]),
                unit_triangle_at([0.0, 0.0, -4.0]),
            ],
        };
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        // The z = 0 triangle is closest.
        assert_eq!(pick_object(&ray, &scene), Some(0));
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
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![off_axis, on_axis],
        };
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: -Vec3::Z,
        };
        assert_eq!(pick_object(&ray, &scene), Some(1));

        // Confirm the AABB rejector specifically rules out the
        // off-axis object — its world-space AABB should miss the ray.
        let (min, max) = world_aabb_for_object(&scene, 0).unwrap();
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
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![unit_triangle_at([0.0, 0.0, 0.0])],
        };
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, -5.0),
            direction: Vec3::Z,
        };
        let (min, max) = world_aabb_for_object(&scene, 0).unwrap();
        assert!(
            ray_hits_aabb(&ray, min, max),
            "AABB should be intersected — that's the whole point of this test"
        );
        assert!(
            pick_object(&ray, &scene).is_none(),
            "back-face hit must propagate as a no-pick"
        );
    }

    #[test]
    fn dangling_resource_index_is_treated_as_a_no_op() {
        // The picker shouldn't surface errors for malformed scenes —
        // it just skips the object. The renderer's scene-build path
        // already errors on dangling indices, so the picker only sees
        // such scenes if something else has gone wrong upstream.
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Resource(99),
            }],
        };
        let ray = Ray {
            origin: Vec3::ZERO,
            direction: Vec3::X,
        };
        assert!(pick_object(&ray, &scene).is_none());
    }
}
