/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! A rotate drag turns evenly for a circle drawn on screen, whatever the
//! tilt of the camera: the gizmo ring and the modal R measure the turn on
//! screen, around the projected pivot.

use glam::Vec3;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, Scene};
use std::sync::Arc;

use super::controller::ViewportController;
use super::types::ScreenPoint;
use crate::PointerModifiers;
use crate::camera::OrbitCamera;
use crate::gizmo;
use crate::mesh_render::default_fov_y;
use crate::picking;
use crate::selection::{LeafId, Selection};

const WIDTH: f32 = 800.0;
const HEIGHT: f32 = 600.0;
/// Each step of the circle drawn on screen, in degrees.
const STEP_DEG: f32 = 10.0;

/// A selected cube, seen from `elevation_deg` above the plate.
fn tilted(elevation_deg: f32) -> (ViewportController, Vec3) {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 5.0, 0.0])),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("a fresh root");
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(WIDTH, HEIGHT);
    let pivot = ctrl
        .document()
        .with_scene(|s| gizmo::pivot_for_leaf(s, s.leaf_id_at_index(0)?))
        .flatten()
        .expect("a pivot");
    let mut camera = OrbitCamera::new(pivot, 8.0);
    camera.yaw = -0.7;
    camera.pitch = elevation_deg.to_radians();
    ctrl.set_camera_for_test(camera);
    let id = ctrl
        .document()
        .with_scene(|s| s.leaf_id_at_index(0))
        .flatten()
        .unwrap();
    ctrl.document().set_selection(Selection::One(LeafId(id)));
    (ctrl, pivot)
}

fn screen_of(ctrl: &ViewportController, point: Vec3) -> (f32, f32) {
    let camera = ctrl.camera_for_test().expect("a camera");
    picking::project_world_to_screen(&camera, default_fov_y(), WIDTH, HEIGHT, point)
        .expect("in front of the camera")
}

/// The object's turn about Z, in degrees.
fn z_turn(ctrl: &ViewportController) -> f32 {
    ctrl.document()
        .with_scene(
            |s| match s.leaf(s.leaf_id_at_index(0)?)?.local_transform() {
                Transform3D::Identity => Some(0.0),
                Transform3D::Affine(a) => {
                    let m = a.linear();
                    Some((m[1][0].atan2(m[0][0]) as f32).to_degrees())
                }
            },
        )
        .flatten()
        .expect("the leaf")
}

/// The front point of the Z ring on screen, where a press grabs it.
fn z_ring_front(ctrl: &ViewportController, pivot: Vec3) -> (f32, f32) {
    let camera = ctrl.camera_for_test().unwrap();
    let scale = gizmo::gizmo_scale(&camera, pivot, default_fov_y(), HEIGHT).unwrap();
    let toward = camera.eye() - pivot;
    let flat = Vec3::new(toward.x, toward.y, 0.0).normalize();
    screen_of(
        ctrl,
        pivot + flat * scale * gizmo::dimensions::ROTATE_ARC_RADIUS_FRACTION,
    )
}

/// Points on the screen circle around `centre` through `start`, `STEP_DEG`
/// apart, for `steps` steps.
fn circle(centre: (f32, f32), start: (f32, f32), steps: usize) -> Vec<(f32, f32)> {
    let (dx, dy) = (start.0 - centre.0, start.1 - centre.1);
    let r = dx.hypot(dy);
    let a0 = dy.atan2(dx);
    (1..=steps)
        .map(|k| {
            let a = a0 + (STEP_DEG * k as f32).to_radians();
            (centre.0 + r * a.cos(), centre.1 + r * a.sin())
        })
        .collect()
}

fn assert_even(turns: &[f32]) {
    let mut last = 0.0;
    for (k, &t) in turns.iter().enumerate() {
        let step = (t - last).abs();
        assert!(
            (step - STEP_DEG).abs() < 0.05,
            "step {k} turned {step}° instead of {STEP_DEG}°: {turns:?}"
        );
        last = t;
    }
}

#[test]
fn a_circle_on_screen_turns_the_gizmo_ring_evenly_on_a_tilted_camera() {
    for elevation in [60.0, 30.0] {
        let (ctrl, pivot) = tilted(elevation);
        let press = z_ring_front(&ctrl, pivot);
        ctrl.on_pointer_down(
            ScreenPoint::new(press.0, press.1),
            PointerModifiers::default(),
        );
        let centre = screen_of(&ctrl, pivot);
        let turns: Vec<f32> = circle(centre, press, 8)
            .into_iter()
            .map(|(x, y)| {
                ctrl.on_pointer_move(ScreenPoint::new(x, y), PointerModifiers::default());
                z_turn(&ctrl)
            })
            .collect();
        assert_even(&turns);
        // Seen from above, a clockwise circle on screen turns clockwise from
        // above: negative about +Z.
        assert!(turns[0] < 0.0, "{turns:?}");

        // One undo step takes the whole drag back.
        let end = circle(centre, press, 8)[7];
        ctrl.on_pointer_up(ScreenPoint::new(end.0, end.1), PointerModifiers::default());
        assert!(ctrl.document().undo().expect("undo"));
        assert!(z_turn(&ctrl).abs() < 1e-3, "elevation {elevation}");
    }
}

#[test]
fn a_circle_on_screen_turns_modal_r_evenly_on_a_tilted_camera() {
    for elevation in [60.0, 30.0] {
        let (ctrl, pivot) = tilted(elevation);
        let centre = screen_of(&ctrl, pivot);
        let press = (centre.0 + 90.0, centre.1 + 40.0);
        ctrl.on_modal_transform_start(1, press.0, press.1);
        ctrl.on_modal_transform_axis(2, press.0, press.1);
        let turns: Vec<f32> = circle(centre, press, 8)
            .into_iter()
            .map(|(x, y)| {
                ctrl.on_pointer_move(ScreenPoint::new(x, y), PointerModifiers::default());
                z_turn(&ctrl)
            })
            .collect();
        assert_even(&turns);
        ctrl.on_modal_transform_finish(false);
        assert!(z_turn(&ctrl).abs() < 1e-3);
    }
}

#[test]
fn shift_mid_turn_slows_it_without_a_jump() {
    let (ctrl, pivot) = tilted(60.0);
    let press = z_ring_front(&ctrl, pivot);
    let centre = screen_of(&ctrl, pivot);
    let shift = PointerModifiers {
        shift: true,
        ..PointerModifiers::default()
    };
    ctrl.on_pointer_down(
        ScreenPoint::new(press.0, press.1),
        PointerModifiers::default(),
    );
    let path = circle(centre, press, 2);
    ctrl.on_pointer_move(
        ScreenPoint::new(path[0].0, path[0].1),
        PointerModifiers::default(),
    );
    let before = z_turn(&ctrl);
    // Shift goes down where the cursor is: nothing moves.
    ctrl.on_pointer_move(ScreenPoint::new(path[0].0, path[0].1), shift.clone());
    assert!((z_turn(&ctrl) - before).abs() < 1e-4);
    // The next 10° on screen turn about a tenth as far.
    ctrl.on_pointer_move(ScreenPoint::new(path[1].0, path[1].1), shift);
    let fine = (z_turn(&ctrl) - before).abs();
    assert!(fine > 0.5 && fine < 1.5, "{fine}°");
}

#[test]
fn ctrl_snaps_the_screen_turn_to_fifteen_degrees() {
    let (ctrl, pivot) = tilted(60.0);
    let press = z_ring_front(&ctrl, pivot);
    let centre = screen_of(&ctrl, pivot);
    let ctrl_held = PointerModifiers {
        control: true,
        ..PointerModifiers::default()
    };
    ctrl.on_pointer_down(
        ScreenPoint::new(press.0, press.1),
        PointerModifiers::default(),
    );
    // Two steps of 10° on screen: 20° snaps to 15°.
    let to = circle(centre, press, 2)[1];
    ctrl.on_pointer_move(ScreenPoint::new(to.0, to.1), ctrl_held);
    assert!(
        (z_turn(&ctrl).abs() - 15.0).abs() < 1e-3,
        "{}",
        z_turn(&ctrl)
    );
}

#[test]
fn cutting_the_ring_plane_would_turn_unevenly_on_a_tilted_camera() {
    // Why the turn is measured on screen: the angle where the cursor ray cuts
    // the plane of a tilted ring runs fast and slow around a circle drawn on
    // screen (the ring reads as an ellipse).
    let (ctrl, pivot) = tilted(30.0);
    let camera = ctrl.camera_for_test().unwrap();
    let centre = screen_of(&ctrl, pivot);
    let press = z_ring_front(&ctrl, pivot);
    let angle = |(x, y): (f32, f32)| {
        let ray = picking::ray_from_pixel(&camera, default_fov_y(), WIDTH, HEIGHT, x, y);
        gizmo::drag_math::rotate_drag_angle(&ray, pivot, gizmo::axis::Axis::Z)
            .unwrap()
            .to_degrees()
    };
    let mut last = angle(press);
    let steps: Vec<f32> = circle(centre, press, 9)
        .into_iter()
        .map(|p| {
            let a = angle(p);
            let step = (a - last + 540.0).rem_euclid(360.0) - 180.0;
            last = a;
            step.abs()
        })
        .collect();
    let (lo, hi) = steps
        .iter()
        .fold((f32::MAX, 0.0f32), |(lo, hi), &s| (lo.min(s), hi.max(s)));
    assert!(hi - lo > 5.0, "{steps:?}");
}
