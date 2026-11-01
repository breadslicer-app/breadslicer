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
//!
//! The numpad turns the camera a fixed step about the same pivot (#348); a
//! key has no place on screen, so there it is the selection, else the
//! camera's target. A pivot off screen is marked on the viewport's edge,
//! pointing toward it (#349).

use glam::{Vec2, Vec3};
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
    if let Some(centre) = selection_centre(document, camera) {
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

/// One numpad orbit step: 15 degrees, as in Blender.
pub(super) const STEP: f32 = std::f32::consts::PI / 12.0;

/// How far inside the viewport's edge the marker of an off-screen pivot sits.
pub(super) const EDGE_INSET: f32 = 14.0;

/// The yaw and pitch of one numpad step. `horizontal` is -1 for left
/// (Numpad 4) and 1 for right (Numpad 6); `vertical` is 1 for up (Numpad 8)
/// and -1 for down (Numpad 2). The camera moves that way about the pivot.
pub(super) fn step_angles(horizontal: i32, vertical: i32) -> (f32, f32) {
    (
        horizontal.signum() as f32 * STEP,
        vertical.signum() as f32 * STEP,
    )
}

/// The pivot of a numpad step: the selection, else the camera's target.
pub(super) fn step_pivot(document: &SceneDocument, camera: &OrbitCamera) -> Vec3 {
    selection_centre(document, camera).unwrap_or(camera.target)
}

/// Where the pivot marker goes on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Marker {
    pub x: f32,
    pub y: f32,
    /// With the pivot off screen the marker sits on the edge and this is the
    /// screen direction toward the pivot, in degrees clockwise from the
    /// right (screen y grows downward). `None` on screen.
    pub toward: Option<f32>,
}

/// Places the marker of `pivot`: on it while it is on screen, else on the
/// viewport's edge, [`EDGE_INSET`] inside, where the line from the centre
/// toward the pivot leaves the view.
pub(super) fn marker(camera: &OrbitCamera, viewport: ViewportSize, pivot: Vec3) -> Marker {
    let (width, height) = (viewport.width(), viewport.height());
    let projected = picking::project_world_to_screen(camera, default_fov_y(), width, height, pivot);
    if let Some((x, y)) = projected
        && (0.0..=width).contains(&x)
        && (0.0..=height).contains(&y)
    {
        return Marker { x, y, toward: None };
    }
    let centre = Vec2::new(width, height) * 0.5;
    let direction = match projected {
        Some((x, y)) => Vec2::new(x, y) - centre,
        // Behind the camera a projection mirrors: take the side the pivot
        // lies on across the view instead.
        None => {
            let (right, up) = camera.right_and_up();
            let offset = pivot - camera.eye();
            let across = Vec2::new(offset.dot(right), -offset.dot(up));
            // Straight behind, any side will do: below.
            if across.length() > 1e-4 * offset.length() {
                across
            } else {
                Vec2::Y
            }
        }
    };
    let room = (centre - Vec2::splat(EDGE_INSET)).max(Vec2::ZERO);
    let reach = (room.x / direction.x.abs()).min(room.y / direction.y.abs());
    let at = centre + direction * reach;
    Marker {
        x: at.x,
        y: at.y,
        toward: Some(direction.y.atan2(direction.x).to_degrees()),
    }
}

/// The centre of the selection's bounding box, when something is selected
/// and it lies in front of the camera.
fn selection_centre(document: &SceneDocument, camera: &OrbitCamera) -> Option<Vec3> {
    selection_bounds(document)
        .map(|(min, max)| (min + max) * 0.5)
        .filter(|&centre| in_front(camera, centre))
}

/// The world bounding box of everything selected, or `None` with nothing
/// selected (or nothing measurable).
fn selection_bounds(document: &SceneDocument) -> Option<(Vec3, Vec3)> {
    // Every selected leaf (#13); the union below spans them all.
    let ids = document.selection_node_ids().into_iter();
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

    #[test]
    fn numpad_steps_move_the_camera_15_degrees_each_way() {
        for ortho in [false, true] {
            let pivot = Vec3::new(10.0, -5.0, 2.0);
            let mut camera = OrbitCamera::new(Vec3::ZERO, 20.0);
            camera.orthographic = ortho;
            for (h, v) in [(-1, 0), (1, 0), (0, 1), (0, -1)] {
                let mut turned = camera.clone();
                let (yaw, pitch) = step_angles(h, v);
                turned.turn_around(pivot, yaw, pitch);
                let (right, _) = camera.right_and_up();
                let moved = turned.eye() - camera.eye();
                if h != 0 {
                    assert_eq!(moved.dot(right).signum(), h as f32, "({h}, {v})");
                } else {
                    assert_eq!(moved.z.signum(), v as f32, "({h}, {v})");
                }
                // One step, and the eye swings rigidly about the pivot.
                let turn = (turned.yaw - camera.yaw, turned.pitch - camera.pitch);
                assert!(
                    (turn.0.abs() + turn.1.abs() - STEP).abs() < 1e-5,
                    "{turn:?}"
                );
                let (from, to) = (camera.eye() - pivot, turned.eye() - pivot);
                assert!((from.length() - to.length()).abs() < 1e-3);
                if h != 0 {
                    let angle = from.truncate().angle_to(to.truncate()).abs();
                    assert!((angle - STEP).abs() < 1e-4, "{angle} for ({h}, {v})");
                }
                let screen = |c: &OrbitCamera| {
                    picking::project_world_to_screen(c, default_fov_y(), WIDTH, HEIGHT, pivot)
                        .unwrap()
                };
                let (a, b) = (screen(&camera), screen(&turned));
                assert!((a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05);
            }
        }
    }

    #[test]
    fn a_step_turns_about_the_selection_else_the_target() {
        let (document, camera) = cube_from_above(false);
        let target = Vec3::new(3.0, 4.0, 0.0);
        let mut aside = camera.clone();
        aside.target = target;
        assert_eq!(step_pivot(&document, &aside), target);
        select_the_cube(&document);
        assert!(step_pivot(&document, &aside).length() < 1e-4);
        // A selection behind the camera is no pivot.
        aside.target = Vec3::new(0.0, 0.0, -1.0e3);
        aside.distance = 10.0;
        assert_eq!(step_pivot(&document, &aside), aside.target);
    }

    /// Looking along +Y from y = -60: +X is screen right, +Z screen up.
    fn front_camera(ortho: bool) -> OrbitCamera {
        let mut camera = OrbitCamera::new(Vec3::ZERO, 20.0);
        camera.set_axis_view(AxisView::Front);
        camera.orthographic = ortho;
        camera
    }

    fn assert_near(marker: Marker, x: f32, y: f32, toward: Option<f32>) {
        assert!(
            (marker.x - x).abs() < 0.5 && (marker.y - y).abs() < 0.5,
            "{marker:?} not at ({x}, {y})"
        );
        match (marker.toward, toward) {
            (Some(a), Some(b)) => assert!((a - b).abs() < 0.5, "{marker:?} toward {b}"),
            (a, b) => assert_eq!(a, b, "{marker:?}"),
        }
    }

    #[test]
    fn a_pivot_on_screen_is_marked_where_it_is() {
        for ortho in [false, true] {
            let camera = front_camera(ortho);
            let pivot = Vec3::new(1.0, 0.0, 1.0);
            let (x, y) =
                picking::project_world_to_screen(&camera, default_fov_y(), WIDTH, HEIGHT, pivot)
                    .unwrap();
            assert_near(marker(&camera, viewport(), pivot), x, y, None);
        }
    }

    #[test]
    fn a_pivot_off_screen_is_marked_on_the_nearest_edge() {
        let (w, h, inset) = (WIDTH, HEIGHT, EDGE_INSET);
        for ortho in [false, true] {
            let camera = front_camera(ortho);
            let at = |pivot| marker(&camera, viewport(), pivot);
            assert_near(
                at(Vec3::new(1.0e3, 0.0, 0.0)),
                w - inset,
                h / 2.0,
                Some(0.0),
            );
            assert_near(at(Vec3::new(-1.0e3, 0.0, 0.0)), inset, h / 2.0, Some(180.0));
            assert_near(at(Vec3::new(0.0, 0.0, 1.0e3)), w / 2.0, inset, Some(-90.0));
            assert_near(
                at(Vec3::new(0.0, 0.0, -1.0e3)),
                w / 2.0,
                h - inset,
                Some(90.0),
            );
            // Off a corner, along the line from the centre: the top edge for
            // this wide view, a little right of the centre.
            let corner = at(Vec3::new(1.0e3, 0.0, 1.0e3));
            assert!((corner.y - inset).abs() < 0.5, "{corner:?}");
            assert!(corner.x > w / 2.0 && corner.x <= w - inset, "{corner:?}");
            let toward = corner.toward.unwrap();
            assert!((-90.0..0.0).contains(&toward), "{corner:?}");
        }
    }

    #[test]
    fn a_pivot_behind_the_camera_is_marked_on_its_side() {
        let (w, h, inset) = (WIDTH, HEIGHT, EDGE_INSET);
        for ortho in [false, true] {
            let camera = front_camera(ortho);
            // The eye sits at y = -60; these lie behind it.
            let at = |pivot| marker(&camera, viewport(), pivot);
            assert_near(
                at(Vec3::new(5.0, -90.0, 0.0)),
                w - inset,
                h / 2.0,
                Some(0.0),
            );
            assert_near(at(Vec3::new(0.0, -90.0, 5.0)), w / 2.0, inset, Some(-90.0));
            // Straight behind: below.
            assert_near(
                at(Vec3::new(0.0, -90.0, 0.0)),
                w / 2.0,
                h - inset,
                Some(90.0),
            );
        }
    }
}
