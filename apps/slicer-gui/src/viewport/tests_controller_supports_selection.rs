/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! The supports are selected only in the Supports tab (#120). Anywhere else
//! a click on them selects their model, the object list and Tab pass them
//! by, and leaving Supports with them selected selects nothing, so no tool
//! of another tab can move, turn or scale them apart from their model.

use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::indexed_mesh::MeshMetadata;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, Scene};

use super::controller::ViewportController;
use super::types::ScreenPoint;
use crate::camera::{AxisView, OrbitCamera};
use crate::{PointerModifiers, WorkspaceSection};

/// What a leaf of the test plate is.
#[derive(Clone, Copy)]
enum Piece {
    Model,
    Supports,
    /// The one raft for all of a plate (#556).
    Raft,
}

/// A controller holding a unit cube per piece, each its own root at its
/// translation, in that leaf order, nothing selected, in `section`. The
/// camera looks straight down, so the centre of the 200 × 100 viewport
/// clicks the vertical through the camera's target.
fn plate(pieces: &[(Piece, [f64; 3])], section: WorkspaceSection) -> ViewportController {
    let mut scene = Scene::new();
    for &(piece, at) in pieces {
        let mut mesh = unit_cube_mesh();
        let name = match piece {
            Piece::Model => None,
            Piece::Supports => Some(sc_geom::df_supports::DF_SUPPORT_MESH_NAME),
            Piece::Raft => Some(sc_geom::df_supports::PLATE_RAFT_NAME),
        };
        if let Some(name) = name {
            mesh.meta = Some(MeshMetadata {
                name: Some(name.to_owned()),
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
    ctrl.set_viewport_for_test(200.0, 100.0);
    ctrl.set_section(section);
    ctrl
}

/// Clicks the centre of the viewport with the camera over `target`.
fn click_over(ctrl: &ViewportController, target: [f32; 3]) {
    let mut camera = OrbitCamera::new(Vec3::from(target), 30.0);
    camera.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(camera);
    let centre = ScreenPoint::new(100.0, 50.0);
    ctrl.on_pointer_down(centre, PointerModifiers::default());
    ctrl.on_pointer_up(centre, PointerModifiers::default());
}

#[test]
fn a_click_on_the_supports_selects_their_model_in_transform() {
    // The click falls beside the model, on the supports alone: the ray never
    // reaches the model, and the model is still the one selected.
    let ctrl = plate(
        &[(Piece::Model, [0.0; 3]), (Piece::Supports, [3.0, 0.0, 0.0])],
        WorkspaceSection::Transform,
    );
    click_over(&ctrl, [3.0, 0.0, 0.0]);
    assert_eq!(ctrl.selection(), Some(0));
    // The context menu's pick, which selects what it acts on, agrees.
    let model = ctrl.document().selection_primary_node_id();
    assert_eq!(ctrl.pick_node_at(100.0, 50.0), model);
}

#[test]
fn a_click_on_the_supports_selects_the_model_standing_over_them() {
    let ctrl = plate(
        &[
            (Piece::Model, [0.0; 3]),
            (Piece::Supports, [9.0, 0.0, 0.0]),
            (Piece::Model, [10.0, 0.0, 0.0]),
        ],
        WorkspaceSection::Hollow,
    );
    click_over(&ctrl, [9.0, 0.0, 0.0]);
    assert_eq!(
        ctrl.selection(),
        Some(2),
        "the model nearest the supports hit"
    );
}

#[test]
fn a_click_on_the_supports_selects_them_in_supports() {
    let ctrl = plate(
        &[(Piece::Model, [0.0; 3]), (Piece::Supports, [3.0, 0.0, 0.0])],
        WorkspaceSection::Supports,
    );
    click_over(&ctrl, [3.0, 0.0, 0.0]);
    assert_eq!(ctrl.selection(), Some(1));
}

#[test]
fn the_supports_row_selects_nothing_outside_supports() {
    let ctrl = plate(
        &[(Piece::Model, [0.0; 3]), (Piece::Supports, [3.0, 0.0, 0.0])],
        WorkspaceSection::Transform,
    );
    ctrl.on_select_by_index(1);
    assert_eq!(ctrl.selection(), None);
    ctrl.on_select_by_index(0);
    ctrl.on_select_by_index(1);
    assert_eq!(ctrl.selection(), Some(0), "the model stays selected");
    ctrl.set_section(WorkspaceSection::Supports);
    ctrl.on_select_by_index(1);
    assert_eq!(ctrl.selection(), Some(1));
}

#[test]
fn tab_passes_the_supports_by_outside_supports() {
    let ctrl = plate(
        &[
            (Piece::Model, [0.0; 3]),
            (Piece::Supports, [3.0, 0.0, 0.0]),
            (Piece::Model, [10.0, 0.0, 0.0]),
        ],
        WorkspaceSection::Transform,
    );
    ctrl.on_selection_cycle(1);
    assert_eq!(ctrl.selection(), Some(0));
    ctrl.on_selection_cycle(1);
    assert_eq!(ctrl.selection(), Some(2));
    ctrl.on_selection_cycle(1);
    assert_eq!(ctrl.selection(), Some(0), "wraps past the supports");
    ctrl.on_selection_cycle(-1);
    assert_eq!(ctrl.selection(), Some(2));
    ctrl.set_section(WorkspaceSection::Supports);
    ctrl.on_selection_cycle(-1);
    assert_eq!(ctrl.selection(), Some(1));
}

#[test]
fn leaving_supports_with_the_supports_selected_selects_nothing() {
    let ctrl = plate(
        &[(Piece::Model, [0.0; 3]), (Piece::Supports, [3.0, 0.0, 0.0])],
        WorkspaceSection::Supports,
    );
    ctrl.on_select_by_index(1);
    ctrl.set_section(WorkspaceSection::Transform);
    assert_eq!(ctrl.selection(), None);
    // A model selected in Supports stays selected.
    ctrl.set_section(WorkspaceSection::Supports);
    ctrl.on_select_by_index(0);
    ctrl.set_section(WorkspaceSection::Transform);
    assert_eq!(ctrl.selection(), Some(0));
}

#[test]
fn nothing_is_selected_in_slice_and_leaving_it_brings_the_selection_back() {
    // #140: the layers are scrubbed with no selection outline over them.
    let ctrl = plate(
        &[
            (Piece::Model, [0.0; 3]),
            (Piece::Supports, [3.0, 0.0, 0.0]),
            (Piece::Model, [10.0, 0.0, 0.0]),
        ],
        WorkspaceSection::Supports,
    );
    ctrl.on_select_by_index(1);
    ctrl.set_section(WorkspaceSection::Slice);
    assert_eq!(ctrl.selection(), None, "the supports selected before");
    // No click, row, Tab or box selects anything there.
    click_over(&ctrl, [0.0; 3]);
    assert_eq!(ctrl.selection(), None, "a click on a model");
    ctrl.on_select_by_index(0);
    ctrl.on_toggle_by_index(2);
    ctrl.on_selection_cycle(1);
    assert_eq!(ctrl.selection(), None, "the list and Tab");
    let shift = PointerModifiers {
        shift: true,
        ..PointerModifiers::default()
    };
    ctrl.on_pointer_down(ScreenPoint::new(5.0, 5.0), shift.clone());
    ctrl.on_pointer_move(ScreenPoint::new(195.0, 95.0), shift.clone());
    ctrl.on_pointer_up(ScreenPoint::new(195.0, 95.0), shift);
    assert_eq!(ctrl.selection(), None, "a box with Shift");
    // Back in Supports the supports are selected again.
    ctrl.set_section(WorkspaceSection::Supports);
    assert_eq!(ctrl.selection(), Some(1));
    // Back in Transform a model comes back, supports do not (#120).
    ctrl.on_select_by_index(2);
    ctrl.set_section(WorkspaceSection::Slice);
    assert_eq!(ctrl.selection(), None);
    ctrl.set_section(WorkspaceSection::Transform);
    assert_eq!(ctrl.selection(), Some(2));
    // Supports selected in Supports come back as nothing in Transform.
    ctrl.set_section(WorkspaceSection::Supports);
    ctrl.on_select_by_index(1);
    ctrl.set_section(WorkspaceSection::Slice);
    ctrl.set_section(WorkspaceSection::Transform);
    assert_eq!(ctrl.selection(), None, "the supports stay in Supports");
}

#[test]
fn supports_selected_light_the_one_raft_under_them() {
    // The one raft for all is no set's own: it is outlined with the sets it
    // stands under, not with sets elsewhere on the plate.
    let ctrl = plate(
        &[
            (Piece::Model, [0.0; 3]),
            (Piece::Supports, [0.0, 0.0, -1.0]),
            (Piece::Raft, [0.5, 0.0, -2.0]),
            (Piece::Supports, [20.0, 0.0, -1.0]),
        ],
        WorkspaceSection::Supports,
    );
    let raft = ctrl
        .document()
        .with_scene(|s| s.leaf_id_at_index(2))
        .flatten();
    ctrl.on_select_by_index(1);
    assert!(ctrl.document().focus_ids().contains(&raft.unwrap()));
    ctrl.on_select_by_index(3);
    assert!(!ctrl.document().focus_ids().contains(&raft.unwrap()));
    // The raft selected lights every set standing on it, and theirs alone.
    ctrl.on_select_by_index(2);
    let lit = ctrl.document().focus_ids();
    let leaf = |i| {
        ctrl.document()
            .with_scene(|s| s.leaf_id_at_index(i))
            .flatten()
            .unwrap()
    };
    assert!(lit.contains(&leaf(1)), "the set on the raft");
    assert!(!lit.contains(&leaf(3)), "the set off it");
}
