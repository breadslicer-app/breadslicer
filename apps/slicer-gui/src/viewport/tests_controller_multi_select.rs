/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! Selecting several objects (#13): Shift+click adds an object or takes it
//! out, Shift+left drag selects with a box on top of what was selected, the
//! object list agrees with the viewport, and Delete, `G` and the gizmo's
//! arrows act on all of them, each as one undo step.

use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::indexed_mesh::MeshMetadata;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, NodeId, Scene};

use super::controller::{ViewportController, selected_rows};
use super::types::ScreenPoint;
use crate::camera::{AxisView, OrbitCamera};
use crate::mesh_render::default_fov_y;
use crate::selection::{LeafId, Selection};
use crate::{PointerModifiers, WorkspaceSection};

const VIEW: (f32, f32) = (200.0, 100.0);

/// What a leaf of the test plate is.
#[derive(Clone, Copy)]
enum Piece {
    Model,
    Supports,
}

/// A controller holding a unit cube per piece, each its own root at its
/// translation, in that leaf order, nothing selected, in `section`, with
/// the camera looking straight down at the origin.
fn plate(pieces: &[(Piece, [f64; 3])], section: WorkspaceSection) -> ViewportController {
    let mut scene = Scene::new();
    for &(piece, at) in pieces {
        let mut mesh = unit_cube_mesh();
        if let Piece::Supports = piece {
            mesh.meta = Some(MeshMetadata {
                name: Some(sc_geom::df_supports::DF_SUPPORT_MESH_NAME.to_owned()),
                ..Default::default()
            });
        }
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation(at)),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("a fresh root");
    }
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(VIEW.0, VIEW.1);
    ctrl.set_section(section);
    ctrl.set_camera_for_test(top_camera());
    ctrl
}

fn models(n: usize, section: WorkspaceSection) -> ViewportController {
    let pieces: Vec<(Piece, [f64; 3])> = (0..n)
        .map(|i| (Piece::Model, [i as f64 * 6.0 - 6.0, 0.0, 0.0]))
        .collect();
    plate(&pieces, section)
}

fn top_camera() -> OrbitCamera {
    let mut camera = OrbitCamera::new(Vec3::ZERO, 30.0);
    camera.set_axis_view(AxisView::Top);
    camera
}

fn shift() -> PointerModifiers {
    PointerModifiers {
        shift: true,
        ..Default::default()
    }
}

/// The pixel the top camera shows the world point `p` at.
fn pixel(p: [f32; 3]) -> ScreenPoint {
    let (x, y) = crate::picking::project_world_to_screen(
        &top_camera(),
        default_fov_y(),
        VIEW.0,
        VIEW.1,
        Vec3::from(p),
    )
    .expect("in view");
    ScreenPoint::new(x, y)
}

fn click(ctrl: &ViewportController, at: [f32; 3], modifiers: PointerModifiers) {
    let p = pixel(at);
    ctrl.on_pointer_down(p, modifiers.clone());
    ctrl.on_pointer_up(p, modifiers);
}

fn drag(ctrl: &ViewportController, from: [f32; 3], to: [f32; 3], modifiers: PointerModifiers) {
    let (a, b) = (pixel(from), pixel(to));
    ctrl.on_pointer_down(a, modifiers.clone());
    ctrl.on_pointer_move(b, modifiers.clone());
    ctrl.on_pointer_up(b, modifiers);
}

fn leaf(ctrl: &ViewportController, row: usize) -> NodeId {
    ctrl.document()
        .with_scene(|s| s.leaf_id_at_index(row))
        .flatten()
        .expect("a leaf")
}

/// The selected rows, in the order they were selected.
fn selected(ctrl: &ViewportController) -> Vec<usize> {
    let document = ctrl.document();
    document
        .selection_node_ids()
        .into_iter()
        .map(|id| {
            document
                .with_scene(|s| s.leaf_index_of(id))
                .flatten()
                .expect("a live leaf")
        })
        .collect()
}

fn select_rows(ctrl: &ViewportController, rows: &[usize]) {
    let leaves: Vec<LeafId> = rows.iter().map(|&r| LeafId(leaf(ctrl, r))).collect();
    ctrl.document().set_selection(Selection::of(leaves));
}

fn translation(ctrl: &ViewportController, row: usize) -> [f64; 3] {
    let id = leaf(ctrl, row);
    match ctrl
        .document()
        .with_scene(|s| s.leaf(id).map(|q| q.world_transform()))
        .flatten()
        .expect("a leaf")
    {
        Transform3D::Identity => [0.0; 3],
        Transform3D::Affine(a) => *a.translation(),
    }
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

#[test]
fn shift_click_adds_an_object_and_takes_it_out_again() {
    let ctrl = models(3, WorkspaceSection::Transform);
    click(&ctrl, [-6.0, 0.0, 1.0], PointerModifiers::default());
    assert_eq!(selected(&ctrl), vec![0]);
    click(&ctrl, [6.0, 0.0, 1.0], shift());
    assert_eq!(selected(&ctrl), vec![0, 2]);
    // The last one added is the primary: the inspector shows it.
    assert_eq!(ctrl.selection(), Some(2));
    click(&ctrl, [0.0, 0.0, 1.0], shift());
    assert_eq!(selected(&ctrl), vec![0, 2, 1]);
    click(&ctrl, [-6.0, 0.0, 1.0], shift());
    assert_eq!(selected(&ctrl), vec![2, 1]);
}

#[test]
fn a_plain_click_selects_only_that_object() {
    let ctrl = models(3, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0, 1, 2]);
    click(&ctrl, [0.0, 0.0, 1.0], PointerModifiers::default());
    assert_eq!(selected(&ctrl), vec![1]);
    // A plain click on empty space selects nothing.
    select_rows(&ctrl, &[0, 2]);
    click(&ctrl, [0.0, 9.0, 0.0], PointerModifiers::default());
    assert!(selected(&ctrl).is_empty());
}

#[test]
fn shift_click_on_empty_space_keeps_the_selection() {
    let ctrl = models(3, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0, 2]);
    click(&ctrl, [0.0, 9.0, 0.0], shift());
    assert_eq!(selected(&ctrl), vec![0, 2]);
}

#[test]
fn a_shift_drag_adds_what_its_box_touches_to_the_selection() {
    let ctrl = models(3, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0]);
    // From above the middle cube to below the right one, touching both.
    drag(&ctrl, [0.2, 3.0, 0.0], [9.0, -3.0, 0.0], shift());
    assert_eq!(selected(&ctrl), vec![0, 1, 2]);
    // A box over empty plate changes nothing.
    select_rows(&ctrl, &[1]);
    drag(&ctrl, [-4.0, 4.0, 0.0], [-2.0, 2.0, 0.0], shift());
    assert_eq!(selected(&ctrl), vec![1]);
}

#[test]
fn a_box_only_has_to_touch_an_object() {
    let ctrl = models(3, WorkspaceSection::Transform);
    // Over the right edge of the left cube only.
    drag(&ctrl, [-5.8, 1.5, 0.0], [-3.0, -1.5, 0.0], shift());
    assert_eq!(selected(&ctrl), vec![0]);
}

#[test]
fn a_drag_without_shift_draws_no_box() {
    let ctrl = models(3, WorkspaceSection::Transform);
    drag(
        &ctrl,
        [-9.0, 3.0, 0.0],
        [9.0, -3.0, 0.0],
        PointerModifiers::default(),
    );
    assert!(selected(&ctrl).is_empty());
}

#[test]
fn outside_the_supports_tab_a_box_never_selects_supports() {
    // No session drew these supports, so no model comes with them.
    let pieces = [
        (Piece::Model, [-6.0, 0.0, 0.0]),
        (Piece::Supports, [6.0, 0.0, 0.0]),
    ];
    let ctrl = plate(&pieces, WorkspaceSection::Transform);
    drag(&ctrl, [-9.0, 3.0, 0.0], [9.0, -3.0, 0.0], shift());
    assert_eq!(selected(&ctrl), vec![0]);
    // Shift+click on them does not add them either.
    click(&ctrl, [6.0, 0.0, 1.0], shift());
    assert!(!selected(&ctrl).contains(&1));

    let ctrl = plate(&pieces, WorkspaceSection::Supports);
    drag(&ctrl, [-9.0, 3.0, 0.0], [9.0, -3.0, 0.0], shift());
    assert_eq!(selected(&ctrl), vec![0, 1]);
}

#[test]
fn leaving_the_supports_tab_takes_only_the_supports_out() {
    let pieces = [
        (Piece::Model, [-6.0, 0.0, 0.0]),
        (Piece::Supports, [0.0, 0.0, 0.0]),
        (Piece::Model, [6.0, 0.0, 0.0]),
    ];
    let ctrl = plate(&pieces, WorkspaceSection::Supports);
    select_rows(&ctrl, &[0, 1, 2]);
    ctrl.set_section(WorkspaceSection::Transform);
    assert_eq!(selected(&ctrl), vec![0, 2]);
}

#[test]
fn the_object_list_and_the_viewport_agree() {
    let ctrl = models(3, WorkspaceSection::Transform);
    click(&ctrl, [-6.0, 0.0, 1.0], PointerModifiers::default());
    click(&ctrl, [6.0, 0.0, 1.0], shift());
    assert_eq!(selected_rows(&ctrl.document()), vec![true, false, true]);
    // Ctrl/Shift+click on a row does what Shift+click does in the viewport.
    ctrl.on_toggle_by_index(1);
    assert_eq!(selected_rows(&ctrl.document()), vec![true, true, true]);
    ctrl.on_toggle_by_index(0);
    assert_eq!(selected(&ctrl), vec![2, 1]);
    assert_eq!(selected_rows(&ctrl.document()), vec![false, true, true]);
    // A plain click on a row selects that row alone, as in the viewport.
    ctrl.on_select_by_index(0);
    assert_eq!(selected_rows(&ctrl.document()), vec![true, false, false]);
}

#[test]
fn the_supports_row_does_not_join_outside_the_supports_tab() {
    let pieces = [
        (Piece::Model, [-6.0, 0.0, 0.0]),
        (Piece::Supports, [6.0, 0.0, 0.0]),
    ];
    let ctrl = plate(&pieces, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0]);
    ctrl.on_toggle_by_index(1);
    assert_eq!(selected(&ctrl), vec![0]);
}

#[test]
fn delete_removes_every_selected_object_in_one_undo_step() {
    let ctrl = models(3, WorkspaceSection::Transform);
    let kept = leaf(&ctrl, 1);
    select_rows(&ctrl, &[0, 2]);
    ctrl.on_delete_requested();
    let document = ctrl.document();
    assert_eq!(
        document.with_scene(|s| s.graph().reachable_leaf_count()),
        Some(1)
    );
    assert_eq!(leaf(&ctrl, 0), kept);
    assert!(selected(&ctrl).is_empty());
    assert!(document.undo().unwrap());
    assert!(!document.can_undo(), "one step");
    assert_eq!(
        document.with_scene(|s| s.graph().reachable_leaf_count()),
        Some(3)
    );
    assert_eq!(leaf(&ctrl, 1), kept, "each back in its place");
}

#[test]
fn deleting_some_selected_objects_keeps_the_others_selected() {
    let ctrl = models(3, WorkspaceSection::Transform);
    let (first, last) = (leaf(&ctrl, 0), leaf(&ctrl, 2));
    select_rows(&ctrl, &[0, 2]);
    let document = ctrl.document();
    // Undo of a paste, a split or another tool removes some of them.
    let remove =
        document.with_scene(|s| sc_scene_edit::SceneEdit::remove_leaf_with_gc(s.graph(), first));
    document
        .apply_edit(remove.flatten().expect("a leaf"))
        .expect("applied");
    assert_eq!(document.selection(), Selection::One(LeafId(last)));
}

#[test]
fn g_moves_every_selected_object_by_the_same_offset_in_one_undo_step() {
    let ctrl = models(3, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0, 2]);
    let before: Vec<[f64; 3]> = (0..3).map(|r| translation(&ctrl, r)).collect();
    let start = pixel([0.0, 0.0, 0.0]);
    ctrl.on_modal_transform_start(0, start.x, start.y);
    ctrl.on_pointer_move(
        ScreenPoint::new(start.x + 20.0, start.y),
        PointerModifiers::default(),
    );
    let moved: Vec<[f64; 3]> = (0..3).map(|r| translation(&ctrl, r)).collect();
    let d0 = [moved[0][0] - before[0][0], moved[0][1] - before[0][1]];
    let d2 = [moved[2][0] - before[2][0], moved[2][1] - before[2][1]];
    assert!(d0[0] > 1.0, "moved right: {d0:?}");
    assert!((d0[0] - d2[0]).abs() < 1e-6 && (d0[1] - d2[1]).abs() < 1e-6);
    assert!(close(moved[1], before[1]), "the unselected one stays");
    ctrl.on_modal_transform_finish(true);
    let document = ctrl.document();
    assert!(document.undo().unwrap());
    assert!(!document.can_undo(), "one step");
    for (r, &b) in before.iter().enumerate() {
        assert!(close(translation(&ctrl, r), b));
    }
}

#[test]
fn cancelling_g_puts_every_selected_object_back_and_records_nothing() {
    let ctrl = models(3, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0, 1]);
    let before: Vec<[f64; 3]> = (0..3).map(|r| translation(&ctrl, r)).collect();
    let start = pixel([0.0, 0.0, 0.0]);
    ctrl.on_modal_transform_start(0, start.x, start.y);
    ctrl.on_pointer_move(
        ScreenPoint::new(start.x, start.y + 15.0),
        PointerModifiers::default(),
    );
    ctrl.on_modal_transform_finish(false);
    for (r, &b) in before.iter().enumerate() {
        assert_eq!(translation(&ctrl, r), b);
    }
    assert!(!ctrl.document().can_undo());
}

#[test]
fn r_and_s_wait_for_one_object_with_several_selected() {
    let ctrl = models(2, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0, 1]);
    let start = pixel([0.0, 0.0, 0.0]);
    for code in [1, 2] {
        ctrl.on_modal_transform_start(code, start.x, start.y);
        ctrl.on_pointer_move(
            ScreenPoint::new(start.x + 30.0, start.y + 10.0),
            PointerModifiers::default(),
        );
        ctrl.on_modal_transform_finish(true);
    }
    assert!(!ctrl.document().can_undo(), "nothing turned or scaled");
}

#[test]
fn the_gizmo_arrow_moves_every_selected_object_in_one_undo_step() {
    let ctrl = models(3, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0, 2]);
    let before: Vec<[f64; 3]> = (0..3).map(|r| translation(&ctrl, r)).collect();
    // The gizmo stands at the centre of the two cubes' box, the middle
    // one's centre. Its X arrow runs right from there, the cone grabbing.
    let camera = top_camera();
    let pivot = Vec3::ZERO;
    let scale =
        crate::gizmo::gizmo_scale(&camera, pivot, default_fov_y(), VIEW.1).expect("in view");
    let on_arrow = pixel((pivot + Vec3::X * scale * 1.1).to_array());
    ctrl.on_pointer_down(on_arrow, PointerModifiers::default());
    ctrl.on_pointer_move(
        ScreenPoint::new(on_arrow.x + 10.0, on_arrow.y),
        PointerModifiers::default(),
    );
    ctrl.on_pointer_up(
        ScreenPoint::new(on_arrow.x + 10.0, on_arrow.y),
        PointerModifiers::default(),
    );
    let moved: Vec<[f64; 3]> = (0..3).map(|r| translation(&ctrl, r)).collect();
    let dx = moved[0][0] - before[0][0];
    assert!(dx > 0.1, "moved along X: {dx}");
    assert!((moved[2][0] - before[2][0] - dx).abs() < 1e-6);
    assert!(close(moved[1], before[1]), "the unselected one stays");
    assert_eq!(selected(&ctrl), vec![0, 2], "the selection stays");
    let document = ctrl.document();
    assert!(document.undo().unwrap());
    assert!(!document.can_undo(), "one step");
    for (r, &b) in before.iter().enumerate() {
        assert!(close(translation(&ctrl, r), b));
    }
}

#[test]
fn arrange_selection_moves_every_selected_model_and_nothing_else() {
    let ctrl = models(3, WorkspaceSection::Transform);
    select_rows(&ctrl, &[0, 2]);
    let before: Vec<[f64; 3]> = (0..3).map(|r| translation(&ctrl, r)).collect();
    let plate = sc_arrange::Plate {
        min: [0.0, 0.0],
        max: [60.0, 40.0],
        floor: 0.0,
    };
    crate::arrange_run::arrange_now(
        &ctrl.document(),
        &plate,
        crate::arrange_run::ArrangeOptions::default(),
        true,
    )
    .expect("arranged");
    assert!(!close(translation(&ctrl, 0), before[0]));
    assert!(!close(translation(&ctrl, 2), before[2]));
    assert!(close(translation(&ctrl, 1), before[1]));
    let document = ctrl.document();
    assert!(document.undo().unwrap());
    assert!(!document.can_undo(), "one step");
}
