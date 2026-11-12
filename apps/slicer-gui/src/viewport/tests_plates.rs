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
    let ctrl = ViewportController::new_for_test();
    ctrl.set_build_volume(volume());
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    let mesh = Arc::new(box_mesh());
    let mut leaves = Vec::new();
    for p in places {
        let at = Transform3D::Affine(AffineTransform3D::from_translation([
            f64::from(p.x),
            f64::from(p.y),
            0.0,
        ]));
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
    assert_eq!(document.plates().active(), 2, "the new plate is active");
    assert!(document.is_dirty(), "the project changed");
    assert!(document.undo().expect("undoes"));
    assert_eq!(document.plates().count(), 2);
    assert!(document.redo().expect("redoes"));
    assert_eq!(document.plates().count(), 3);
}

#[test]
fn a_plate_with_a_model_on_it_stays() {
    let v = volume();
    let (ctrl, _) = window(&[Vec3::ZERO, offset(&v, 1)]);
    let document = ctrl.document();
    ctrl.add_plate();
    document.set_active_plate(1);
    ctrl.remove_active_plate();
    assert_eq!(document.plates().count(), 2, "the model keeps its plate");
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
    assert_eq!(document.plates().active(), 0);
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
    assert_eq!(document.plates().active(), 2);
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
    assert_eq!(ctrl.focus(), vec![leaves[1]], "plate 1 is active");
    document.set_active_plate(0);
    assert_eq!(ctrl.focus(), vec![leaves[0]]);
    document.set_selection(Selection::One(LeafId(leaves[1])));
    assert!(!ctrl.dims_off_plate(), "a selection is the focus");
    assert_eq!(ctrl.focus(), vec![leaves[1]]);
}
