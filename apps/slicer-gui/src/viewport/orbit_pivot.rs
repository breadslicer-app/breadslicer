/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! Where a camera orbit turns (#346). A drag orbits around what the user is
//! working on rather than around the camera's own target:
//!
//! 1. the selection, at the centre of its world bounding box;
//! 2. else the model surface under the cursor where the drag started;
//! 3. else the plate under the cursor;
//! 4. else the point under the cursor at the camera's focus distance (the
//!    plane through the target), so empty space still turns about a point in
//!    front of the camera.
//!
//! The pivot is chosen once per drag and then held, so it does not slide on
//! screen while the camera turns ([`OrbitCamera::orbit_around`]).

use glam::Vec3;
use sc_mesh_spatial::SceneSpatial;

use super::types::{ScreenPoint, ViewportSize};
use crate::build_volume::BuildVolume;
use crate::camera::OrbitCamera;
use crate::mesh_render::default_fov_y;
use crate::picking::{self, Ray};
use crate::scene_document::SceneDocument;

/// Which rule picked the pivot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PivotSource {
    Selection,
    Surface,
    Plate,
    Focus,
}

/// Chooses the pivot of an orbit that starts with the cursor at `cursor`.
pub(super) fn choose(
    document: &SceneDocument,
    camera: &OrbitCamera,
    viewport: ViewportSize,
    build_volume: BuildVolume,
    cursor: ScreenPoint,
) -> (Vec3, PivotSource) {
    if let Some(centre) = selection_bounds(document)
        .map(|(min, max)| (min + max) * 0.5)
        .filter(|&centre| in_front(camera, centre))
    {
        return (centre, PivotSource::Selection);
    }
    let ray = picking::ray_from_pixel(
        camera,
        default_fov_y(),
        viewport.width(),
        viewport.height(),
        cursor.x,
        cursor.y,
    );
    if let Some(t) = document
        .with_scene(|spatial| surface_t(&ray, spatial))
        .flatten()
    {
        return (ray.origin + ray.direction * t, PivotSource::Surface);
    }
    if let Some(point) = plate_hit(&ray, build_volume) {
        return (point, PivotSource::Plate);
    }
    (focus_point(camera, &ray), PivotSource::Focus)
}

/// The world bounding box of everything selected, or `None` with nothing
/// selected (or nothing measurable).
fn selection_bounds(document: &SceneDocument) -> Option<(Vec3, Vec3)> {
    // One leaf today. A multi-selection only has to yield more ids here; the
    // union below already spans them all.
    let ids = document.selection_primary_node_id().into_iter();
    document
        .with_scene(|spatial| {
            ids.filter_map(|id| {
                let query = spatial.leaf(id)?;
                let aabb = query.transformed_aabb(query.world_transform())?;
                Some((
                    Vec3::from(aabb.min().coords()),
                    Vec3::from(aabb.max().coords()),
                ))
            })
            .reduce(|(lo, hi), (min, max)| (lo.min(min), hi.max(max)))
        })
        .flatten()
}

/// The ray parameter of the nearest model surface the ray meets.
fn surface_t(ray: &Ray, spatial: &SceneSpatial) -> Option<f32> {
    picking::pick_object_where(ray, spatial, |_| true).map(|(_, t)| t)
}

/// Where the ray meets the plate, when it lands inside the plate's outline.
fn plate_hit(ray: &Ray, build_volume: BuildVolume) -> Option<Vec3> {
    let (min, max) = (
        build_volume.envelope_min_mm(),
        build_volume.envelope_max_mm(),
    );
    let (x, y) = picking::ray_z_plane_intersection(ray, min.z)?;
    (min.x <= x && x <= max.x && min.y <= y && y <= max.y).then_some(Vec3::new(x, y, min.z))
}

/// The point on the ray at the camera's focus distance: on the plane through
/// the target that faces the camera.
fn focus_point(camera: &OrbitCamera, ray: &Ray) -> Vec3 {
    let forward = (camera.target - camera.eye()).normalize_or_zero();
    let depth = camera.distance - (ray.origin - camera.eye()).dot(forward);
    ray.origin + ray.direction * (depth / ray.direction.dot(forward).max(1e-6))
}

fn in_front(camera: &OrbitCamera, point: Vec3) -> bool {
    (point - camera.eye()).dot(camera.target - camera.eye()) > 0.0
}

#[cfg(test)]
mod orbit_pivot_tests {
    use std::rc::Rc;

    use super::*;
    use crate::camera::AxisView;
    use crate::selection::Selection;
    use crate::test_support::scenes::cube_scene;

    const WIDTH: f32 = 400.0;
    const HEIGHT: f32 = 300.0;

    fn viewport() -> ViewportSize {
        ViewportSize::new(WIDTH, HEIGHT).unwrap()
    }

    fn centre() -> ScreenPoint {
        ScreenPoint::new(WIDTH * 0.5, HEIGHT * 0.5)
    }

    fn plate() -> BuildVolume {
        BuildVolume::new(100.0, 100.0, 100.0)
    }

    /// The unit cube about the origin, looked at straight down from above.
    fn cube_from_above(ortho: bool) -> (Rc<SceneDocument>, OrbitCamera) {
        let document = SceneDocument::new();
        document.install_graph(cube_scene(), std::path::PathBuf::from("<test>"));
        let mut camera = OrbitCamera::new(Vec3::ZERO, 20.0);
        camera.set_axis_view(AxisView::Top);
        camera.orthographic = ortho;
        (document, camera)
    }

    fn select_the_cube(document: &SceneDocument) {
        let id = document
            .with_scene(|s| s.leaf_id_at_index(0))
            .flatten()
            .unwrap();
        document.set_selection(Selection::One(crate::selection::LeafId(id)));
    }

    #[test]
    fn the_selection_wins_wherever_the_cursor_is() {
        for ortho in [false, true] {
            let (document, camera) = cube_from_above(ortho);
            select_the_cube(&document);
            let corner = ScreenPoint::new(5.0, 5.0);
            let (pivot, source) = choose(&document, &camera, viewport(), plate(), corner);
            assert_eq!(source, PivotSource::Selection);
            assert!(pivot.length() < 1e-4, "box centre, got {pivot}");
        }
    }

    #[test]
    fn without_a_selection_the_surface_under_the_cursor() {
        for ortho in [false, true] {
            let (document, camera) = cube_from_above(ortho);
            let (pivot, source) = choose(&document, &camera, viewport(), plate(), centre());
            assert_eq!(source, PivotSource::Surface);
            // The cube's top face is at z = 0.5.
            assert!(
                (pivot - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-3,
                "{pivot}"
            );
        }
    }

    #[test]
    fn beside_the_model_the_plate_under_the_cursor() {
        for ortho in [false, true] {
            let (document, camera) = cube_from_above(ortho);
            let beside = ScreenPoint::new(WIDTH * 0.5 + 60.0, HEIGHT * 0.5);
            let (pivot, source) = choose(&document, &camera, viewport(), plate(), beside);
            assert_eq!(source, PivotSource::Plate);
            assert!(pivot.z.abs() < 1e-4 && pivot.x > 1.0, "{pivot}");
        }
    }

    #[test]
    fn over_empty_space_a_point_at_the_focus_distance() {
        for ortho in [false, true] {
            let document = SceneDocument::new();
            let mut camera = OrbitCamera::new(Vec3::new(0.0, 0.0, 30.0), 20.0);
            camera.set_axis_view(AxisView::Front);
            camera.orthographic = ortho;
            // Level with the plate the ray never comes down to it.
            let up_high = ScreenPoint::new(WIDTH * 0.5 + 40.0, 10.0);
            let (pivot, source) = choose(&document, &camera, viewport(), plate(), up_high);
            assert_eq!(source, PivotSource::Focus);
            let forward = (camera.target - camera.eye()).normalize();
            let depth = (pivot - camera.eye()).dot(forward);
            assert!((depth - camera.distance).abs() < 1e-3, "depth {depth}");
            let on_screen =
                picking::project_world_to_screen(&camera, default_fov_y(), WIDTH, HEIGHT, pivot)
                    .unwrap();
            assert!((on_screen.0 - up_high.x).abs() < 0.05);
            assert!((on_screen.1 - up_high.y).abs() < 0.05);
        }
    }

    #[test]
    fn a_selection_behind_the_camera_falls_back_to_the_cursor() {
        let (document, mut camera) = cube_from_above(false);
        select_the_cube(&document);
        // Sunk below the plate, still looking down: the cube is behind the eye.
        camera.target = Vec3::new(0.0, 0.0, -1.0e3);
        camera.distance = 10.0;
        let (_, source) = choose(&document, &camera, viewport(), plate(), centre());
        assert_ne!(source, PivotSource::Selection);
    }
}
