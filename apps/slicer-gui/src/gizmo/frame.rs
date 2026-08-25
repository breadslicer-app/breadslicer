/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-frame `GizmoFrame` assembly for the renderer. Bridges the
//! controller's per-frame state (pivot, scale, visibility,
//! lock-out mask) into the renderer's struct.

use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::camera::OrbitCamera;
use crate::mesh_render::{GizmoDragVisual, GizmoFrame, GizmoHover, GizmoVisibility, default_fov_y};
use crate::picking;

use super::axis::ALL_AXES;
use super::hit_test::{arc_pickable, axis_pickable};
use super::{gizmo_scale, pivot_for_leaf};

/// Builds the per-frame `GizmoFrame` for the renderer when a single
/// leaf is selected. Returns `None` if the leaf has no geometry (no
/// pivot to attach to) or `selection` does not name a reachable
/// leaf. The locked-axes mask uses the camera's forward direction as
/// a single representative ray: for a small gizmo near the screen
/// centre, all rays through the gizmo region are within a degree or
/// so of the camera forward, so a per-frame "is this axis grabbable
/// from this view" check is uniform across the handle.
pub(crate) fn build_gizmo_frame(
    spatial: &SceneSpatial,
    camera: &OrbitCamera,
    selection: NodeId,
    viewport_h_logical: f32,
    visibility: GizmoVisibility,
    hover: Option<GizmoHover>,
    drag_visual: Option<GizmoDragVisual>,
) -> Option<GizmoFrame> {
    let pivot = pivot_for_leaf(spatial, selection)?;
    let fov_y = default_fov_y();
    let scale = gizmo_scale(camera, fov_y, viewport_h_logical);
    let forward = (camera.target - camera.eye()).normalize_or_zero();
    let view_ray = picking::Ray {
        origin: camera.eye(),
        direction: forward,
    };
    let locked_translate = ALL_AXES.map(|a| !axis_pickable(&view_ray, a));
    let locked_rotate = ALL_AXES.map(|a| !arc_pickable(&view_ray, a));
    Some(GizmoFrame {
        pivot,
        scale,
        visibility,
        locked_translate,
        locked_rotate,
        hover,
        drag_visual,
    })
}

#[cfg(test)]
mod build_gizmo_frame_tests {
    use std::sync::Arc;

    use float_cmp::assert_approx_eq;
    use glam::Vec3;

    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_scene::{LeafPayload, Scene};

    use crate::camera::{AxisView, OrbitCamera};

    use super::super::axis::Axis;
    use super::*;

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

    fn unit_triangle_spatial() -> SceneSpatial {
        spatial_with_one_leaf(
            triangle_mesh([-1.0, -2.0, -3.0], [3.0, 0.0, 0.0], [-1.0, 4.0, 5.0]),
            Transform3D::Identity,
        )
    }

    fn first_leaf(spatial: &SceneSpatial) -> NodeId {
        spatial
            .leaf_id_at_index(0)
            .expect("fixture has a leaf at idx 0")
    }

    fn default_camera() -> OrbitCamera {
        OrbitCamera::new(Vec3::ZERO, 1.0)
    }

    #[test]
    fn returns_none_when_selection_is_not_a_leaf() {
        // The Group root introduced by spatial_with_one_leaf is not a
        // leaf, so pivot_for_leaf returns None and build_gizmo_frame
        // propagates that.
        let spatial = unit_triangle_spatial();
        let root_id = spatial.graph().roots()[0];

        let frame = build_gizmo_frame(
            &spatial,
            &default_camera(),
            root_id,
            600.0,
            GizmoVisibility::Idle,
            None,
            None,
        );
        assert!(frame.is_none());
    }

    #[test]
    fn returns_none_when_leaf_has_no_geometry() {
        let spatial = spatial_with_one_leaf(
            IndexedMesh::new(None, vec![], vec![]),
            Transform3D::Identity,
        );

        let frame = build_gizmo_frame(
            &spatial,
            &default_camera(),
            first_leaf(&spatial),
            600.0,
            GizmoVisibility::Idle,
            None,
            None,
        );
        assert!(frame.is_none());
    }

    #[test]
    fn pivot_matches_pivot_for_leaf_under_translation() {
        // local AABB = ([-1, -2, -3], [3, 4, 5]) -> centre (1, 1, 1);
        // translated by (10, 20, 30) -> world centre (11, 21, 31).
        let spatial = spatial_with_one_leaf(
            triangle_mesh([-1.0, -2.0, -3.0], [3.0, 0.0, 0.0], [-1.0, 4.0, 5.0]),
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 20.0, 30.0])),
        );

        let frame = build_gizmo_frame(
            &spatial,
            &default_camera(),
            first_leaf(&spatial),
            600.0,
            GizmoVisibility::Idle,
            None,
            None,
        )
        .expect("frame produced for valid leaf");

        assert_approx_eq!(f32, frame.pivot.x, 11.0, epsilon = 1e-5);
        assert_approx_eq!(f32, frame.pivot.y, 21.0, epsilon = 1e-5);
        assert_approx_eq!(f32, frame.pivot.z, 31.0, epsilon = 1e-5);
    }

    #[test]
    fn scale_matches_gizmo_scale_at_default_fov_y() {
        // build_gizmo_frame must call gizmo_scale with the renderer's
        // default_fov_y so the on-screen pixel length stays constant
        // across cameras. Pin the wiring by reproducing the same call.
        let spatial = unit_triangle_spatial();
        let camera = default_camera();
        let viewport_h = 720.0;
        let expected = gizmo_scale(&camera, default_fov_y(), viewport_h);

        let frame = build_gizmo_frame(
            &spatial,
            &camera,
            first_leaf(&spatial),
            viewport_h,
            GizmoVisibility::Idle,
            None,
            None,
        )
        .expect("frame produced for valid leaf");

        assert_approx_eq!(f32, frame.scale, expected, epsilon = 1e-6);
    }

    fn camera_looking_down_x() -> OrbitCamera {
        let mut camera = default_camera();
        camera.set_axis_view(AxisView::Right);
        camera
    }

    #[test]
    fn locked_translate_marks_axis_parallel_to_view_ray() {
        // Right view: yaw=0, pitch=0 -> eye at (+d, 0, 0), forward = -X.
        // axis_pickable rejects X (|X . -X| = 1, above LOCKOUT_COS);
        // Y and Z stay grabbable.
        let spatial = unit_triangle_spatial();
        let camera = camera_looking_down_x();

        let frame = build_gizmo_frame(
            &spatial,
            &camera,
            first_leaf(&spatial),
            600.0,
            GizmoVisibility::Idle,
            None,
            None,
        )
        .expect("frame produced for valid leaf");

        assert_eq!(frame.locked_translate, [true, false, false]);
    }

    #[test]
    fn locked_rotate_marks_axes_perpendicular_to_view_ray() {
        // Same Right-view camera. arc_pickable demands |axis . forward|
        // >= ROTATE_LOCKOUT_COS; only X meets that (axis parallel to
        // ray), so Y and Z arcs are locked (edge-on).
        let spatial = unit_triangle_spatial();
        let camera = camera_looking_down_x();

        let frame = build_gizmo_frame(
            &spatial,
            &camera,
            first_leaf(&spatial),
            600.0,
            GizmoVisibility::Idle,
            None,
            None,
        )
        .expect("frame produced for valid leaf");

        assert_eq!(frame.locked_rotate, [false, true, true]);
    }

    #[test]
    fn visibility_hover_and_drag_visual_pass_through_unchanged() {
        let spatial = unit_triangle_spatial();
        let visibility = GizmoVisibility::DraggingScaleAxis { axis: Axis::Y };
        let hover = Some(GizmoHover::Rotate(Axis::Z));
        let drag_visual = Some(GizmoDragVisual::Rotate {
            axis: Axis::X,
            angle: 1.5,
        });

        let frame = build_gizmo_frame(
            &spatial,
            &default_camera(),
            first_leaf(&spatial),
            600.0,
            visibility,
            hover,
            drag_visual,
        )
        .expect("frame produced for valid leaf");

        assert_eq!(frame.visibility, visibility);
        assert_eq!(frame.hover, hover);
        // GizmoDragVisual has no PartialEq; destructure to check the
        // payload survived the round trip.
        match frame.drag_visual {
            Some(GizmoDragVisual::Rotate { axis, angle }) => {
                assert_eq!(axis, Axis::X);
                assert_approx_eq!(f32, angle, 1.5);
            }
            other => panic!("expected Rotate drag visual, got {other:?}"),
        }
    }
}
