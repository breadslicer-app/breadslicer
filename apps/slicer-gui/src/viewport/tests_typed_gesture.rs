/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! A number typed during G, R or S (#14): `G X 10` Enter moves exactly
//! 10 mm, `R Z 45` turns exactly 45°, `S 2` doubles the size, each in one
//! Undo step, and the cursor no longer drives the gesture.

use glam::Vec3;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, Scene};
use std::sync::Arc;

use super::controller::ViewportController;
use super::types::ScreenPoint;
use crate::PointerModifiers;
use crate::camera::OrbitCamera;
use crate::selection::{LeafId, Selection};

/// A selected unit cube at (10, 5, 0.5), seen at a slant.
fn cube() -> ViewportController {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 5.0, 0.5])),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("a fresh root");
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(800.0, 600.0);
    let mut camera = OrbitCamera::new(Vec3::new(10.0, 5.0, 0.5), 12.0);
    camera.yaw = -0.7;
    camera.pitch = 0.6;
    ctrl.set_camera_for_test(camera);
    ctrl.document().set_selection(Selection::One(LeafId(leaf)));
    ctrl
}

fn affine(ctrl: &ViewportController) -> AffineTransform3D {
    ctrl.document()
        .with_scene(
            |s| match s.leaf(s.leaf_id_at_index(0)?)?.local_transform() {
                Transform3D::Identity => Some(AffineTransform3D::identity()),
                Transform3D::Affine(a) => Some(a),
            },
        )
        .flatten()
        .expect("the cube")
}

fn type_keys(ctrl: &ViewportController, keys: &[&str]) {
    for key in keys {
        ctrl.on_modal_transform_type(key);
    }
}

#[test]
fn g_x_10_enter_moves_exactly_10_mm_in_one_undo_step() {
    let ctrl = cube();
    let undo = ctrl.document().undo_len();
    ctrl.on_modal_transform_start(0, 300.0, 300.0);
    ctrl.on_pointer_move(ScreenPoint::new(340.0, 280.0), PointerModifiers::default());
    ctrl.on_modal_transform_axis(0, 340.0, 280.0);
    type_keys(&ctrl, &["1", "0"]);
    // The cursor no longer drives it.
    ctrl.on_pointer_move(ScreenPoint::new(500.0, 100.0), PointerModifiers::default());
    ctrl.on_modal_transform_finish(true);
    assert_eq!(*affine(&ctrl).translation(), [20.0, 5.0, 0.5]);
    assert_eq!(ctrl.document().undo_len(), undo + 1);
    ctrl.on_undo_requested();
    assert_eq!(*affine(&ctrl).translation(), [10.0, 5.0, 0.5]);
}

#[test]
fn a_minus_a_comma_and_backspace_shape_the_number() {
    let ctrl = cube();
    ctrl.on_modal_transform_start(0, 300.0, 300.0);
    ctrl.on_modal_transform_axis(1, 300.0, 300.0);
    // 2,55 then Backspace, then a minus: -2.5 along Y.
    type_keys(&ctrl, &["2", ",", "5", "5", "\u{8}", "-"]);
    // A key that is not the number's is refused.
    assert!(!ctrl.on_modal_transform_type("q"));
    ctrl.on_modal_transform_finish(true);
    let t = affine(&ctrl);
    assert!((t.translation()[1] - 2.5).abs() < 1e-9, "{t:?}");
}

#[test]
fn r_z_45_turns_exactly_45_degrees_about_the_cube() {
    let ctrl = cube();
    ctrl.on_modal_transform_start(1, 350.0, 300.0);
    ctrl.on_modal_transform_axis(2, 350.0, 300.0);
    type_keys(&ctrl, &["4", "5"]);
    ctrl.on_modal_transform_finish(true);
    let t = affine(&ctrl);
    let expected = AffineTransform3D::from_rotation_z(45f64.to_radians());
    for r in 0..3 {
        for c in 0..3 {
            assert!((t.linear()[r][c] - expected.linear()[r][c]).abs() < 1e-9);
        }
    }
    // Turned about its own middle: it stays where it was.
    assert!((t.translation()[0] - 10.0).abs() < 1e-6);
    assert!((t.translation()[1] - 5.0).abs() < 1e-6);
}

#[test]
fn s_2_doubles_the_size_and_s_x_2_only_its_width() {
    let ctrl = cube();
    ctrl.on_modal_transform_start(2, 350.0, 300.0);
    type_keys(&ctrl, &["2"]);
    ctrl.on_modal_transform_finish(true);
    let m = *affine(&ctrl).linear();
    assert!((m[0][0] - 2.0).abs() < 1e-9 && (m[2][2] - 2.0).abs() < 1e-9);

    ctrl.on_modal_transform_start(2, 350.0, 300.0);
    ctrl.on_modal_transform_axis(0, 350.0, 300.0);
    type_keys(&ctrl, &["1", ".", "5"]);
    ctrl.on_modal_transform_finish(true);
    let m = *affine(&ctrl).linear();
    assert!((m[0][0] - 3.0).abs() < 1e-9, "{m:?}");
    assert!((m[1][1] - 2.0).abs() < 1e-9 && (m[2][2] - 2.0).abs() < 1e-9);
}
