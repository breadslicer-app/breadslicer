/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! Lay on face (#15): armed, a click on a face of a model turns it so that
//! face lies on the plate, in one Undo step, and the tool disarms.

use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, Scene};

use super::controller::ViewportController;
use super::types::ScreenPoint;
use crate::camera::{AxisView, OrbitCamera};
use crate::{PointerModifiers, WorkspaceSection};

/// A 1 × 1 × 4 mm post standing 3 mm above the plate, its centre at
/// (0, 0, 5).
fn post() -> ViewportController {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 4.0]],
                [0.0, 0.0, 5.0],
            )),
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

/// The model's world box.
fn world_box(ctrl: &ViewportController) -> ([f32; 3], [f32; 3]) {
    let document = ctrl.document();
    document
        .with_scene(|spatial| {
            let id = spatial.leaf_id_at_index(0)?;
            let query = spatial.leaf(id)?;
            let aabb = query.transformed_aabb(query.world_transform())?;
            Some((aabb.min().coords(), aabb.max().coords()))
        })
        .flatten()
        .expect("the post")
}

/// Clicks the middle of the view, looking at `target` from `view`.
fn click(ctrl: &ViewportController, target: [f32; 3], view: AxisView) {
    let mut camera = OrbitCamera::new(Vec3::from(target), 40.0);
    camera.set_axis_view(view);
    ctrl.set_camera_for_test(camera);
    let centre = ScreenPoint::new(100.0, 50.0);
    ctrl.on_pointer_down(centre, PointerModifiers::default());
    ctrl.on_pointer_up(centre, PointerModifiers::default());
}

#[test]
fn a_click_on_a_side_lays_the_model_on_it_in_one_undo_step() {
    let ctrl = post();
    let (lo, hi) = world_box(&ctrl);
    assert!((lo[2] - 3.0).abs() < 1e-4 && (hi[2] - 7.0).abs() < 1e-4);
    let undo_before = ctrl.document().undo_len();

    ctrl.set_lay_on_face(true);
    assert!(ctrl.laying_on_face());
    // From the front, the side facing -Y.
    click(&ctrl, [0.0, 0.0, 5.0], AxisView::Front);
    assert!(!ctrl.laying_on_face(), "one click, then the tool disarms");

    let (lo, hi) = world_box(&ctrl);
    let size = [0, 1, 2].map(|k| hi[k] - lo[k]);
    assert!((size[2] - 1.0).abs() < 1e-3, "lying down: {size:?}");
    assert!((size[1] - 4.0).abs() < 1e-3, "long along Y: {size:?}");
    assert!(lo[2].abs() < 1e-4, "on the plate: {lo:?}");
    // Turned about the middle of its footprint.
    assert!(((lo[0] + hi[0]) * 0.5).abs() < 1e-4);
    assert!(((lo[1] + hi[1]) * 0.5).abs() < 1e-4);
    assert_eq!(ctrl.document().undo_len(), undo_before + 1, "one step");
    assert_eq!(
        ctrl.document().selection_node_ids().len(),
        1,
        "the model laid is selected"
    );

    ctrl.on_undo_requested();
    let (lo, hi) = world_box(&ctrl);
    assert!((lo[2] - 3.0).abs() < 1e-4 && (hi[2] - 7.0).abs() < 1e-4);
}

#[test]
fn unarmed_or_outside_transform_a_click_lays_nothing() {
    let ctrl = post();
    let before = world_box(&ctrl);
    click(&ctrl, [0.0, 0.0, 5.0], AxisView::Front);
    assert_eq!(world_box(&ctrl), before, "a plain click selects");

    // Leaving the Transform tab disarms it.
    ctrl.set_lay_on_face(true);
    ctrl.set_section(WorkspaceSection::Slice);
    assert!(!ctrl.laying_on_face());
    ctrl.set_lay_on_face(true);
    assert!(!ctrl.laying_on_face(), "not armed where models do not move");
}

#[test]
fn the_face_that_already_lies_down_only_drops_to_the_plate() {
    let ctrl = post();
    let undo_before = ctrl.document().undo_len();
    ctrl.set_lay_on_face(true);
    // From under the plate, the bottom face: no turn, down to the plate.
    click(&ctrl, [0.0, 0.0, 5.0], AxisView::Bottom);
    let (lo, hi) = world_box(&ctrl);
    assert!(
        lo[2].abs() < 1e-4 && (hi[2] - 4.0).abs() < 1e-4,
        "{lo:?} {hi:?}"
    );
    assert_eq!(ctrl.document().undo_len(), undo_before + 1);
    // On the plate already: nothing to do, no step.
    ctrl.set_lay_on_face(true);
    click(&ctrl, [0.0, 0.0, 2.0], AxisView::Bottom);
    assert_eq!(ctrl.document().undo_len(), undo_before + 1);
}
