/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure-CPU helpers for the translate gizmo: pivot, world-aligned
//! axes, screen-constant scale, ray-vs-axis-handle hit test, and the
//! drag-time closest-point math. Renderer-agnostic so it can be
//! unit-tested without a `wgpu::Device`.

pub mod axis;
pub mod dimensions;
pub mod drag_math;
pub mod hit_test;
mod tolerances;

use glam::Vec3;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::scene::{Scene, SceneObjectContent, Transform3D};

use crate::camera::OrbitCamera;

use self::dimensions::{HANDLE_PIXEL_LENGTH, HANDLE_PIXEL_RADIUS};

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
            Vec3::from(v.coords())
        }
    })
}

fn local_aabb_centre(mesh: &IndexedMesh) -> Option<(Vec3, Vec3)> {
    let mut iter = mesh.vertices().iter();
    let first = Vec3::from(iter.next()?.coords());
    let mut min = first;
    let mut max = first;
    for v in iter {
        let v = Vec3::from(v.coords());
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
