/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! The second round of the gizmo: plane handles (#207), pressing and
//! dragging a model to move it (#209), the focused style (#274), the gizmo
//! size (#276), each object's own pivot with several selected (#218) and R
//! without an axis from an axis-aligned view (#351).

use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, NodeId, Scene};

use super::controller::ViewportController;
use super::gizmo_drag::{DragOp, GizmoState};
use super::types::ScreenPoint;
use crate::camera::{AxisView, OrbitCamera};
use crate::gizmo::dimensions::HANDLE_PIXEL_LENGTH;
use crate::gizmo::look::{GizmoStyle, OUTLINE_METRICS};
use crate::mesh_render::default_fov_y;
use crate::picking;
use crate::selection::{LeafId, Selection};
use crate::{PointerModifiers, WorkspaceSection};

const W: f32 = 800.0;
const H: f32 = 600.0;
/// Half the side of the test cubes, in millimetres.
const HALF: f64 = 10.0;

/// A 20 mm cube.
fn big_cube() -> sc_mesh_core::IndexedMesh {
    let mut mesh = unit_cube_mesh();
    mesh.vertices = mesh
        .vertices
        .iter()
        .map(|v| Vertex::new(v.coords().map(|c| c * 2.0 * HALF as f32)))
        .collect();
    mesh
}

/// A controller in the Transform tab with a 20 mm cube resting on the plate
/// at each `(x, y)`, nothing selected, seen from `view`.
fn plate(at: &[(f64, f64)], view: AxisView) -> ViewportController {
    let mut scene = Scene::new();
    for &(x, y) in at {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([x, y, HALF])),
                LeafPayload::Mesh(Arc::new(big_cube())),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("a fresh root");
    }
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(W, H);
    ctrl.set_section(WorkspaceSection::Transform);
    let mut camera = OrbitCamera::new(Vec3::ZERO, 60.0);
    camera.set_axis_view(view);
    ctrl.set_camera_for_test(camera);
    ctrl
}

fn none() -> PointerModifiers {
    PointerModifiers::default()
}

fn shift() -> PointerModifiers {
    PointerModifiers {
        shift: true,
        ..Default::default()
    }
}

fn camera(ctrl: &ViewportController) -> OrbitCamera {
    ctrl.camera_for_test().expect("a camera")
}

fn pixel(ctrl: &ViewportController, p: Vec3) -> ScreenPoint {
    let (x, y) =
        picking::project_world_to_screen(&camera(ctrl), default_fov_y(), W, H, p).expect("in view");
    ScreenPoint::new(x, y)
}

fn leaf(ctrl: &ViewportController, row: usize) -> NodeId {
    ctrl.document()
        .with_scene(|s| s.leaf_id_at_index(row))
        .flatten()
        .expect("a leaf")
}

fn world(ctrl: &ViewportController, row: usize) -> Transform3D {
    let id = leaf(ctrl, row);
    ctrl.document()
        .with_scene(|s| s.leaf(id).map(|q| q.world_transform()))
        .flatten()
        .expect("a leaf")
}

fn translation(ctrl: &ViewportController, row: usize) -> Vec3 {
    match world(ctrl, row) {
        Transform3D::Identity => Vec3::ZERO,
        Transform3D::Affine(a) => Vec3::from(a.translation().map(|c| c as f32)),
    }
}

fn linear(ctrl: &ViewportController, row: usize) -> [[f64; 3]; 3] {
    match world(ctrl, row) {
        Transform3D::Identity => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        Transform3D::Affine(a) => *a.linear(),
    }
}

fn selected(ctrl: &ViewportController) -> Vec<NodeId> {
    ctrl.document().selection_node_ids()
}

fn select(ctrl: &ViewportController, rows: &[usize]) {
    let leaves: Vec<LeafId> = rows.iter().map(|&r| LeafId(leaf(ctrl, r))).collect();
    ctrl.document().set_selection(Selection::of(leaves));
}

/// A point on the top face of the cube centred at `(x, y)` whose pixel no
/// gizmo handle covers: the cursor hovering there lights no handle.
fn top_face(ctrl: &ViewportController, x: f32, y: f32) -> Vec3 {
    let z = 2.0 * HALF as f32;
    for r in [5.0f32, 7.0, 8.5] {
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let p = Vec3::new(x + r * a.cos(), y + r * a.sin(), z);
            ctrl.on_pointer_move(pixel(ctrl, p), none());
            if ctrl.gizmo_hover_for_test().is_none() {
                return p;
            }
        }
    }
    panic!("every point of the top face is under a handle");
}

/// Where the cursor ray at `p` cuts the horizontal plane at height `z`.
fn on_plane_z(ctrl: &ViewportController, p: ScreenPoint, z: f32) -> Vec3 {
    let ray = picking::ray_from_pixel(&camera(ctrl), default_fov_y(), W, H, p.x, p.y);
    let t = (z - ray.origin.z) / ray.direction.z;
    ray.origin + ray.direction * t
}

fn close(a: Vec3, b: Vec3, eps: f32) -> bool {
    (a - b).length() < eps
}

/// A slanted view: the body drags need a camera that looks down at an angle.
fn slanted(at: &[(f64, f64)]) -> ViewportController {
    let ctrl = plate(at, AxisView::Top);
    let mut camera = OrbitCamera::new(Vec3::ZERO, 60.0);
    camera.yaw = -2.2;
    camera.pitch = 0.9;
    ctrl.set_camera_for_test(camera);
    ctrl
}

// ---------------------------------------------------------------- #209 --

#[test]
fn pressing_and_dragging_an_unselected_model_selects_it_and_moves_it_on_the_plate() {
    let ctrl = slanted(&[(-40.0, 0.0), (40.0, 0.0)]);
    let grab = top_face(&ctrl, 40.0, 0.0);
    let press = pixel(&ctrl, grab);
    let before = translation(&ctrl, 1);
    ctrl.on_pointer_down(press, none());
    // Within the click threshold nothing moves yet.
    ctrl.on_pointer_move(ScreenPoint::new(press.x + 2.0, press.y), none());
    assert!(close(translation(&ctrl, 1), before, 1e-6));
    let to = ScreenPoint::new(press.x - 60.0, press.y + 35.0);
    ctrl.on_pointer_move(to, none());
    assert_eq!(
        selected(&ctrl),
        vec![leaf(&ctrl, 1)],
        "selected by the press"
    );
    let after = translation(&ctrl, 1);
    let moved = after - before;
    assert!(moved.length() > 5.0, "{moved}");
    assert!(moved.z.abs() < 1e-6, "Z stays: {moved}");
    // The grabbed point stays under the cursor.
    let target = on_plane_z(&ctrl, to, grab.z);
    assert!(
        close(grab + moved, target, 1e-3),
        "{} vs {target}",
        grab + moved
    );
    ctrl.on_pointer_up(to, none());
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(-40.0, 0.0, 10.0),
        1e-6
    ));
    // One undo step puts it back, and the selection stays.
    let document = ctrl.document();
    assert!(document.undo().unwrap());
    assert!(!document.can_undo(), "one step");
    assert!(close(translation(&ctrl, 1), before, 1e-6));
}

#[test]
fn a_click_on_a_model_only_selects_it() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    let press = pixel(&ctrl, top_face(&ctrl, 0.0, 0.0));
    ctrl.on_pointer_down(press, none());
    ctrl.on_pointer_move(ScreenPoint::new(press.x + 2.0, press.y + 1.0), none());
    ctrl.on_pointer_up(ScreenPoint::new(press.x + 2.0, press.y + 1.0), none());
    assert_eq!(selected(&ctrl), vec![leaf(&ctrl, 0)]);
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(0.0, 0.0, 10.0),
        1e-9
    ));
    assert!(!ctrl.document().can_undo());
}

#[test]
fn a_press_on_the_body_of_the_selected_model_off_the_handles_moves_it() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    select(&ctrl, &[0]);
    let grab = top_face(&ctrl, 0.0, 0.0);
    let press = pixel(&ctrl, grab);
    ctrl.on_pointer_down(press, none());
    let to = ScreenPoint::new(press.x + 50.0, press.y + 20.0);
    ctrl.on_pointer_move(to, none());
    match ctrl.gizmo_state_for_test() {
        GizmoState::Drag {
            op: DragOp::Plane { body, .. },
            ..
        } => assert!(body),
        other => panic!("expected a body drag, got {other:?}"),
    }
    let moved = translation(&ctrl, 0) - Vec3::new(0.0, 0.0, 10.0);
    assert!(close(grab + moved, on_plane_z(&ctrl, to, grab.z), 1e-3));
    ctrl.on_pointer_up(to, none());
}

#[test]
fn a_press_on_a_handle_of_the_selected_model_still_drags_the_handle() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    select(&ctrl, &[0]);
    let pivot = Vec3::new(0.0, 0.0, 10.0);
    let scale =
        crate::gizmo::gizmo_scale(&camera(&ctrl), pivot, default_fov_y(), H).expect("in view");
    // The X cone, which sits over the model's body.
    let press = pixel(&ctrl, pivot + Vec3::X * scale * 1.1);
    ctrl.on_pointer_down(press, none());
    ctrl.on_pointer_move(ScreenPoint::new(press.x + 30.0, press.y), none());
    assert!(matches!(
        ctrl.gizmo_state_for_test(),
        GizmoState::Drag {
            op: DragOp::Translate { .. },
            ..
        }
    ));
}

#[test]
fn esc_puts_a_dragged_model_back_and_records_nothing() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    let press = pixel(&ctrl, top_face(&ctrl, 0.0, 0.0));
    ctrl.on_pointer_down(press, none());
    ctrl.on_pointer_move(ScreenPoint::new(press.x + 80.0, press.y), none());
    assert!(!close(
        translation(&ctrl, 0),
        Vec3::new(0.0, 0.0, 10.0),
        1.0
    ));
    ctrl.on_clear_selection();
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(0.0, 0.0, 10.0),
        1e-9
    ));
    ctrl.on_pointer_up(ScreenPoint::new(press.x + 80.0, press.y), none());
    assert!(!ctrl.document().can_undo());
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(0.0, 0.0, 10.0),
        1e-9
    ));
}

#[test]
fn a_body_drag_does_nothing_in_the_supports_tab() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    ctrl.set_section(WorkspaceSection::Supports);
    let press = pixel(&ctrl, top_face(&ctrl, 0.0, 0.0));
    ctrl.on_pointer_down(press, none());
    ctrl.on_pointer_move(ScreenPoint::new(press.x + 80.0, press.y), none());
    ctrl.on_pointer_up(ScreenPoint::new(press.x + 80.0, press.y), none());
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(0.0, 0.0, 10.0),
        1e-9
    ));
    assert!(!ctrl.document().can_undo());
}

#[test]
fn dragging_a_member_of_a_multi_selection_moves_the_group_in_one_step() {
    let ctrl = slanted(&[(-40.0, 0.0), (0.0, 0.0), (40.0, 0.0)]);
    select(&ctrl, &[0, 2]);
    let before: Vec<Vec3> = (0..3).map(|r| translation(&ctrl, r)).collect();
    let press = pixel(&ctrl, top_face(&ctrl, 40.0, 0.0));
    ctrl.on_pointer_down(press, none());
    let to = ScreenPoint::new(press.x - 40.0, press.y + 30.0);
    ctrl.on_pointer_move(to, none());
    ctrl.on_pointer_up(to, none());
    let d0 = translation(&ctrl, 0) - before[0];
    let d2 = translation(&ctrl, 2) - before[2];
    assert!(d2.length() > 3.0, "{d2}");
    assert!(close(d0, d2, 1e-5), "together: {d0} {d2}");
    assert!(d0.z.abs() < 1e-6);
    assert!(
        close(translation(&ctrl, 1), before[1], 1e-9),
        "the other stays"
    );
    assert_eq!(selected(&ctrl).len(), 2, "the selection stays");
    let document = ctrl.document();
    assert!(document.undo().unwrap());
    assert!(!document.can_undo(), "one step");
    for (r, &b) in before.iter().enumerate() {
        assert!(close(translation(&ctrl, r), b, 1e-6));
    }
}

#[test]
fn dragging_a_model_outside_the_selection_selects_only_it() {
    let ctrl = slanted(&[(-40.0, 0.0), (0.0, 60.0), (40.0, 0.0)]);
    select(&ctrl, &[0, 2]);
    let press = pixel(&ctrl, top_face(&ctrl, 0.0, 60.0));
    ctrl.on_pointer_down(press, none());
    let to = ScreenPoint::new(press.x + 40.0, press.y);
    ctrl.on_pointer_move(to, none());
    ctrl.on_pointer_up(to, none());
    assert_eq!(selected(&ctrl), vec![leaf(&ctrl, 1)]);
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(-40.0, 0.0, 10.0),
        1e-9
    ));
    assert!(!close(
        translation(&ctrl, 1),
        Vec3::new(0.0, 60.0, 10.0),
        1.0
    ));
}

#[test]
fn a_shift_press_on_a_model_still_draws_a_box_and_moves_nothing() {
    let ctrl = slanted(&[(-40.0, 0.0), (40.0, 0.0)]);
    let press = pixel(&ctrl, top_face(&ctrl, 40.0, 0.0));
    ctrl.on_pointer_down(press, shift());
    let to = ScreenPoint::new(press.x - 60.0, press.y + 10.0);
    ctrl.on_pointer_move(to, shift());
    ctrl.on_pointer_up(to, shift());
    assert!(close(
        translation(&ctrl, 1),
        Vec3::new(40.0, 0.0, 10.0),
        1e-9
    ));
    assert_eq!(selected(&ctrl), vec![leaf(&ctrl, 1)], "the box took it");
    assert!(!ctrl.document().can_undo());
}

#[test]
fn a_level_camera_holds_a_body_drag_where_it_was() {
    let ctrl = plate(&[(0.0, 0.0)], AxisView::Front);
    // Seen level from the front, a press on the front face.
    let grab = Vec3::new(-4.0, -HALF as f32, 6.0);
    let press = pixel(&ctrl, grab);
    ctrl.on_pointer_down(press, none());
    ctrl.on_pointer_move(ScreenPoint::new(press.x + 60.0, press.y), none());
    // The ray runs almost along the plate: no jump, the model stays put.
    let t = translation(&ctrl, 0);
    assert!(close(t, Vec3::new(0.0, 0.0, 10.0), 1e-6), "{t}");
}

// ---------------------------------------------------------------- #207 --

/// The centre of the plane square perpendicular to `normal` of the gizmo at
/// `pivot`, on screen.
fn square(ctrl: &ViewportController, pivot: Vec3, normal: crate::gizmo::axis::Axis) -> ScreenPoint {
    let scale =
        crate::gizmo::gizmo_scale(&camera(ctrl), pivot, default_fov_y(), H).expect("in view");
    let (u, v) = normal.plane_axes();
    let camera = camera(ctrl);
    let view = (camera.target - camera.eye()).normalize();
    let (su, sv) = crate::gizmo::hit_test::plane_quadrant(normal, view);
    let off = OUTLINE_METRICS.plane_offset_px / HANDLE_PIXEL_LENGTH;
    pixel(
        ctrl,
        pivot + (u.world_dir() * su + v.world_dir() * sv) * scale * off,
    )
}

#[test]
fn the_xy_square_moves_the_model_on_the_plate_and_keeps_the_grab_under_the_cursor() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    select(&ctrl, &[0]);
    let pivot = Vec3::new(0.0, 0.0, 10.0);
    let press = square(&ctrl, pivot, crate::gizmo::axis::Axis::Z);
    let grab = on_plane_z(&ctrl, press, pivot.z);
    ctrl.on_pointer_down(press, none());
    match ctrl.gizmo_state_for_test() {
        GizmoState::Drag {
            op: DragOp::Plane { normal, body, .. },
            ..
        } => {
            assert_eq!(normal, crate::gizmo::axis::Axis::Z);
            assert!(!body);
        }
        other => panic!("expected a plane drag, got {other:?}"),
    }
    let to = ScreenPoint::new(press.x + 70.0, press.y - 25.0);
    ctrl.on_pointer_move(to, none());
    let moved = translation(&ctrl, 0) - pivot;
    assert!(moved.z.abs() < 1e-6, "Z stays: {moved}");
    assert!(moved.x.abs() > 1.0 && moved.y.abs() > 1.0, "{moved}");
    assert!(close(grab + moved, on_plane_z(&ctrl, to, pivot.z), 1e-3));
    ctrl.on_pointer_up(to, none());
    assert!(ctrl.document().undo().unwrap());
    assert!(!ctrl.document().can_undo());
}

#[test]
fn the_xz_square_moves_the_model_upright_and_leaves_y() {
    let ctrl = plate(&[(0.0, 0.0)], AxisView::Front);
    let mut camera = OrbitCamera::new(Vec3::ZERO, 60.0);
    camera.set_axis_view(AxisView::Front);
    camera.pitch = 0.3;
    ctrl.set_camera_for_test(camera);
    select(&ctrl, &[0]);
    let pivot = Vec3::new(0.0, 0.0, 10.0);
    let press = square(&ctrl, pivot, crate::gizmo::axis::Axis::Y);
    ctrl.on_pointer_down(press, none());
    let to = ScreenPoint::new(press.x + 40.0, press.y - 40.0);
    ctrl.on_pointer_move(to, none());
    ctrl.on_pointer_up(to, none());
    let moved = translation(&ctrl, 0) - pivot;
    assert!(moved.y.abs() < 1e-6, "Y stays: {moved}");
    assert!(moved.x.abs() > 1.0 && moved.z > 1.0, "{moved}");
}

#[test]
fn a_square_seen_edge_on_cannot_be_grabbed() {
    // From the front, the XY square is edge-on.
    let ctrl = plate(&[(0.0, 0.0)], AxisView::Front);
    select(&ctrl, &[0]);
    let pivot = Vec3::new(0.0, 0.0, 10.0);
    let press = square(&ctrl, pivot, crate::gizmo::axis::Axis::Z);
    ctrl.on_pointer_down(press, none());
    assert!(!matches!(
        ctrl.gizmo_state_for_test(),
        GizmoState::Drag {
            op: DragOp::Plane { body: false, .. },
            ..
        }
    ));
}

// ---------------------------------------------------- #274 and #276 --

#[test]
fn the_focused_style_grabs_only_the_handles_of_the_rail_tool() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    select(&ctrl, &[0]);
    ctrl.set_gizmo_prefs(GizmoStyle::Focused, 100);
    let pivot = Vec3::new(0.0, 0.0, 10.0);
    let scale = crate::gizmo::gizmo_scale(&camera(&ctrl), pivot, default_fov_y(), H).unwrap();
    let cone = pixel(&ctrl, pivot + Vec3::X * scale * 1.1);
    let grabs = |p: ScreenPoint| {
        ctrl.on_pointer_down(p, none());
        let state = ctrl.gizmo_state_for_test();
        if matches!(state, GizmoState::Drag { .. }) {
            ctrl.on_clear_selection();
        }
        ctrl.on_pointer_up(p, none());
        // A press that grabbed nothing was a click: it may have cleared the
        // selection.
        select(&ctrl, &[0]);
        state
    };
    // Move (the default tool): the arrow grabs.
    assert!(matches!(
        grabs(cone),
        GizmoState::Drag {
            op: DragOp::Translate { .. },
            ..
        }
    ));
    // Rotate: the arrow is gone; the Z ring, larger, grabs.
    ctrl.on_gizmo_focus_rotate();
    assert_eq!(
        ctrl.gizmo_look_for_test().tool,
        crate::gizmo::look::GizmoTool::Rotate
    );
    assert!(!matches!(
        grabs(cone),
        GizmoState::Drag {
            op: DragOp::Translate { .. },
            ..
        }
    ));
    let toward = camera(&ctrl).eye() - pivot;
    let flat = Vec3::new(toward.x, toward.y, 0.0).normalize();
    let ring = pixel(
        &ctrl,
        pivot + flat * scale * crate::gizmo::look::FOCUSED_METRICS.ring_radius,
    );
    assert!(matches!(
        grabs(ring),
        GizmoState::Drag {
            op: DragOp::Rotate { .. },
            ..
        }
    ));
    // Scale: the ball at the centre scales evenly.
    ctrl.on_gizmo_focus_scale();
    assert!(matches!(
        grabs(pixel(&ctrl, pivot)),
        GizmoState::Drag {
            op: DragOp::ScaleUniform { .. },
            ..
        }
    ));
}

#[test]
fn the_outline_style_keeps_every_handle_whatever_the_rail_tool() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    select(&ctrl, &[0]);
    ctrl.on_gizmo_focus_rotate();
    let pivot = Vec3::new(0.0, 0.0, 10.0);
    let scale = crate::gizmo::gizmo_scale(&camera(&ctrl), pivot, default_fov_y(), H).unwrap();
    ctrl.on_pointer_down(pixel(&ctrl, pivot + Vec3::X * scale * 1.1), none());
    assert!(matches!(
        ctrl.gizmo_state_for_test(),
        GizmoState::Drag {
            op: DragOp::Translate { .. },
            ..
        }
    ));
}

#[test]
fn a_larger_gizmo_reaches_farther() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    select(&ctrl, &[0]);
    let pivot = Vec3::new(0.0, 0.0, 10.0);
    let scale = crate::gizmo::gizmo_scale(&camera(&ctrl), pivot, default_fov_y(), H).unwrap();
    // 1.1 shafts out at 150 %: past the arrow at 100 %.
    let far = pixel(&ctrl, pivot + Vec3::X * scale * 1.1 * 1.5);
    ctrl.on_pointer_down(far, none());
    let at_100 = ctrl.gizmo_state_for_test();
    if matches!(at_100, GizmoState::Drag { .. }) {
        ctrl.on_clear_selection();
    }
    ctrl.on_pointer_up(far, none());
    select(&ctrl, &[0]);
    assert!(!matches!(
        at_100,
        GizmoState::Drag {
            op: DragOp::Translate { .. },
            ..
        }
    ));
    ctrl.set_gizmo_prefs(GizmoStyle::Outline, 150);
    ctrl.on_pointer_down(far, none());
    assert!(matches!(
        ctrl.gizmo_state_for_test(),
        GizmoState::Drag {
            op: DragOp::Translate { .. },
            ..
        }
    ));
}

// ---------------------------------------------------------------- #218 --

/// Two cubes resting 60 mm apart, both selected, seen from above.
fn pair() -> ViewportController {
    let ctrl = plate(&[(-30.0, 0.0), (30.0, 0.0)], AxisView::Top);
    select(&ctrl, &[0, 1]);
    ctrl
}

/// A quarter turn of the cursor about the projected centre of the pair.
fn quarter_turn(ctrl: &ViewportController, keys: &[i32], axis: Option<i32>) {
    let centre = pixel(ctrl, Vec3::new(0.0, 0.0, HALF as f32));
    let start = ScreenPoint::new(centre.x + 60.0, centre.y);
    for &code in keys {
        ctrl.on_modal_transform_start(code, start.x, start.y);
    }
    if let Some(axis) = axis {
        ctrl.on_modal_transform_axis(axis, start.x, start.y);
    }
    ctrl.on_pointer_move(ScreenPoint::new(centre.x, centre.y - 60.0), none());
}

#[test]
fn r_r_z_turns_each_object_about_its_own_vertical_axis_in_one_undo_step() {
    let ctrl = pair();
    quarter_turn(&ctrl, &[1, 1], Some(2));
    ctrl.on_modal_transform_finish(true);
    // Each stayed where it was and turned a quarter on itself.
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(-30.0, 0.0, 10.0),
        1e-4
    ));
    assert!(close(
        translation(&ctrl, 1),
        Vec3::new(30.0, 0.0, 10.0),
        1e-4
    ));
    for row in 0..2 {
        let l = linear(&ctrl, row);
        assert!(
            l[0][0].abs() < 1e-4 && (l[1][0].abs() - 1.0).abs() < 1e-4,
            "{l:?}"
        );
        assert!((l[2][2] - 1.0).abs() < 1e-9);
    }
    // A turn about world Z: the supports of each would follow it.
    let start = Transform3D::Affine(AffineTransform3D::from_translation([-30.0, 0.0, HALF]));
    assert!(crate::support_follow::supports_follow(
        &start,
        &world(&ctrl, 0)
    ));
    let document = ctrl.document();
    assert!(document.undo().unwrap());
    assert!(!document.can_undo(), "one step");
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(-30.0, 0.0, 10.0),
        1e-9
    ));
    assert!((linear(&ctrl, 0)[0][0] - 1.0).abs() < 1e-9);
}

#[test]
fn r_z_still_turns_the_group_about_its_shared_centre() {
    let ctrl = pair();
    quarter_turn(&ctrl, &[1], Some(2));
    ctrl.on_modal_transform_finish(true);
    let (a, b) = (translation(&ctrl, 0), translation(&ctrl, 1));
    assert!(a.x.abs() < 1e-3 && (a.y.abs() - 30.0).abs() < 1e-3, "{a}");
    assert!(close(a, -b + Vec3::new(0.0, 0.0, 20.0), 1e-3), "{a} {b}");
}

#[test]
fn r_again_switches_back_to_the_shared_pivot() {
    let ctrl = pair();
    quarter_turn(&ctrl, &[1, 1, 1], Some(2));
    ctrl.on_modal_transform_finish(true);
    let a = translation(&ctrl, 0);
    assert!(a.x.abs() < 1e-3 && (a.y.abs() - 30.0).abs() < 1e-3, "{a}");
}

#[test]
fn esc_puts_each_object_back_after_r_r() {
    let ctrl = pair();
    quarter_turn(&ctrl, &[1, 1], None);
    assert!(
        (linear(&ctrl, 0)[0][0] - 1.0).abs() > 1e-3,
        "turned while live"
    );
    ctrl.on_modal_transform_finish(false);
    for (row, x) in [(0, -30.0), (1, 30.0)] {
        assert_eq!(translation(&ctrl, row), Vec3::new(x, 0.0, 10.0));
        assert_eq!(linear(&ctrl, row)[0][0], 1.0);
    }
    assert!(!ctrl.document().can_undo());
}

#[test]
fn s_s_scales_each_object_about_its_own_centre_and_keeps_it_on_the_plate() {
    let ctrl = pair();
    let centre = pixel(&ctrl, Vec3::new(0.0, 0.0, HALF as f32));
    let start = ScreenPoint::new(centre.x + 50.0, centre.y);
    ctrl.on_modal_transform_start(2, start.x, start.y);
    ctrl.on_modal_transform_start(2, start.x, start.y);
    ctrl.on_pointer_move(ScreenPoint::new(centre.x + 100.0, centre.y), none());
    ctrl.on_modal_transform_finish(true);
    for (row, x) in [(0, -30.0f32), (1, 30.0)] {
        let t = translation(&ctrl, row);
        // Twice the size about its own centre, lifted back onto the plate.
        assert!((linear(&ctrl, row)[0][0] - 2.0).abs() < 1e-3);
        assert!((t.x - x).abs() < 1e-3 && t.y.abs() < 1e-3, "{t}");
        assert!((t.z - 2.0 * HALF as f32).abs() < 1e-3, "on the plate: {t}");
    }
    assert!(ctrl.document().undo().unwrap());
    assert!(!ctrl.document().can_undo(), "one step");
}

#[test]
fn r_r_with_one_object_changes_nothing() {
    let ctrl = plate(&[(0.0, 0.0)], AxisView::Top);
    select(&ctrl, &[0]);
    quarter_turn(&ctrl, &[1, 1], Some(2));
    ctrl.on_modal_transform_finish(true);
    // One object turns about its own centre either way.
    assert!(close(
        translation(&ctrl, 0),
        Vec3::new(0.0, 0.0, 10.0),
        1e-4
    ));
}

// ---------------------------------------------------------------- #351 --

#[test]
fn r_without_an_axis_from_the_top_preset_turns_about_the_exact_vertical() {
    let ctrl = plate(&[(0.0, 0.0)], AxisView::Top);
    select(&ctrl, &[0]);
    // The preset itself looks a hair off vertical.
    let camera = camera(&ctrl);
    let forward = (camera.target - camera.eye()).normalize();
    assert!(forward.z > -1.0 && forward.dot(-Vec3::Z) > 0.99999);
    let start = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, HALF]));
    quarter_turn(&ctrl, &[1], None);
    ctrl.on_modal_transform_finish(true);
    // The model's up axis is still exactly +Z, and its supports follow.
    let l = linear(&ctrl, 0);
    assert_eq!([l[0][2], l[1][2], l[2][0], l[2][1]], [0.0; 4], "{l:?}");
    assert!((l[2][2] - 1.0).abs() < 1e-12);
    assert!(l[0][0].abs() < 1e-4, "a quarter turn: {l:?}");
    assert!(crate::support_follow::supports_follow(
        &start,
        &world(&ctrl, 0)
    ));
    // Turning about the raw view direction instead would have regenerated
    // them: that was the bug.
    let raw = AffineTransform3D::from_axis_angle(
        (-forward).to_array().map(f64::from),
        std::f64::consts::FRAC_PI_2,
    )
    .unwrap();
    let tilted = Transform3D::Affine(raw).compose(&start);
    assert!(!crate::support_follow::supports_follow(&start, &tilted));
}

#[test]
fn r_without_an_axis_from_a_slanted_view_keeps_turning_about_the_view() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    select(&ctrl, &[0]);
    quarter_turn(&ctrl, &[1], None);
    ctrl.on_modal_transform_finish(true);
    // A slanted view tilts the model: its up axis leaves +Z.
    let l = linear(&ctrl, 0);
    assert!((l[2][2] - 1.0).abs() > 1e-3, "{l:?}");
}

fn alt() -> PointerModifiers {
    PointerModifiers {
        alt: true,
        ..Default::default()
    }
}

/// How many objects the scene holds.
fn rows(ctrl: &ViewportController) -> usize {
    ctrl.document()
        .with_scene(|spatial| (0..).map_while(|i| spatial.leaf_id_at_index(i)).count())
        .unwrap_or(0)
}

/// Alt on a drag of a model moves a copy of it (#552): the model stays, the
/// copy is the selection, and the copy and its move are one step.
#[test]
fn an_alt_drag_moves_a_copy_and_leaves_the_model() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    let press = pixel(&ctrl, top_face(&ctrl, 0.0, 0.0));
    ctrl.on_pointer_down(press, alt());
    let to = ScreenPoint::new(press.x + 80.0, press.y);
    ctrl.on_pointer_move(to, alt());
    assert_eq!(
        rows(&ctrl),
        2,
        "the copy follows the pointer while dragging"
    );
    ctrl.on_pointer_up(to, alt());
    assert!(
        close(translation(&ctrl, 0), Vec3::new(0.0, 0.0, 10.0), 1e-9),
        "the model stays"
    );
    assert!(
        !close(translation(&ctrl, 1), Vec3::new(0.0, 0.0, 10.0), 1.0),
        "the copy moved"
    );
    assert_eq!(selected(&ctrl), vec![leaf(&ctrl, 1)]);
    let document = ctrl.document();
    assert!(document.undo().unwrap());
    assert!(!document.can_undo(), "one step");
    assert_eq!(rows(&ctrl), 1);
}

/// An Alt drag cancelled leaves no copy, nothing to undo or redo (#552).
#[test]
fn an_alt_drag_cancelled_leaves_no_copy() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    let press = pixel(&ctrl, top_face(&ctrl, 0.0, 0.0));
    ctrl.on_pointer_down(press, alt());
    ctrl.on_pointer_move(ScreenPoint::new(press.x + 80.0, press.y), alt());
    ctrl.on_clear_selection();
    ctrl.on_pointer_up(ScreenPoint::new(press.x + 80.0, press.y), alt());
    assert_eq!(rows(&ctrl), 1);
    assert!(!ctrl.document().can_undo());
    assert!(!ctrl.document().can_redo());
}

/// Alt on the Move gizmo moves a copy of the selected model (#552).
#[test]
fn alt_on_the_move_gizmo_moves_a_copy() {
    let ctrl = slanted(&[(0.0, 0.0)]);
    select(&ctrl, &[0]);
    let pivot = Vec3::new(0.0, 0.0, 10.0);
    let scale =
        crate::gizmo::gizmo_scale(&camera(&ctrl), pivot, default_fov_y(), H).expect("in view");
    let press = pixel(&ctrl, pivot + Vec3::X * scale * 1.1);
    ctrl.on_pointer_down(press, alt());
    assert!(matches!(
        ctrl.gizmo_state_for_test(),
        GizmoState::Drag {
            op: DragOp::Translate { .. },
            ..
        }
    ));
    let to = ScreenPoint::new(press.x + 60.0, press.y);
    ctrl.on_pointer_move(to, alt());
    ctrl.on_pointer_up(to, alt());
    assert_eq!(rows(&ctrl), 2);
    assert!(close(translation(&ctrl, 0), pivot, 1e-9), "the model stays");
    assert!(translation(&ctrl, 1).x > 1.0, "the copy moved along X");
    assert!(ctrl.document().undo().unwrap());
    assert!(!ctrl.document().can_undo(), "one step");
}
