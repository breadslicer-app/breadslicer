/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pure-CPU helpers for the translate, rotate and scale gizmo: pivot,
//! world-aligned axes, screen-constant scale, ray hit tests against the
//! handles, and the drag math. Renderer-agnostic so it can be unit-tested
//! without a `wgpu::Device`.

pub(crate) mod axis;
pub(crate) mod dimensions;
pub(crate) mod drag_math;
pub(crate) mod frame;
pub(crate) mod hit_test;
mod tolerances;

use glam::Vec3;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::camera::OrbitCamera;

use self::dimensions::{HANDLE_PIXEL_LENGTH, HANDLE_PIXEL_RADIUS};

/// Returns the world-space pivot for the gizmo: the local-AABB centre of the
/// leaf, transformed by the leaf's world transform. Returns `None` when the
/// leaf has no vertices, an out-of-bounds resource reference, `node_id` does
/// not name a reachable leaf, or the resulting pivot has a non-finite component
/// (from NaN or infinite vertex coordinates, or from a transform that overflows
/// to infinity); in all of these cases the gizmo cannot attach.
pub(crate) fn pivot_for_leaf(spatial: &SceneSpatial, node_id: NodeId) -> Option<Vec3> {
    let query = spatial.leaf(node_id)?;
    let aabb = query.local_aabb()?;
    let lmin = Vec3::from(aabb.min().coords());
    let lmax = Vec3::from(aabb.max().coords());
    let local_centre = (lmin + lmax) * 0.5;
    if !local_centre.is_finite() {
        return None;
    }
    let pivot = match &query.world_transform() {
        Transform3D::Identity => local_centre,
        Transform3D::Affine(a) => {
            let v = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new([
                local_centre.x,
                local_centre.y,
                local_centre.z,
            ]));
            Vec3::from(v.coords())
        }
    };
    pivot.is_finite().then_some(pivot)
}

/// Returns the world-space shaft length that makes the gizmo at `pivot` span
/// `HANDLE_PIXEL_LENGTH` pixels on screen. Returns `None` when `pivot` is not
/// in front of the eye.
pub(crate) fn gizmo_scale(
    camera: &OrbitCamera,
    pivot: Vec3,
    fov_y: f32,
    viewport_h: f32,
) -> Option<f32> {
    camera
        .world_per_pixel_at(pivot, fov_y, viewport_h)
        .map(|world_per_pixel| world_per_pixel * HANDLE_PIXEL_LENGTH)
}

/// Returns the world-space radius of the translate hit cylinder that spans
/// `HANDLE_PIXEL_RADIUS` pixels on screen at `pivot`. Returns `None` when
/// `pivot` is not in front of the eye.
pub(crate) fn hit_radius_world(
    camera: &OrbitCamera,
    pivot: Vec3,
    fov_y: f32,
    viewport_h: f32,
) -> Option<f32> {
    camera
        .world_per_pixel_at(pivot, fov_y, viewport_h)
        .map(|world_per_pixel| world_per_pixel * HANDLE_PIXEL_RADIUS)
}

#[cfg(test)]
mod pivot_tests {
    use std::sync::Arc;

    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_scene::{LeafPayload, Scene};

    fn triangle_mesh(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> IndexedMesh {
        IndexedMesh::new(
            None,
            vec![Vertex::new(v0), Vertex::new(v1), Vertex::new(v2)],
            vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        )
    }

    /// Returns a single-leaf scene under a Group root, mirroring the shape the
    /// loader pipeline produces (1 inline-mesh leaf attached under an
    /// identity-Group root).
    fn spatial_with_one_leaf(mesh: IndexedMesh, transform: Transform3D) -> SceneSpatial {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let leaf = g
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        SceneSpatial::from_graph(g)
    }

    fn first_leaf(spatial: &SceneSpatial) -> NodeId {
        spatial
            .leaf_id_at_index(0)
            .expect("fixture has a leaf at idx 0")
    }

    #[test]
    fn pivot_under_identity_is_local_aabb_centre() {
        let spatial = spatial_with_one_leaf(
            triangle_mesh([-1.0, -2.0, -3.0], [3.0, 0.0, 0.0], [-1.0, 4.0, 5.0]),
            Transform3D::Identity,
        );
        let p = pivot_for_leaf(&spatial, first_leaf(&spatial)).unwrap();
        // local AABB = ([-1, -2, -3], [3, 4, 5]), so its centre is (1, 1, 1).
        assert_approx_eq!(f32, p.x, 1.0);
        assert_approx_eq!(f32, p.y, 1.0);
        assert_approx_eq!(f32, p.z, 1.0);
    }

    #[test]
    fn pivot_under_translation_shifts_with_object() {
        let spatial = spatial_with_one_leaf(
            triangle_mesh([-1.0, -2.0, -3.0], [3.0, 0.0, 0.0], [-1.0, 4.0, 5.0]),
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 20.0, 30.0])),
        );
        let p = pivot_for_leaf(&spatial, first_leaf(&spatial)).unwrap();
        // Local centre (1, 1, 1) translated by (10, 20, 30).
        assert_approx_eq!(f32, p.x, 11.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.y, 21.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.z, 31.0, epsilon = 1e-5);
    }

    #[test]
    fn pivot_for_empty_mesh_is_none() {
        let spatial = spatial_with_one_leaf(
            IndexedMesh::new(None, vec![], vec![]),
            Transform3D::Identity,
        );
        assert!(pivot_for_leaf(&spatial, first_leaf(&spatial)).is_none());
    }

    #[test]
    fn pivot_for_non_leaf_node_is_none() {
        // The Group root introduced by `spatial_with_one_leaf` is not a leaf;
        // SceneSpatial::leaf rejects it, and pivot_for_leaf inherits that
        // rejection.
        let spatial = spatial_with_one_leaf(
            triangle_mesh([-1.0, -2.0, -3.0], [3.0, 0.0, 0.0], [-1.0, 4.0, 5.0]),
            Transform3D::Identity,
        );
        let root_id = spatial.graph().roots()[0];
        assert!(pivot_for_leaf(&spatial, root_id).is_none());
    }

    #[test]
    fn pivot_for_mesh_with_nan_vertex_is_none() {
        // Every vertex carries NaN on the same axis so the AABB on that axis
        // stays NaN (f32::min and f32::max silently drop a NaN operand when
        // paired with a finite one, so a single bad vertex would not reach the
        // pivot).
        let spatial = spatial_with_one_leaf(
            triangle_mesh(
                [f32::NAN, 0.0, 0.0],
                [f32::NAN, 1.0, 0.0],
                [f32::NAN, 0.0, 1.0],
            ),
            Transform3D::Identity,
        );
        assert!(pivot_for_leaf(&spatial, first_leaf(&spatial)).is_none());
    }

    #[test]
    fn pivot_for_mesh_with_pos_inf_vertex_is_none() {
        let spatial = spatial_with_one_leaf(
            triangle_mesh([f32::INFINITY, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            Transform3D::Identity,
        );
        assert!(pivot_for_leaf(&spatial, first_leaf(&spatial)).is_none());
    }

    #[test]
    fn pivot_for_mesh_with_neg_inf_vertex_is_none() {
        let spatial = spatial_with_one_leaf(
            triangle_mesh(
                [0.0, 0.0, 0.0],
                [1.0, f32::NEG_INFINITY, 0.0],
                [0.0, 1.0, 0.0],
            ),
            Transform3D::Identity,
        );
        assert!(pivot_for_leaf(&spatial, first_leaf(&spatial)).is_none());
    }

    #[test]
    fn pivot_for_finite_mesh_with_non_finite_transform_is_none() {
        // The local AABB centre is finite, but the affine transform pushes the
        // result to non-finite. The guard has to catch the post-transform
        // value, not just the local one.
        let non_finite_translation = AffineTransform3D::from_translation([f64::INFINITY, 0.0, 0.0]);
        let spatial = spatial_with_one_leaf(
            triangle_mesh([-1.0, -2.0, -3.0], [3.0, 0.0, 0.0], [-1.0, 4.0, 5.0]),
            Transform3D::Affine(non_finite_translation),
        );
        assert!(pivot_for_leaf(&spatial, first_leaf(&spatial)).is_none());
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    use crate::camera::AxisView;
    use crate::picking;

    fn cam_at_distance(d: f32) -> OrbitCamera {
        let mut c = OrbitCamera::new(Vec3::ZERO, 1.0);
        c.distance = d;
        c
    }

    #[test]
    fn gizmo_scale_grows_with_camera_distance() {
        let near = gizmo_scale(&cam_at_distance(2.0), Vec3::ZERO, 1.0, 480.0).unwrap();
        let far = gizmo_scale(&cam_at_distance(20.0), Vec3::ZERO, 1.0, 480.0).unwrap();
        // 10 times farther gives 10 times more world units per pixel, so 10
        // times the scale.
        assert!(far > near * 9.0 && far < near * 11.0);
    }

    #[test]
    fn gizmo_scale_shrinks_with_viewport_height() {
        let small_h = gizmo_scale(&cam_at_distance(5.0), Vec3::ZERO, 1.0, 240.0).unwrap();
        let big_h = gizmo_scale(&cam_at_distance(5.0), Vec3::ZERO, 1.0, 1080.0).unwrap();
        assert!(big_h < small_h);
    }

    #[test]
    fn hit_radius_spans_handle_pixel_radius_off_the_orbit_target_plane() {
        // Front view from (0, -3, 0). The pivot is 1 unit in front of the plane
        // through the orbit target.
        let mut camera = OrbitCamera::new(Vec3::ZERO, 1.0);
        camera.set_axis_view(AxisView::Front);
        let (w, h) = (1024.0, 768.0);
        let pivot = Vec3::new(0.0, -1.0, 0.0);
        let radius = hit_radius_world(&camera, pivot, 0.8, h).unwrap();

        let (x0, _) = picking::project_world_to_screen(&camera, 0.8, w, h, pivot).unwrap();
        let rim = pivot + Vec3::X * radius;
        let (x1, _) = picking::project_world_to_screen(&camera, 0.8, w, h, rim).unwrap();
        assert_approx_eq!(f32, x1 - x0, HANDLE_PIXEL_RADIUS, epsilon = 1e-2);
    }

    #[test]
    fn scale_and_hit_radius_are_none_for_a_pivot_behind_the_eye() {
        let camera = cam_at_distance(5.0);
        let behind = camera.eye() + (camera.eye() - camera.target);
        assert!(gizmo_scale(&camera, behind, 1.0, 480.0).is_none());
        assert!(hit_radius_world(&camera, behind, 1.0, 480.0).is_none());
    }
}
