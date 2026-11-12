/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The plates of a scene (#545): adding and removing them in one undo step,
//! the models of the plates after a removed one moving up with them, a click
//! on a plate making it the active one, and a printer change taking the
//! models along with their plates.

use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::AffineTransform3D;
use sc_scene::{LeafPayload, NodeId, Scene};

use super::*;
use crate::camera::OrbitCamera;
use crate::plates::offset;

fn volume() -> BuildVolume {
    BuildVolume::new(100.0, 50.0, 80.0)
}

/// A small closed box of 4 mm a side standing at `x, y`.
fn box_mesh() -> IndexedMesh {
    let h = 2.0;
    let c = [
        [-h, -h, 0.0],
        [h, -h, 0.0],
        [h, h, 0.0],
        [-h, h, 0.0],
        [-h, -h, 4.0],
        [h, -h, 4.0],
        [h, h, 4.0],
        [-h, h, 4.0],
    ];
    let faces = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    IndexedMesh {
        meta: None,
        vertices: c.iter().map(|&p| Vertex::new(p)).collect(),
        faces: faces
            .iter()
            .map(|&f| IndexedTriangle::from_raw_indices(f))
            .collect(),
    }
}

/// A window on boxes at `places`, under a group root as a loader makes,
/// on [`volume`]'s plates; their leaves in order.
fn window(places: &[Vec3]) -> (ViewportController, Vec<NodeId>) {
    window_of(places, 1.0)
}

/// [`window`] on boxes `scale` times as big.
fn window_of(places: &[Vec3], scale: f64) -> (ViewportController, Vec<NodeId>) {
    let ctrl = ViewportController::new_for_test();
    ctrl.set_build_volume(volume());
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    let mesh = Arc::new(box_mesh());
    let mut leaves = Vec::new();
    for p in places {
        let at = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
            [[scale, 0.0, 0.0], [0.0, scale, 0.0], [0.0, 0.0, scale]],
            [f64::from(p.x), f64::from(p.y), 0.0],
        ));
        let leaf = g
            .add_leaf(None, at, LeafPayload::Mesh(mesh.clone()))
            .expect("leaf");
        g.add_child(root, leaf).expect("attach");
        leaves.push(leaf);
    }
    ctrl.install_graph_for_test(g);
    ctrl.set_viewport_for_test(200.0, 200.0);
    ctrl.frame_camera_for_test();
    (ctrl, leaves)
}

fn centre_of(ctrl: &ViewportController, leaf: NodeId) -> Vec3 {
    ctrl.document()
        .with_scene(|spatial| {
            let q = spatial.leaf(leaf)?;
            let b = q.transformed_aabb(q.world_transform())?;
            let (lo, hi) = (b.min().coords(), b.max().coords());
            Some(Vec3::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, 0.0))
        })
        .flatten()
        .expect("the box")
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-3
}

#[test]
fn a_plate_added_goes_after_the_last_and_undoes_in_one_step() {
    let (ctrl, _) = window(&[Vec3::ZERO]);
    let document = ctrl.document();
    assert_eq!(document.plates().count(), 1);
    ctrl.add_plate();
    ctrl.add_plate();
    assert_eq!(document.plates().count(), 3);
    assert_eq!(document.plates().active(), 0, "the active plate stays");
    assert!(document.is_dirty(), "the project changed");
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plates().count(), 2);
    assert!(document.redo().expect("redoes"));
    assert_eq!(document.plates().count(), 3);
}

/// A plate with a model on it goes too (#545): the next plate takes its
/// place and its models with it; the last plate's go to the one before.
#[test]
fn a_plate_removed_hands_its_models_to_the_plate_in_its_place() {
    let v = volume();
    let (ctrl, leaves) = window(&[
        Vec3::new(10.0, 5.0, 0.0),
        offset(&v, 2) + Vec3::new(-10.0, 0.0, 0.0),
    ]);
    let document = ctrl.document();
    ctrl.add_plate();
    ctrl.add_plate();
    // The first plate, with a model, and the second empty: the second
    // takes its place and the model.
    document.set_active_plate(0);
    ctrl.remove_active_plate();
    assert_eq!(document.plates().count(), 2);
    assert_eq!(document.plates().active(), 0, "the plate that took them");
    assert!(
        near(centre_of(&ctrl, leaves[0]), Vec3::new(10.0, 5.0, 0.0)),
        "on it, where it was"
    );
    assert!(near(
        centre_of(&ctrl, leaves[1]),
        offset(&v, 1) + Vec3::new(-10.0, 0.0, 0.0)
    ));
    // The last plate's model goes to the one before, where it stood.
    document.set_active_plate(1);
    ctrl.remove_active_plate();
    assert_eq!(document.plates().count(), 1);
    assert!(near(
        centre_of(&ctrl, leaves[1]),
        Vec3::new(-10.0, 0.0, 0.0)
    ));
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plates().count(), 2);
    assert!(near(
        centre_of(&ctrl, leaves[1]),
        offset(&v, 1) + Vec3::new(-10.0, 0.0, 0.0)
    ));
}

#[test]
fn removing_a_plate_moves_the_plates_after_it_up_with_their_models() {
    let v = volume();
    let (ctrl, leaves) = window(&[Vec3::ZERO, offset(&v, 2), offset(&v, 5)]);
    let document = ctrl.document();
    for _ in 0..5 {
        ctrl.add_plate();
    }
    assert_eq!(document.plates().count(), 6);
    document.set_active_plate(1);
    ctrl.remove_active_plate();
    assert_eq!(document.plates().count(), 5);
    assert_eq!(document.plates().active(), 1, "the plate in its place");
    assert!(
        near(centre_of(&ctrl, leaves[0]), Vec3::ZERO),
        "plate 0 stays"
    );
    assert!(
        near(centre_of(&ctrl, leaves[1]), offset(&v, 1)),
        "plate 2 is plate 1"
    );
    assert!(
        near(centre_of(&ctrl, leaves[2]), offset(&v, 4)),
        "plate 5 is plate 4"
    );
    // One undo puts the plate and the models back.
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plates().count(), 6);
    assert!(near(centre_of(&ctrl, leaves[1]), offset(&v, 2)));
    assert!(near(centre_of(&ctrl, leaves[2]), offset(&v, 5)));
}

#[test]
fn a_click_on_a_plate_or_a_model_on_it_makes_it_the_active_one() {
    let v = volume();
    let (ctrl, leaves) = window(&[Vec3::ZERO, offset(&v, 1) + Vec3::new(20.0, 0.0, 0.0)]);
    let document = ctrl.document();
    ctrl.add_plate();
    ctrl.add_plate();
    assert_eq!(
        document.plates().active(),
        0,
        "adding keeps the active plate"
    );
    document.set_active_plate(2);
    // Straight down at the empty middle of plate 1.
    let mut cam = OrbitCamera::new(offset(&v, 1), 200.0);
    cam.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(cam);
    ctrl.run_pick_at(ScreenPoint { x: 100.0, y: 100.0 });
    assert_eq!(document.plates().active(), 1, "the plate clicked");
    document.set_active_plate(2);
    // A model picked brings its plate.
    let mut cam = OrbitCamera::new(Vec3::new(0.0, 0.0, 2.0), 200.0);
    cam.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(cam);
    ctrl.run_pick_at(ScreenPoint { x: 100.0, y: 100.0 });
    assert_eq!(
        document.selection_primary_node_id(),
        Some(leaves[0]),
        "the model is picked"
    );
    assert_eq!(document.plates().active(), 0, "and its plate is active");
}

#[test]
fn a_printer_of_another_size_takes_the_models_with_their_plates() {
    let v = volume();
    let (ctrl, leaves) = window(&[
        Vec3::new(10.0, 0.0, 0.0),
        offset(&v, 1) + Vec3::new(10.0, 0.0, 0.0),
    ]);
    ctrl.add_plate();
    let bigger = BuildVolume::new(160.0, 90.0, 80.0);
    ctrl.set_build_volume(bigger);
    assert!(
        near(centre_of(&ctrl, leaves[0]), Vec3::new(10.0, 0.0, 0.0)),
        "plate 0 stays"
    );
    assert!(
        near(
            centre_of(&ctrl, leaves[1]),
            offset(&bigger, 1) + Vec3::new(10.0, 0.0, 0.0)
        ),
        "the model stays where it was on its plate"
    );
}

#[test]
fn a_project_keeps_how_many_plates_it_has() {
    let (ctrl, _) = window(&[Vec3::ZERO]);
    let document = ctrl.document();
    ctrl.add_plate();
    ctrl.add_plate();
    let payload = document.compose_project_payload(&Default::default());
    assert_eq!(payload.plates, 3);
    // Opened again, the scene has them.
    let reopened = SceneDocument::new();
    reopened.install_project_graph(Scene::new(), PathBuf::from("<test>.bread"), payload);
    assert_eq!(reopened.plates().count(), 3);
    reopened.clear_scene();
    assert_eq!(reopened.plates().count(), 1, "a cleared scene has one");
}

/// With several plates and nothing selected, what stands on the active plate
/// stays lit and the rest dims (#545); with a selection, the selection does.
#[test]
fn off_the_active_plate_the_models_dim() {
    let v = volume();
    let (ctrl, leaves) = window(&[Vec3::ZERO, offset(&v, 1)]);
    let document = ctrl.document();
    assert!(!ctrl.dims_off_plate(), "one plate dims nothing");
    assert!(ctrl.focus().is_empty());
    ctrl.add_plate();
    assert!(ctrl.dims_off_plate());
    assert_eq!(ctrl.focus(), vec![leaves[0]], "plate 0 is active");
    document.set_active_plate(1);
    assert_eq!(ctrl.focus(), vec![leaves[1]]);
    document.set_selection(Selection::One(LeafId(leaves[1])));
    assert!(!ctrl.dims_off_plate(), "a selection is the focus");
    assert_eq!(ctrl.focus(), vec![leaves[1]]);
}

/// Shift on the plate's remove tool takes its models off with it, and the
/// plates after it move up with theirs, in one undo step (#545).
#[test]
fn a_plate_removed_with_its_models_takes_them_off_in_one_step() {
    let v = volume();
    let (ctrl, leaves) = window(&[Vec3::ZERO, offset(&v, 1), offset(&v, 2)]);
    let document = ctrl.document();
    ctrl.add_plate();
    ctrl.add_plate();
    document.set_active_plate(1);
    ctrl.remove_active_plate_with(true);
    assert_eq!(document.plates().count(), 2);
    let gone = document
        .with_scene(|spatial| spatial.leaf(leaves[1]).is_none())
        .expect("a scene");
    assert!(gone, "its model went with it");
    assert!(
        near(centre_of(&ctrl, leaves[2]), offset(&v, 1)),
        "plate 2 is plate 1"
    );
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plates().count(), 3);
    assert!(near(centre_of(&ctrl, leaves[1]), offset(&v, 1)), "back");
    assert!(near(centre_of(&ctrl, leaves[2]), offset(&v, 2)));
}

/// Arrange with several plates works on the active one alone (#545): its
/// models are placed on it, the others stay where they are.
#[test]
fn arrange_works_on_the_active_plate_alone() {
    let v = volume();
    let second = offset(&v, 1);
    // Two models off to a side of plate 1, one on plate 0.
    let (ctrl, leaves) = window(&[
        Vec3::new(-30.0, 0.0, 0.0),
        second + Vec3::new(40.0, 15.0, 0.0),
        second + Vec3::new(42.0, 18.0, 0.0),
    ]);
    let document = ctrl.document();
    ctrl.add_plate();
    document.set_active_plate(1);
    let before = centre_of(&ctrl, leaves[0]);
    crate::arrange_run::arrange_on_plates_now(
        document.as_ref(),
        &v,
        Default::default(),
        crate::arrange_run::ArrangeScope::ActivePlate,
    )
    .expect("arranged");
    assert!(
        near(centre_of(&ctrl, leaves[0]), before),
        "plate 0 untouched"
    );
    for &leaf in &leaves[1..] {
        let c = centre_of(&ctrl, leaf) - second;
        assert!(c.x.abs() <= 50.0 && c.y.abs() <= 25.0, "on plate 1: {c:?}");
    }
    let (a, b) = (centre_of(&ctrl, leaves[1]), centre_of(&ctrl, leaves[2]));
    assert!(
        (a - b).length() >= 4.0,
        "apart, not overlapping: {a:?} {b:?}"
    );
}

/// The active plate's tools lie on its rim (#545): a click on Add adds a
/// plate and leaves the camera where it is; a click on Remove with Shift
/// takes the plate off with its models.
#[test]
fn the_plate_tools_on_its_rim_add_and_remove() {
    let v = volume();
    let (ctrl, _) = window(&[Vec3::ZERO]);
    let document = ctrl.document();
    ctrl.set_section(WorkspaceSection::Transform);
    // Straight down at plate 0's Add tool.
    let add = Vec3::new(
        50.0 + crate::plates::TOOL_INSET_MM + crate::plates::TOOL_MM / 2.0,
        25.0 - crate::plates::TOOL_MM / 2.0,
        0.0,
    );
    let mut cam = OrbitCamera::new(add, 200.0);
    cam.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(cam.clone());
    let centre = ScreenPoint { x: 100.0, y: 100.0 };
    assert_eq!(ctrl.plate_tool_under(centre), Some(0));
    ctrl.run_plate_tool(0, false);
    assert_eq!(document.plates().count(), 2);
    let after = ctrl.camera_for_test().expect("a camera");
    assert_eq!(after.target, cam.target, "the camera stays");
    assert_eq!(after.distance, cam.distance);
    // Off the tools, nothing.
    assert_eq!(
        ctrl.plate_tool_under(ScreenPoint { x: 10.0, y: 10.0 }),
        None
    );
    let _ = v;
}

/// Over any plate's floor, not only the first's, the orbit turns about
/// the point under the cursor (#545).
#[test]
fn the_orbit_pivot_lands_on_any_plate() {
    let v = volume();
    let (ctrl, _) = window(&[Vec3::ZERO]);
    ctrl.add_plate();
    let second = offset(&v, 1) + Vec3::new(10.0, -5.0, 0.0);
    let mut cam = OrbitCamera::new(second, 60.0);
    cam.set_axis_view(AxisView::Top);
    // A target far off, as with every plate framed.
    cam.target = second;
    ctrl.set_camera_for_test(cam.clone());
    let (pivot, source) = super::super::orbit_pivot::choose(
        ctrl.document().as_ref(),
        &cam,
        crate::viewport::types::ViewportSize::new(200.0, 200.0).expect("a size"),
        v,
        ScreenPoint { x: 100.0, y: 100.0 },
    );
    assert_eq!(source, super::super::orbit_pivot::PivotSource::Plate);
    assert!((pivot - second).length() < 1.0, "{pivot:?}");
}

/// Whether `c`, a centre, lies on plate `plate` of [`volume`].
fn on_plate(c: Vec3, plate: u32) -> bool {
    let c = c - offset(&volume(), plate);
    c.x.abs() <= 50.0 && c.y.abs() <= 25.0
}

/// Arrange all packs every plate's models for the fewest plates (#554): a
/// model on each of 2 plates both land on the first, and the second, left
/// empty, goes; one Undo puts both back with the plate.
#[test]
fn arrange_all_packs_every_plate_for_the_fewest_plates() {
    let v = volume();
    let second = offset(&v, 1);
    let (ctrl, leaves) = window(&[
        Vec3::new(-40.0, 18.0, 0.0),
        second + Vec3::new(40.0, -18.0, 0.0),
    ]);
    let document = ctrl.document();
    ctrl.add_plate();
    let before = [centre_of(&ctrl, leaves[0]), centre_of(&ctrl, leaves[1])];
    let applied = crate::arrange_run::arrange_on_plates_now(
        document.as_ref(),
        &v,
        Default::default(),
        crate::arrange_run::ArrangeScope::Plates,
    )
    .expect("arranged");
    assert_eq!((applied.plates_before, applied.plates), (2, 1));
    assert_eq!(document.plates().count(), 1, "the emptied plate goes");
    for &leaf in &leaves {
        assert!(on_plate(centre_of(&ctrl, leaf), 0));
    }
    assert!(document.undo().expect("undoes"), "one step");
    assert_eq!(document.plates().count(), 2);
    assert!(near(centre_of(&ctrl, leaves[0]), before[0]));
    assert!(near(centre_of(&ctrl, leaves[1]), before[1]));
}

/// What does not fit on a plate goes to plates added after it (#554): 10
/// boxes of 20 mm, 4 to a plate, take 3 plates.
#[test]
fn arrange_all_adds_plates_for_what_does_not_fit() {
    let v = volume();
    let places: Vec<Vec3> = (0..10)
        .map(|k| Vec3::new(k as f32 * 3.0 - 15.0, 0.0, 0.0))
        .collect();
    let (ctrl, leaves) = window_of(&places, 5.0);
    let document = ctrl.document();
    let applied = crate::arrange_run::arrange_on_plates_now(
        document.as_ref(),
        &v,
        Default::default(),
        crate::arrange_run::ArrangeScope::Plates,
    )
    .expect("arranged");
    assert_eq!(applied.plates, 3);
    assert!(applied.left_off.is_empty());
    assert_eq!(document.plates().count(), 3);
    let mut on = [0; 3];
    for &leaf in &leaves {
        let c = centre_of(&ctrl, leaf);
        let plate = (0..3).find(|&p| on_plate(c, p)).expect("on a plate");
        on[plate as usize] += 1;
    }
    assert_eq!(on, [4, 4, 2]);
    assert!(document.undo().expect("undoes"), "one step");
    assert_eq!(document.plates().count(), 1);
}

/// The plate's own Arrange packs its plate alone, and what does not fit
/// goes to plates added at the end (#554).
#[test]
fn a_plates_own_arrange_sends_what_does_not_fit_to_new_plates() {
    let v = volume();
    let places: Vec<Vec3> = (0..6)
        .map(|k| Vec3::new(k as f32 * 3.0 - 9.0, 0.0, 0.0))
        .collect();
    let (ctrl, leaves) = window_of(&places, 5.0);
    let document = ctrl.document();
    crate::arrange_run::arrange_on_plates_now(
        document.as_ref(),
        &v,
        Default::default(),
        crate::arrange_run::ArrangeScope::ActivePlate,
    )
    .expect("arranged");
    assert_eq!(document.plates().count(), 2);
    let on_second = leaves
        .iter()
        .filter(|&&leaf| on_plate(centre_of(&ctrl, leaf), 1))
        .count();
    assert_eq!(on_second, 2);
}

/// A model moved by hand counts as placed by hand (#554) until Arrange
/// places it or Undo takes the move back; kept, it stays where it is and
/// the others keep clear of it.
#[test]
fn a_model_placed_by_hand_may_stay_where_it_is() {
    let v = volume();
    let places: Vec<Vec3> = (0..3)
        .map(|k| Vec3::new(k as f32 * 3.0 - 3.0, 0.0, 0.0))
        .collect();
    let (ctrl, leaves) = window_of(&places, 5.0);
    let document = ctrl.document();
    let by_hand = |document: &SceneDocument| {
        crate::arrange_run::placed_by_hand(document, &v, crate::arrange_run::ArrangeScope::Plates)
    };
    assert_eq!(by_hand(&document), 0, "as they came");
    let edit = crate::plate_layout::moved_by(&document, leaves[0], Vec3::new(25.0, 10.0, 0.0))
        .expect("a move");
    document.apply_edit(edit.clone()).expect("applies");
    document.push_edit(edit);
    assert_eq!(by_hand(&document), 1, "moved by hand");
    assert!(document.undo().expect("undoes"));
    assert_eq!(by_hand(&document), 0, "the move undone");
    assert!(document.redo().expect("redoes"));
    let kept = centre_of(&ctrl, leaves[0]);
    crate::arrange_run::arrange_across_now(
        document.as_ref(),
        &v,
        Default::default(),
        crate::arrange_run::ArrangeScope::Plates,
        true,
    )
    .expect("arranged");
    assert!(near(centre_of(&ctrl, leaves[0]), kept), "it stayed");
    assert_eq!(by_hand(&document), 1, "still by hand");
    for &leaf in &leaves[1..] {
        let c = centre_of(&ctrl, leaf);
        assert!(
            (c - kept).x.abs() >= 20.0 || (c - kept).y.abs() >= 20.0,
            "clear"
        );
    }
    // Arranged too, it is not by hand any more.
    crate::arrange_run::arrange_across_now(
        document.as_ref(),
        &v,
        Default::default(),
        crate::arrange_run::ArrangeScope::Plates,
        false,
    )
    .expect("arranged");
    assert_eq!(by_hand(&document), 0);
}

/// A project keeps which models were placed by hand (#554).
#[test]
fn a_project_keeps_the_models_placed_by_hand() {
    let v = volume();
    let (ctrl, leaves) = window_of(&[Vec3::ZERO, Vec3::new(30.0, 0.0, 0.0)], 2.0);
    let document = ctrl.document();
    let edit = crate::plate_layout::moved_by(&document, leaves[1], Vec3::new(5.0, 5.0, 0.0))
        .expect("a move");
    document.apply_edit(edit.clone()).expect("applies");
    document.push_edit(edit);
    let payload = document.compose_project_payload(&Default::default());
    assert_eq!(payload.hand_placed.len(), 1);
    let graph = document
        .with_scene(|spatial| spatial.graph().clone())
        .expect("a scene");
    let reopened = SceneDocument::new();
    reopened.install_project_graph(graph, "hand.bread".into(), payload);
    assert_eq!(reopened.hand_placed(), vec![leaves[1]]);
    let _ = v;
}

/// Plates are "Plate N" as they come (#545), and keep their names when one
/// before them goes: with Plate 2 removed, Plate 3 stays Plate 3; the next
/// one added takes the first number no plate has.
#[test]
fn plates_keep_their_names_when_one_before_them_goes() {
    let (ctrl, _) = window(&[Vec3::ZERO]);
    let document = ctrl.document();
    ctrl.add_plate();
    ctrl.add_plate();
    assert_eq!(document.plate_names(), ["Plate 1", "Plate 2", "Plate 3"]);
    document.set_active_plate(1);
    ctrl.remove_active_plate();
    assert_eq!(document.plate_names(), ["Plate 1", "Plate 3"]);
    ctrl.add_plate();
    assert_eq!(document.plate_names(), ["Plate 1", "Plate 3", "Plate 4"]);
    assert!(document.undo().expect("undoes"));
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plate_names(), ["Plate 1", "Plate 2", "Plate 3"]);
}

/// A plate's name edited where it lies (#545): what is typed is the
/// plate's name once kept, as one undo step; Escape leaves it as it was,
/// and a blank name changes nothing.
#[test]
fn a_plate_name_is_edited_in_place_and_undoes() {
    let (ctrl, _) = window(&[Vec3::ZERO]);
    let document = ctrl.document();
    ctrl.add_plate();
    ctrl.start_plate_name_edit(1);
    ctrl.plate_name_edited("Esqueletos", 10, 10);
    ctrl.plate_name_done(true);
    assert_eq!(document.plate_names(), ["Plate 1", "Esqueletos"]);
    ctrl.start_plate_name_edit(0);
    ctrl.plate_name_edited("Nada", 4, 4);
    ctrl.plate_name_done(false);
    assert_eq!(document.plate_names()[0], "Plate 1", "Escape keeps it");
    ctrl.start_plate_name_edit(0);
    ctrl.plate_name_edited("   ", 3, 3);
    ctrl.plate_name_done(true);
    assert_eq!(
        document.plate_names()[0],
        "Plate 1",
        "a blank name is no name"
    );
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plate_names(), ["Plate 1", "Plate 2"]);
    // The project keeps the names.
    assert!(document.redo().expect("redoes"));
    let payload = document.compose_project_payload(&Default::default());
    assert_eq!(payload.plate_names, ["Plate 1", "Esqueletos"]);
    let reopened = SceneDocument::new();
    reopened.install_project_graph(Scene::new(), PathBuf::from("<test>.bread"), payload);
    assert_eq!(reopened.plate_names(), ["Plate 1", "Esqueletos"]);
}

/// A double click on a plate's name starts its edit; one click makes the
/// plate active (#545).
#[test]
fn a_double_click_on_a_plate_name_edits_it() {
    let v = volume();
    let (ctrl, _) = window(&[Vec3::ZERO]);
    let document = ctrl.document();
    ctrl.add_plate();
    let [x0, y0, x1, y1] = crate::plates::name_box(&v);
    let on_name = offset(&v, 1) + Vec3::new((x0 + x1) / 2.0, (y0 + y1) / 2.0, 0.0);
    let mut cam = OrbitCamera::new(on_name, 200.0);
    cam.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(cam);
    let centre = ScreenPoint { x: 100.0, y: 100.0 };
    assert_eq!(ctrl.plate_name_under(centre), Some(1));
    ctrl.plate_name_clicked(1);
    assert_eq!(document.plates().active(), 1);
    assert!(
        ctrl.plate_name_edit.borrow().is_none(),
        "one click does not edit"
    );
    ctrl.plate_name_clicked(1);
    assert_eq!(
        ctrl.plate_name_edit.borrow().as_ref().map(|e| (
            e.plate,
            e.text.clone(),
            e.anchor,
            e.caret
        )),
        Some((1, "Plate 2".to_owned(), 0, 7)),
        "the whole name selected"
    );
}

/// Fill plate fills the active plate alone (#548): the copies stand on it,
/// and the other plate is left as it is.
#[test]
fn fill_plate_fills_the_active_plate_alone() {
    let v = volume();
    let second = offset(&v, 1);
    let (ctrl, leaves) = window(&[Vec3::new(-30.0, 0.0, 0.0), second]);
    let document = ctrl.document();
    ctrl.add_plate();
    document.set_active_plate(1);
    document.set_selection(Selection::One(LeafId(leaves[1])));
    let before = centre_of(&ctrl, leaves[0]);
    let added =
        crate::arrange_run::fill_active_plate_now(document.as_ref(), &v, Default::default())
            .expect("filled");
    assert!(added > 3, "copies fill plate 1: {added}");
    assert!(
        near(centre_of(&ctrl, leaves[0]), before),
        "plate 0 untouched"
    );
    let on_plates = ctrl.models_on_plates();
    let on_second = on_plates.iter().filter(|&&(_, p)| p == 1).count();
    assert_eq!(on_second, added + 1, "every copy on plate 1: {on_plates:?}");
    assert_eq!(on_plates.iter().filter(|&&(_, p)| p == 0).count(), 1);
}

/// The selection sent to another plate (#549) stands there where it stood on
/// its own, as one undo step, and that plate is the active one.
#[test]
fn the_selection_goes_to_another_plate_where_it_stood() {
    let v = volume();
    let (ctrl, leaves) = window(&[Vec3::new(-30.0, 10.0, 0.0), Vec3::new(20.0, 0.0, 0.0)]);
    let document = ctrl.document();
    ctrl.add_plate();
    ctrl.add_plate();
    document.set_selection(Selection::One(LeafId(leaves[0])));
    let before = centre_of(&ctrl, leaves[0]);
    ctrl.send_selection_to_plate(2);
    assert!(
        near(centre_of(&ctrl, leaves[0]), before + offset(&v, 2)),
        "on plate 2 where it stood"
    );
    assert!(
        near(centre_of(&ctrl, leaves[1]), Vec3::new(20.0, 0.0, 0.0)),
        "the other stays"
    );
    assert_eq!(document.plates().active(), 2);
    assert!(document.undo().expect("undoes"));
    assert!(near(centre_of(&ctrl, leaves[0]), before), "one step back");
}

/// Shift on the plus tool (#551): a new plate with a copy of every model of
/// the active one, each where it stands on its plate; one undo step.
#[test]
fn shift_on_plus_duplicates_the_plate_with_its_models() {
    let v = volume();
    let (ctrl, leaves) = window(&[Vec3::new(-30.0, 10.0, 0.0), Vec3::new(20.0, -5.0, 0.0)]);
    let document = ctrl.document();
    ctrl.duplicate_active_plate();
    assert_eq!(document.plates().count(), 2);
    assert_eq!(document.plates().active(), 0, "the active plate stays");
    let on = ctrl.models_on_plates();
    assert_eq!(on.iter().filter(|&&(_, p)| p == 1).count(), 2, "{on:?}");
    let copies: Vec<Vec3> = on
        .iter()
        .filter(|&&(id, p)| p == 1 && !leaves.contains(&id))
        .map(|&(id, _)| centre_of(&ctrl, id) - offset(&v, 1))
        .collect();
    for original in [Vec3::new(-30.0, 10.0, 0.0), Vec3::new(20.0, -5.0, 0.0)] {
        assert!(
            copies
                .iter()
                .any(|&c| (c.truncate() - original.truncate()).length() < 0.01),
            "{original:?} in {copies:?}"
        );
    }
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plates().count(), 1, "one step");
    assert_eq!(ctrl.models_on_plates().len(), 2);
}

/// A paste with no room on the plates goes on a plate added after the last
/// (#557), in one step of history with it; the next paste fills that plate
/// before another is added.
#[test]
fn a_paste_with_no_room_goes_on_a_new_plate() {
    let v = volume();
    // Two boxes of 40 mm fill the plate of 100 x 50.
    let (ctrl, leaves) = window_of(
        &[Vec3::new(-25.0, 0.0, 0.0), Vec3::new(25.0, 0.0, 0.0)],
        10.0,
    );
    let document = ctrl.document();
    document.set_selection(Selection::One(LeafId(leaves[0])));
    let mut clipboard = clipboard::clipboard_from_document_selection(&document).expect("copies");
    let steps = document.undo_len();

    let copy = clipboard::paste(&document, &clipboard, [0.0, 0.0], &v)
        .expect("pastes")
        .expect("a copy");
    clipboard.bump_cascade();
    assert_eq!(document.plates().count(), 2, "a plate added");
    assert_eq!(document.undo_len(), steps + 1, "one step");
    assert!(on_plate(centre_of(&ctrl, copy), 1), "on the new plate");

    let next = clipboard::paste(&document, &clipboard, [0.0, 0.0], &v)
        .expect("pastes")
        .expect("a copy");
    assert_eq!(document.plates().count(), 2, "the new plate had room");
    assert!(on_plate(centre_of(&ctrl, next), 1));

    assert!(document.undo().expect("undoes"));
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plates().count(), 1, "the plate goes with the copy");
}
