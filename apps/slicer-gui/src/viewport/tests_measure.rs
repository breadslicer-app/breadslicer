/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! The Measure tool (#652): clicks take points on the model's surface,
//! snapped to a face's corner near the cursor, and select nothing.

use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, Scene};

use super::controller::ViewportController;
use super::types::ScreenPoint;
use crate::camera::{AxisView, OrbitCamera};
use crate::{PointerModifiers, WorkspaceSection};

/// A unit cube at `at`, seen from straight above round `target`.
fn scene(at: [f64; 3]) -> ViewportController {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(at)),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("a fresh root");
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(200.0, 100.0);
    ctrl.set_section(WorkspaceSection::Transform);
    ctrl
}

/// Clicks the centre of the viewport with the camera over `target`.
fn click_over(ctrl: &ViewportController, target: [f32; 3], shift: bool) {
    let mut camera = OrbitCamera::new(Vec3::from(target), 30.0);
    camera.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(camera);
    let centre = ScreenPoint::new(100.0, 50.0);
    let modifiers = PointerModifiers {
        shift,
        ..Default::default()
    };
    ctrl.on_pointer_down(centre, modifiers.clone());
    ctrl.on_pointer_up(centre, modifiers);
}

#[test]
fn clicks_while_measuring_take_points_and_select_nothing() {
    let ctrl = scene([0.0, 0.0, 0.5]);
    ctrl.set_measuring(true);
    // The top face of the cube is at z 1, the middle of the face picked.
    click_over(&ctrl, [0.1, 0.1, 0.0], false);
    let points = ctrl.measure_points();
    assert_eq!(points.len(), 1);
    assert!((points[0][2] - 1.0).abs() < 1e-4, "{points:?}");
    assert!(
        ctrl.document().selection_node_ids().is_empty(),
        "nothing is selected"
    );
    // Right over a corner: the point lands on it.
    click_over(&ctrl, [0.49, 0.49, 0.0], true);
    let points = ctrl.measure_points();
    assert_eq!(points.len(), 2);
    for (got, want) in points[1].iter().zip([0.5, 0.5, 1.0]) {
        assert!((got - want).abs() < 1e-5, "{points:?}");
    }
    // Esc forgets the points, a second one turns the tool off.
    ctrl.measure_escape();
    assert!(ctrl.measure_points().is_empty());
    assert!(ctrl.measuring());
    ctrl.measure_escape();
    assert!(!ctrl.measuring());
    // Off, a click selects again.
    click_over(&ctrl, [0.1, 0.1, 0.0], false);
    assert_eq!(ctrl.document().selection_node_ids().len(), 1);
}
