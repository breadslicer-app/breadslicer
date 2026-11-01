/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-frame `GizmoFrame` assembly for the renderer: computes the pivot, scale
//! and lock-out masks from the scene and camera, and passes the controller's
//! visibility, hover and drag state through.

use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::camera::OrbitCamera;
use crate::mesh_render::{GizmoDragVisual, GizmoFrame, GizmoHover, GizmoVisibility, default_fov_y};

use super::look::GizmoLook;
use super::{gizmo_scale_sized, pivot_for_leaf};

/// Builds the per-frame `GizmoFrame` for the renderer when a single leaf is
/// selected. Returns `None` when `pivot_for_leaf` gives no pivot or the pivot
/// is not in front of the eye.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_gizmo_frame(
    spatial: &SceneSpatial,
    camera: &OrbitCamera,
    selection: NodeId,
    viewport_h_logical: f32,
    visibility: GizmoVisibility,
    hover: Option<GizmoHover>,
    drag_visual: Option<GizmoDragVisual>,
    look: GizmoLook,
) -> Option<GizmoFrame> {
    let pivot = pivot_for_leaf(spatial, selection)?;
    build_gizmo_frame_at(
        pivot,
        camera,
        viewport_h_logical,
        visibility,
        hover,
        drag_visual,
        look,
    )
}

/// [`build_gizmo_frame`] for a gizmo standing at `pivot`: the shared centre
/// of several selected objects (#13). `None` when the pivot is not in front
/// of the eye.
pub(crate) fn build_gizmo_frame_at(
    pivot: glam::Vec3,
    camera: &OrbitCamera,
    viewport_h_logical: f32,
    visibility: GizmoVisibility,
    hover: Option<GizmoHover>,
    drag_visual: Option<GizmoDragVisual>,
    look: GizmoLook,
) -> Option<GizmoFrame> {
    let fov_y = default_fov_y();
    let scale = gizmo_scale_sized(camera, pivot, fov_y, viewport_h_logical, look.size)?;
    // The camera forward direction stands in for every ray through the gizmo
    // in the fades of the handles that face the camera: for a small gizmo
    // those rays are within a degree or so of it.
    let view_dir = (camera.target - camera.eye()).normalize_or_zero();
    Some(GizmoFrame {
        pivot,
        scale,
        view_dir,
        visibility,
        hover,
        drag_visual,
        look,
    })
}

#[cfg(test)]
mod build_gizmo_frame_tests {
    use std::sync::Arc;

    use float_cmp::assert_approx_eq;
    use glam::Vec3;

    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_scene::{LeafPayload, Scene};

    use crate::camera::{AxisView, OrbitCamera};
    use crate::picking;

    use super::super::axis::Axis;
    use super::super::dimensions::HANDLE_PIXEL_LENGTH;
    use super::*;

    fn triangle_mesh(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> IndexedMesh {
        IndexedMesh::new(
            None,
            vec![Vertex::new(v0), Vertex::new(v1), Vertex::new(v2)],
            vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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
        // The Group root introduced by spatial_with_one_leaf is not a leaf, so
        // pivot_for_leaf returns None and build_gizmo_frame propagates that.
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
            GizmoLook::default(),
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
            GizmoLook::default(),
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
            GizmoLook::default(),
        )
        .expect("frame produced for valid leaf");

        assert_approx_eq!(f32, frame.pivot.x, 11.0, epsilon = 1e-5);
        assert_approx_eq!(f32, frame.pivot.y, 21.0, epsilon = 1e-5);
        assert_approx_eq!(f32, frame.pivot.z, 31.0, epsilon = 1e-5);
    }

    #[test]
    fn the_gizmo_size_scales_the_frame() {
        let spatial = unit_triangle_spatial();
        let camera = default_camera();
        let at = |size: f32| {
            build_gizmo_frame(
                &spatial,
                &camera,
                first_leaf(&spatial),
                600.0,
                GizmoVisibility::Idle,
                None,
                None,
                GizmoLook {
                    size,
                    ..GizmoLook::default()
                },
            )
            .unwrap()
            .scale
        };
        assert_approx_eq!(f32, at(1.5), at(1.0) * 1.5, epsilon = 1e-6);
    }

    #[test]
    fn scale_matches_gizmo_scale_at_default_fov_y() {
        // build_gizmo_frame must call gizmo_scale with the renderer's
        // default_fov_y so the on-screen pixel length stays constant across
        // cameras. Pin the wiring by reproducing the same call.
        let spatial = unit_triangle_spatial();
        let camera = default_camera();
        let viewport_h = 720.0;
        let pivot = pivot_for_leaf(&spatial, first_leaf(&spatial)).unwrap();
        let expected =
            super::super::gizmo_scale(&camera, pivot, default_fov_y(), viewport_h).unwrap();

        let frame = build_gizmo_frame(
            &spatial,
            &camera,
            first_leaf(&spatial),
            viewport_h,
            GizmoVisibility::Idle,
            None,
            None,
            GizmoLook::default(),
        )
        .expect("frame produced for valid leaf");

        assert_approx_eq!(f32, frame.scale, expected, epsilon = 1e-6);
    }

    /// Returns a 1-leaf scene whose leaf has its local AABB centred on the
    /// origin and is translated so that its pivot lands on `pivot`.
    fn spatial_with_pivot_at(pivot: [f64; 3]) -> SceneSpatial {
        spatial_with_one_leaf(
            triangle_mesh([-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [0.0, 0.0, 1.0]),
            Transform3D::Affine(AffineTransform3D::from_translation(pivot)),
        )
    }

    /// Returns a camera with its eye on the -Y axis, looking along +Y at the
    /// origin.
    fn front_camera_at_distance(distance: f32) -> OrbitCamera {
        let mut camera = default_camera();
        camera.set_axis_view(AxisView::Front);
        camera.distance = distance;
        camera
    }

    #[test]
    fn shaft_spans_handle_pixel_length_off_the_orbit_target_plane() {
        // The X shaft is parallel to the image plane, so its projected length
        // is exact. Pivots at y = -1 and y = +1 are in front of and behind the
        // plane through the orbit target.
        let (w, h) = (1024.0, 768.0);
        let fov_y = default_fov_y();
        for pivot_y in [-1.0, 1.0] {
            let spatial = spatial_with_pivot_at([0.0, pivot_y, 0.0]);
            for distance in [3.0, 1.5] {
                let camera = front_camera_at_distance(distance);
                let frame = build_gizmo_frame(
                    &spatial,
                    &camera,
                    first_leaf(&spatial),
                    h,
                    GizmoVisibility::Idle,
                    None,
                    None,
                    GizmoLook::default(),
                )
                .expect("pivot is in front of the eye");

                let (x0, y0) =
                    picking::project_world_to_screen(&camera, fov_y, w, h, frame.pivot).unwrap();
                let tip = frame.pivot + Vec3::X * frame.scale;
                let (x1, y1) = picking::project_world_to_screen(&camera, fov_y, w, h, tip).unwrap();
                assert_approx_eq!(
                    f32,
                    (x1 - x0).hypot(y1 - y0),
                    HANDLE_PIXEL_LENGTH,
                    epsilon = 0.05
                );
            }
        }
    }

    #[test]
    fn returns_none_when_pivot_is_behind_the_eye() {
        // The eye is at y = -3.
        let spatial = spatial_with_pivot_at([0.0, -4.0, 0.0]);

        let frame = build_gizmo_frame(
            &spatial,
            &front_camera_at_distance(3.0),
            first_leaf(&spatial),
            600.0,
            GizmoVisibility::Idle,
            None,
            None,
            GizmoLook::default(),
        );
        assert!(frame.is_none());
    }

    fn camera_looking_down_x() -> OrbitCamera {
        let mut camera = default_camera();
        camera.set_axis_view(AxisView::Right);
        camera
    }

    #[test]
    fn view_dir_is_the_camera_forward() {
        // Right view: yaw=0, pitch=0 -> eye at (+d, 0, 0), forward = -X.
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
            GizmoLook::default(),
        )
        .expect("frame produced for valid leaf");

        assert!((frame.view_dir - Vec3::NEG_X).length() < 1e-5);
    }

    #[test]
    fn visibility_hover_and_drag_visual_pass_through_unchanged() {
        let spatial = unit_triangle_spatial();
        let visibility = GizmoVisibility::DraggingScaleAxis { axis: Axis::Y };
        let hover = Some(GizmoHover::Rotate(Axis::Z));
        let drag_visual = Some(GizmoDragVisual::Rotate {
            axis: Axis::X,
            start: 0.25,
            sweep: 1.5,
        });

        let frame = build_gizmo_frame(
            &spatial,
            &default_camera(),
            first_leaf(&spatial),
            600.0,
            visibility,
            hover,
            drag_visual,
            GizmoLook::default(),
        )
        .expect("frame produced for valid leaf");

        assert_eq!(frame.visibility, visibility);
        assert_eq!(frame.hover, hover);
        // GizmoDragVisual has no PartialEq; destructure to check the payload
        // survived the round trip.
        match frame.drag_visual {
            Some(GizmoDragVisual::Rotate { axis, start, sweep }) => {
                assert_eq!(axis, Axis::X);
                assert_approx_eq!(f32, start, 0.25);
                assert_approx_eq!(f32, sweep, 1.5);
            }
            other => panic!("expected Rotate drag visual, got {other:?}"),
        }
    }
}
