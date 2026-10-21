/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Section gating: layout tools are silenced outside the Transform section,
//! undo/redo is silenced only in Slice, and selection stays live everywhere.
//!
//! These drive the controller with a light single-leaf fixture. The
//! pointer-drag gate (which needs a camera, a viewport, and gizmo geometry)
//! lives in the `viewport_click_tests` module next to that scaffolding.

use super::controller::ViewportController;
use super::keyboard_drive::KbdFocus;
use crate::WorkspaceSection;
use crate::test_support::scenes::graph_with_one_leaf;

/// A controller holding 1 selected leaf, parked in `section`.
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
fn undo_runs_in_prepare() {
    let ctrl = controller_in(WorkspaceSection::Transform);
    ctrl.on_delete_requested();
    assert_eq!(ctrl.reachable_leaf_count_for_test(), 0);
    ctrl.set_section(WorkspaceSection::Prepare);
    ctrl.on_undo_requested();
    assert_eq!(
        ctrl.reachable_leaf_count_for_test(),
        1,
        "undo in Prepare restores the deleted leaf; history is reachable outside Slice"
    );
}

#[test]
fn undo_is_silenced_in_slice() {
    let ctrl = controller_in(WorkspaceSection::Transform);
    ctrl.on_delete_requested();
    assert_eq!(ctrl.reachable_leaf_count_for_test(), 0);
    ctrl.set_section(WorkspaceSection::Slice);
    ctrl.on_undo_requested();
    assert_eq!(
        ctrl.reachable_leaf_count_for_test(),
        0,
        "undo must be a no-op in the Slice section"
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
