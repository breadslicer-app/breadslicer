/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! The Transform inspector's typed edits (BS-14, BS-15) on a real controller: each
//! is one undoable edit, only in Transform, and lands the object where asked.

use super::controller::ViewportController;
use crate::WorkspaceSection;
use crate::test_support::scenes::cube_scene;

/// A controller holding the unit cube (z from -0.5 to 0.5), selected, in
/// `section`.
fn cube_in(section: WorkspaceSection) -> ViewportController {
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(cube_scene());
    ctrl.on_select_by_index(0);
    ctrl.set_section(section);
    ctrl
}

/// The selected leaf's lowest point and its tight world size.
fn bottom_and_size(ctrl: &ViewportController) -> (f64, [f64; 3]) {
    let document = ctrl.document();
    let id = document.selection_primary_node_id().expect("a selection");
    document
        .with_scene(|spatial| {
            let query = spatial.leaf(id)?;
            let aabb = query.transformed_aabb(query.world_transform())?;
            let (min, max) = (aabb.min().coords(), aabb.max().coords());
            Some((
                f64::from(min[2]),
                [0, 1, 2].map(|k| f64::from(max[k] - min[k])),
            ))
        })
        .flatten()
        .expect("cube geometry")
}

#[test]
fn a_typed_lift_raises_the_bottom_and_undo_puts_it_back() {
    let ctrl = cube_in(WorkspaceSection::Transform);
    let (start, _) = bottom_and_size(&ctrl);
    ctrl.on_numeric_edit(2, 5.0);
    let (lifted, _) = bottom_and_size(&ctrl);
    assert!((lifted - 5.0).abs() < 1e-4, "bottom at {lifted}");
    ctrl.on_undo_requested();
    let (back, _) = bottom_and_size(&ctrl);
    assert!(
        (back - start).abs() < 1e-6,
        "undo left the bottom at {back}"
    );
}

#[test]
fn typed_edits_do_nothing_outside_transform() {
    let ctrl = cube_in(WorkspaceSection::Hollow);
    let (start, _) = bottom_and_size(&ctrl);
    ctrl.on_numeric_edit(2, 5.0);
    assert_eq!(bottom_and_size(&ctrl).0, start);
}

#[test]
fn supports_takes_the_lift_but_not_a_rotation() {
    let ctrl = cube_in(WorkspaceSection::Supports);
    ctrl.on_numeric_edit(3, 45.0);
    let (_, size) = bottom_and_size(&ctrl);
    assert!((size[1] - 1.0).abs() < 1e-6, "no rotation in Supports");
    ctrl.on_numeric_edit(2, 5.0);
    let (bottom, _) = bottom_and_size(&ctrl);
    assert!(
        (bottom - 5.0).abs() < 1e-4,
        "the lift is a Supports setting"
    );
}

#[test]
fn a_typed_height_scales_uniformly_and_keeps_the_bottom() {
    let ctrl = cube_in(WorkspaceSection::Transform);
    ctrl.on_numeric_edit(2, 3.0);
    ctrl.on_numeric_edit(9, 2.0);
    let (bottom, size) = bottom_and_size(&ctrl);
    assert!((bottom - 3.0).abs() < 1e-4, "bottom at {bottom}");
    for (k, s) in size.iter().enumerate() {
        assert!((s - 2.0).abs() < 1e-4, "axis {k} is {s}");
    }
}

#[test]
fn a_typed_rotation_keeps_the_lift() {
    let ctrl = cube_in(WorkspaceSection::Transform);
    ctrl.on_numeric_edit(2, 4.0);
    ctrl.on_numeric_edit(3, 45.0);
    let (bottom, size) = bottom_and_size(&ctrl);
    assert!((bottom - 4.0).abs() < 1e-4, "bottom at {bottom}");
    // A unit cube turned 45 degrees about X spans sqrt 2 in Y and Z.
    assert!((size[1] - 2f64.sqrt()).abs() < 1e-4, "Y span {}", size[1]);
}
