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
pub mod frame;
pub mod hit_test;
mod tolerances;

use glam::Vec3;
use sc_mesh_core::NodeId;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;

use crate::camera::OrbitCamera;

use self::dimensions::{HANDLE_PIXEL_LENGTH, HANDLE_PIXEL_RADIUS};

/// World-space pivot for the gizmo: the local-AABB centre of the
/// leaf, transformed by the leaf's world transform. Returns `None`
/// when the leaf has no vertices, an out-of-bounds resource
/// reference, or `node_id` does not name a reachable leaf; all of
/// which render the gizmo unattachable.
pub fn pivot_for_leaf(spatial: &SceneSpatial, node_id: NodeId) -> Option<Vec3> {
    let query = spatial.leaf(node_id)?;
    let aabb = query.local_aabb()?;
    let lmin = Vec3::from(aabb.min().coords());
    let lmax = Vec3::from(aabb.max().coords());
    let local_centre = (lmin + lmax) * 0.5;
    Some(match &query.world_transform() {
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
    use std::sync::Arc;

    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::{IndexedMesh, LeafPayload, Scene};

    fn triangle_mesh(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> IndexedMesh {
        IndexedMesh::new(
            None,
            vec![Vertex::new(v0), Vertex::new(v1), Vertex::new(v2)],
            vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        )
    }

    /// Single-leaf scene under a Group root, mirroring the shape the
    /// loader pipeline produces (one inline-mesh leaf attached under
    /// an identity-Group root).
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
        // local AABB = ([-1, -2, -3], [3, 4, 5]) → centre (1, 1, 1)
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
        // The Group root introduced by `spatial_with_one_leaf` is not
        // a leaf; SceneSpatial::leaf rejects it, and pivot_for_leaf
        // inherits that rejection.
        let spatial = spatial_with_one_leaf(
            triangle_mesh([-1.0, -2.0, -3.0], [3.0, 0.0, 0.0], [-1.0, 4.0, 5.0]),
            Transform3D::Identity,
        );
        let root_id = spatial.graph().roots()[0];
        assert!(pivot_for_leaf(&spatial, root_id).is_none());
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
