/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! A pointer orbit turns about the selection or the point under the cursor
//! where it started, and that point holds still on screen (#346).

use glam::Vec3;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, Scene};
use std::sync::Arc;

use super::controller::ViewportController;
use super::types::{ScreenDelta, ScreenPoint};
use crate::camera::OrbitCamera;
use crate::mesh_render::default_fov_y;
use crate::picking;
use crate::selection::{LeafId, Selection};

const WIDTH: f32 = 400.0;
const HEIGHT: f32 = 300.0;

/// A small cube far from the plate centre, with the camera framed on the
/// plate centre in `ortho` or perspective.
fn far_cube(ortho: bool) -> ViewportController {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([40.0, 30.0, 0.5])),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("a fresh root");
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(WIDTH, HEIGHT);
    let mut camera = OrbitCamera::new(Vec3::ZERO, 60.0);
    camera.orthographic = ortho;
    ctrl.set_camera_for_test(camera);
    ctrl
}

fn camera(ctrl: &ViewportController) -> OrbitCamera {
    ctrl.camera_for_test().expect("a camera")
}

fn screen_of(ctrl: &ViewportController, point: Vec3) -> (f32, f32) {
    picking::project_world_to_screen(&camera(ctrl), default_fov_y(), WIDTH, HEIGHT, point)
        .expect("in front of the camera")
}

fn drag(ctrl: &ViewportController, from: ScreenPoint, steps: &[(f32, f32)]) -> Vec<(f32, f32)> {
    ctrl.on_orbit_begin(from.x, from.y);
    steps
        .iter()
        .map(|&(dx, dy)| {
            ctrl.on_rotate(ScreenDelta::new(dx, dy));
            screen_of(ctrl, Vec3::new(40.0, 30.0, 0.5))
        })
        .collect()
}

const STEPS: [(f32, f32); 4] = [(12.0, 4.0), (-30.0, 18.0), (90.0, -40.0), (5.0, 300.0)];

#[test]
fn a_selected_model_far_from_the_plate_centre_holds_still_while_orbiting() {
    for ortho in [false, true] {
        let ctrl = far_cube(ortho);
        let id = ctrl
            .document()
            .with_scene(|s| s.leaf_id_at_index(0))
            .flatten()
            .unwrap();
        ctrl.document().set_selection(Selection::One(LeafId(id)));
        let start = screen_of(&ctrl, Vec3::new(40.0, 30.0, 0.5));
        // The cursor is nowhere near the model: the selection still wins.
        for at in drag(&ctrl, ScreenPoint::new(10.0, 10.0), &STEPS) {
            assert!(
                (at.0 - start.0).abs() < 0.05 && (at.1 - start.1).abs() < 0.05,
                "drifted from {start:?} to {at:?} (ortho {ortho})"
            );
        }
        ctrl.on_orbit_end();
    }
}

#[test]
fn with_nothing_selected_the_point_under_the_cursor_holds_still() {
    for ortho in [false, true] {
        let ctrl = far_cube(ortho);
        let (x, y) = screen_of(&ctrl, Vec3::new(40.0, 30.0, 1.0));
        let pressed = ScreenPoint::new(x, y);
        let ray = picking::ray_from_pixel(&camera(&ctrl), default_fov_y(), WIDTH, HEIGHT, x, y);
        // The top face of the cube under the cursor.
        let t = (1.0 - ray.origin.z) / ray.direction.z;
        let surface = ray.origin + ray.direction * t;
        ctrl.on_orbit_begin(x, y);
        for &(dx, dy) in &STEPS {
            ctrl.on_rotate(ScreenDelta::new(dx, dy));
            let at = screen_of(&ctrl, surface);
            assert!(
                (at.0 - pressed.x).abs() < 0.1 && (at.1 - pressed.y).abs() < 0.1,
                "drifted to {at:?} (ortho {ortho})"
            );
        }
        ctrl.on_orbit_end();
    }
}

#[test]
fn a_press_without_movement_leaves_the_camera_alone() {
    let ctrl = far_cube(false);
    let before = camera(&ctrl);
    ctrl.on_orbit_begin(200.0, 150.0);
    ctrl.on_orbit_end();
    let after = camera(&ctrl);
    assert_eq!(before.target, after.target);
    assert_eq!(before.yaw, after.yaw);
}

#[test]
fn each_drag_picks_its_own_pivot() {
    let ctrl = far_cube(false);
    let pressed = screen_of(&ctrl, Vec3::new(40.0, 30.0, 1.0));
    drag(
        &ctrl,
        ScreenPoint::new(pressed.0, pressed.1),
        &[(20.0, 0.0)],
    );
    ctrl.on_orbit_end();
    // Arrow keys orbit about the target again once the drag is over.
    let target = camera(&ctrl).target;
    ctrl.on_rotate(ScreenDelta::new(20.0, 0.0));
    assert_eq!(camera(&ctrl).target, target);
}

fn select_the_cube(ctrl: &ViewportController) {
    let id = ctrl
        .document()
        .with_scene(|s| s.leaf_id_at_index(0))
        .flatten()
        .unwrap();
    ctrl.document().set_selection(Selection::One(LeafId(id)));
}

#[test]
fn a_numpad_step_turns_about_the_selection_and_it_holds_still() {
    for ortho in [false, true] {
        let ctrl = far_cube(ortho);
        select_the_cube(&ctrl);
        let cube = Vec3::new(40.0, 30.0, 0.5);
        let start = screen_of(&ctrl, cube);
        for (h, v) in [(-1, 0), (1, 0), (0, 1), (0, -1), (1, 0)] {
            let before = camera(&ctrl);
            ctrl.on_orbit_step(h, v);
            let after = camera(&ctrl);
            assert_ne!(after.eye(), before.eye(), "step ({h}, {v}) turns");
            let at = screen_of(&ctrl, cube);
            assert!(
                (at.0 - start.0).abs() < 0.05 && (at.1 - start.1).abs() < 0.05,
                "drift {at:?} from {start:?} (ortho {ortho})"
            );
        }
    }
}

#[test]
fn without_a_selection_a_numpad_step_turns_about_the_target() {
    for ortho in [false, true] {
        let ctrl = far_cube(ortho);
        let target = camera(&ctrl).target;
        ctrl.on_orbit_step(1, 0);
        ctrl.on_orbit_step(0, 1);
        assert!((camera(&ctrl).target - target).length() < 1e-4);
    }
}

#[test]
fn a_numpad_step_during_a_drag_keeps_the_drag_pivot() {
    let ctrl = far_cube(false);
    select_the_cube(&ctrl);
    let cube = Vec3::new(40.0, 30.0, 0.5);
    let start = screen_of(&ctrl, cube);
    drag(&ctrl, ScreenPoint::new(10.0, 10.0), &[(20.0, 5.0)]);
    ctrl.document().set_selection(Selection::None);
    ctrl.on_orbit_step(-1, 0);
    let at = screen_of(&ctrl, cube);
    assert!((at.0 - start.0).abs() < 0.05 && (at.1 - start.1).abs() < 0.05);
    ctrl.on_orbit_end();
}
