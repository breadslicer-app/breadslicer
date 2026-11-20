/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Hidden and locked models (#16): a hidden model is not drawn, picked or
//! sliced; a locked one is drawn and sliced but never picked; each change
//! is one Undo step and the project keeps both.

use std::path::PathBuf;
use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, NodeId, Scene};

use crate::camera::{AxisView, OrbitCamera};
use crate::print_selection::PrintDefaults;
use crate::selection::{LeafId, Selection};
use crate::viewport::{ScreenPoint, ViewportController};
use crate::{PointerModifiers, WorkspaceSection};

/// Two unit cubes, one at x 0 and one at x 10, and their leaves.
fn two_cubes() -> (Scene, Vec<NodeId>) {
    let mut scene = Scene::new();
    let mesh = Arc::new(unit_cube_mesh());
    let mut leaves = Vec::new();
    for x in [0.0, 10.0] {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.5])),
                LeafPayload::Mesh(mesh.clone()),
            )
            .unwrap();
        scene.add_root(leaf).unwrap();
        leaves.push(leaf);
    }
    (scene, leaves)
}

fn controller() -> (ViewportController, Vec<NodeId>) {
    let (scene, leaves) = two_cubes();
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(200.0, 100.0);
    ctrl.set_section(WorkspaceSection::Transform);
    (ctrl, leaves)
}

/// Clicks the middle of the view, looking down on `(x, 0)`.
fn click_over(ctrl: &ViewportController, x: f32) {
    let mut camera = OrbitCamera::new(Vec3::new(x, 0.0, 0.5), 30.0);
    camera.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(camera);
    let centre = ScreenPoint::new(100.0, 50.0);
    ctrl.on_pointer_down(centre, PointerModifiers::default());
    ctrl.on_pointer_up(centre, PointerModifiers::default());
}

#[test]
fn a_hidden_model_is_not_picked_not_sliced_and_undo_shows_it() {
    let (ctrl, leaves) = controller();
    let document = ctrl.document();
    document.set_selection(Selection::One(LeafId(leaves[0])));
    let undo = document.undo_len();

    // The eye on the first row hides the first cube: one step, and it
    // leaves the selection.
    ctrl.toggle_row_flag(0, false);
    assert!(document.is_hidden(leaves[0]));
    assert_eq!(document.undo_len(), undo + 1);
    assert!(document.selection_node_ids().is_empty());
    assert!(document.is_dirty(), "the project changed");

    // Not picked: a click on it selects nothing; Select all leaves it out.
    click_over(&ctrl, 0.0);
    assert!(document.selection_node_ids().is_empty());
    document.set_selection(Selection::of([LeafId(leaves[0]), LeafId(leaves[1])]));
    assert_eq!(document.selection_node_ids(), vec![leaves[1]]);

    // Not sliced.
    let (scene, _) = ctrl.scene_of_plate(0).expect("a scene");
    assert!(
        scene.node(leaves[0]).is_none(),
        "the hidden cube is left out"
    );
    assert!(scene.node(leaves[1]).is_some());

    // Undo shows it again.
    assert!(document.undo().unwrap());
    assert!(!document.is_hidden(leaves[0]));
    click_over(&ctrl, 0.0);
    assert_eq!(document.selection_node_ids(), vec![leaves[0]]);
    assert!(document.redo().unwrap());
    assert!(document.is_hidden(leaves[0]));
}

#[test]
fn a_locked_model_is_sliced_but_never_picked() {
    let (ctrl, leaves) = controller();
    let document = ctrl.document();
    ctrl.toggle_row_flag(1, true);
    assert!(document.is_locked(leaves[1]));
    click_over(&ctrl, 10.0);
    assert!(
        document.selection_node_ids().is_empty(),
        "a click passes it"
    );
    document.set_selection(Selection::One(LeafId(leaves[1])));
    assert!(document.selection_node_ids().is_empty(), "nor the list");
    let (scene, _) = ctrl.scene_of_plate(0).expect("a scene");
    assert!(scene.node(leaves[1]).is_some(), "it prints");
    // The padlock again unlocks it.
    ctrl.toggle_row_flag(1, true);
    click_over(&ctrl, 10.0);
    assert_eq!(document.selection_node_ids(), vec![leaves[1]]);
}

#[test]
fn hidden_and_locked_models_come_back_with_the_project() {
    let (ctrl, leaves) = controller();
    let document = ctrl.document();
    assert!(document.set_hidden(&[leaves[1]], true));
    assert!(document.set_locked(&[leaves[0]], true));
    let payload = document.compose_project_payload(&PrintDefaults::default());
    assert_eq!(payload.hidden, vec![1]);
    assert_eq!(payload.locked, vec![0]);

    // Reopened from the project: the same models by their rows.
    let (scene, again) = two_cubes();
    let reopened = crate::scene_document::SceneDocument::new();
    reopened.install_project_graph(scene, PathBuf::from("cubes.bread"), payload);
    assert!(reopened.is_hidden(again[1]));
    assert!(reopened.is_locked(again[0]));
    assert!(!reopened.is_hidden(again[0]));
    assert_eq!(reopened.hidden_leaves(), vec![again[1]]);
}
