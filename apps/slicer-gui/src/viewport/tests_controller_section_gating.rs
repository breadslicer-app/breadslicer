/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Section gating: scene-mutating operations are silenced outside the
//! Transform section, while selection stays live everywhere.
//!
//! These drive the controller with a light single-leaf fixture. The
//! pointer-drag gate (which needs a camera, a viewport, and gizmo
//! geometry) lives in the `viewport_click_tests` module next to that
//! scaffolding.

use std::sync::Arc;

use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, LeafPayload, Scene};

use super::controller::ViewportController;
use super::keyboard_drive::KbdFocus;
use crate::WorkspaceSection;

fn empty_leaf_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
        faces: vec![],
    }
}

/// Loader-faithful single-leaf graph under one identity-Group root.
fn graph_with_one_leaf() -> Scene {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    let leaf = g
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(empty_leaf_mesh())),
        )
        .expect("add leaf");
    g.add_child(root, leaf).expect("attach leaf");
    g
}

/// A controller holding one selected leaf, parked in `section`.
fn controller_in(section: WorkspaceSection) -> ViewportController {
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(graph_with_one_leaf());
    ctrl.on_select_by_index(0);
    ctrl.set_section(section);
    ctrl
}

#[test]
fn delete_is_silenced_in_prepare() {
    let ctrl = controller_in(WorkspaceSection::Prepare);
    ctrl.on_delete_requested();
    assert_eq!(
        ctrl.reachable_leaf_count_for_test(),
        1,
        "delete must be a no-op outside the Transform section"
    );
}

#[test]
fn delete_is_silenced_in_slice() {
    let ctrl = controller_in(WorkspaceSection::Slice);
    ctrl.on_delete_requested();
    assert_eq!(ctrl.reachable_leaf_count_for_test(), 1);
}

#[test]
fn delete_runs_in_transform() {
    let ctrl = controller_in(WorkspaceSection::Transform);
    ctrl.on_delete_requested();
    assert_eq!(ctrl.reachable_leaf_count_for_test(), 0);
}

#[test]
fn keyboard_focus_cannot_arm_outside_transform() {
    let ctrl = controller_in(WorkspaceSection::Slice);
    ctrl.on_gizmo_focus_translate();
    assert!(
        matches!(ctrl.keyboard_focus_for_test(), KbdFocus::None),
        "keyboard gizmo focus must not arm outside the Transform section"
    );
}

#[test]
fn keyboard_focus_arms_in_transform() {
    let ctrl = controller_in(WorkspaceSection::Transform);
    ctrl.on_gizmo_focus_translate();
    assert!(matches!(
        ctrl.keyboard_focus_for_test(),
        KbdFocus::Translate
    ));
}

#[test]
fn leaving_transform_drops_keyboard_focus() {
    let ctrl = controller_in(WorkspaceSection::Transform);
    ctrl.on_gizmo_focus_rotate();
    assert!(matches!(ctrl.keyboard_focus_for_test(), KbdFocus::Rotate));
    ctrl.set_section(WorkspaceSection::Prepare);
    assert!(
        matches!(ctrl.keyboard_focus_for_test(), KbdFocus::None),
        "switching away from Transform must drop any armed keyboard focus"
    );
}

#[test]
fn undo_is_silenced_outside_transform() {
    let ctrl = controller_in(WorkspaceSection::Transform);
    ctrl.on_delete_requested();
    assert_eq!(ctrl.reachable_leaf_count_for_test(), 0);
    ctrl.set_section(WorkspaceSection::Prepare);
    ctrl.on_undo_requested();
    assert_eq!(
        ctrl.reachable_leaf_count_for_test(),
        0,
        "undo must be a no-op outside the Transform section"
    );
}

#[test]
fn undo_restores_in_transform() {
    let ctrl = controller_in(WorkspaceSection::Transform);
    ctrl.on_delete_requested();
    assert_eq!(ctrl.reachable_leaf_count_for_test(), 0);
    ctrl.on_undo_requested();
    assert_eq!(
        ctrl.reachable_leaf_count_for_test(),
        1,
        "undo in Transform restores the deleted leaf"
    );
}

#[test]
fn selection_stays_live_outside_transform() {
    // Selection is not a transform; it must work in every section.
    let ctrl = controller_in(WorkspaceSection::Prepare);
    assert_eq!(ctrl.selection(), Some(0));
    ctrl.on_clear_selection();
    assert_eq!(
        ctrl.selection(),
        None,
        "clearing the selection must work outside the Transform section"
    );
}
