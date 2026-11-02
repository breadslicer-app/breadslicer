/* SPDX-FileCopyrightText: 2026 SynrgStudio */
/* SPDX-License-Identifier: AGPL-3.0-only */

//! The Supports tab's drags (#3): a press of a drag tool is a click until it
//! moves past the threshold, then the support driver gets the drag; the
//! release ends it, Esc or a right press drops it, and a right press that
//! dropped one opens no menu.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::{LeafPayload, Scene};

use super::controller::ViewportController;
use super::support_drag::{DragPoint, Project, SupportDragHandler, SupportDragKind};
use super::types::ScreenPoint;
use crate::camera::{AxisView, OrbitCamera};
use crate::{PointerModifiers, WorkspaceSection};

/// What the support driver was told.
#[derive(Debug, Clone, PartialEq)]
enum Heard {
    Start(SupportDragKind, [f32; 2]),
    Move([f32; 2], bool),
    End(bool),
}

#[derive(Default)]
struct Recorder {
    heard: RefCell<Vec<Heard>>,
    takes: bool,
}

impl SupportDragHandler for Recorder {
    fn drag_start(&self, kind: SupportDragKind, press: &DragPoint, project: Project<'_>) -> bool {
        self.heard
            .borrow_mut()
            .push(Heard::Start(kind, press.cursor));
        // The camera looks down at the cube: its top lands on screen and is
        // in sight; a point well under it is behind it.
        assert!(project.project([0.0, 0.0, 0.5]).is_some());
        assert!(project.visible([0.0, 0.0, 0.5]));
        assert!(!project.visible([0.0, 0.0, -3.0]));
        self.takes
    }

    fn drag_move(&self, at: &DragPoint, _: Project<'_>) {
        self.heard
            .borrow_mut()
            .push(Heard::Move(at.cursor, at.model.is_some()));
    }

    fn drag_end(&self, commit: bool) {
        self.heard.borrow_mut().push(Heard::End(commit));
    }
}

/// A controller in the Supports tab over a unit cube, the camera looking
/// straight down at it, `kind` armed, and its driver.
fn armed(kind: SupportDragKind, takes: bool) -> (ViewportController, Rc<Recorder>) {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([0.0; 3])),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("a fresh root");
    let ctrl = ViewportController::new_for_test();
    ctrl.install_graph_for_test(scene);
    ctrl.set_viewport_for_test(200.0, 100.0);
    ctrl.set_section(WorkspaceSection::Supports);
    let mut camera = OrbitCamera::new(Vec3::ZERO, 4.0);
    camera.set_axis_view(AxisView::Top);
    ctrl.set_camera_for_test(camera);
    ctrl.arm_drag_tool_for_test(Some(kind));
    let recorder = Rc::new(Recorder {
        takes,
        ..Recorder::default()
    });
    let handler: Rc<dyn SupportDragHandler> = recorder.clone();
    ctrl.set_support_drag_handler(Rc::downgrade(&handler));
    (ctrl, recorder)
}

fn none() -> PointerModifiers {
    PointerModifiers::default()
}

fn at(x: f32, y: f32) -> ScreenPoint {
    ScreenPoint::new(x, y)
}

#[test]
fn a_drag_starts_past_the_threshold_and_ends_on_release() {
    let (ctrl, heard) = armed(SupportDragKind::Box, true);
    ctrl.on_pointer_down(at(100.0, 50.0), none());
    ctrl.on_pointer_move(at(102.0, 51.0), none());
    assert!(heard.heard.borrow().is_empty(), "still a click");
    ctrl.on_pointer_move(at(110.0, 50.0), none());
    ctrl.on_pointer_move(at(100.0, 50.0), none());
    ctrl.on_pointer_move(at(120.0, 60.0), none());
    ctrl.on_pointer_up(at(120.0, 60.0), none());
    assert_eq!(
        *heard.heard.borrow(),
        [
            Heard::Start(SupportDragKind::Box, [100.0, 50.0]),
            Heard::Move([110.0, 50.0], false),
            Heard::Move([100.0, 50.0], true),
            Heard::Move([120.0, 60.0], false),
            Heard::End(true),
        ]
    );
}

#[test]
fn esc_or_a_right_press_drops_the_drag_and_the_release_does_nothing() {
    let (ctrl, heard) = armed(SupportDragKind::MoveTip, true);
    ctrl.on_pointer_down(at(100.0, 50.0), none());
    ctrl.on_pointer_move(at(110.0, 50.0), none());
    assert!(ctrl.cancel_support_drag(), "Esc drops it");
    ctrl.on_pointer_move(at(120.0, 50.0), none());
    ctrl.on_pointer_up(at(120.0, 50.0), none());
    assert_eq!(heard.heard.borrow().last(), Some(&Heard::End(false)));
    assert_eq!(heard.heard.borrow().len(), 3, "{:?}", heard.heard.borrow());
    assert!(!ctrl.cancel_support_drag(), "nothing left to drop");

    let (ctrl, heard) = armed(SupportDragKind::Brush, true);
    ctrl.on_pointer_down(at(100.0, 50.0), none());
    ctrl.on_pointer_move(at(110.0, 50.0), none());
    ctrl.on_right_pointer_down(110.0, 50.0);
    assert_eq!(ctrl.on_right_pointer_up(110.0, 50.0), None, "no menu");
    ctrl.on_pointer_up(at(110.0, 50.0), none());
    assert_eq!(heard.heard.borrow().last(), Some(&Heard::End(false)));
    // The next right click is a right click again.
    ctrl.on_right_pointer_down(110.0, 50.0);
    assert!(ctrl.on_right_pointer_up(110.0, 50.0).is_some());
}

#[test]
fn a_drag_the_driver_refuses_does_nothing() {
    let (ctrl, heard) = armed(SupportDragKind::Box, false);
    ctrl.on_pointer_down(at(100.0, 50.0), none());
    ctrl.on_pointer_move(at(110.0, 50.0), none());
    ctrl.on_pointer_move(at(130.0, 50.0), none());
    ctrl.on_pointer_up(at(130.0, 50.0), none());
    assert_eq!(heard.heard.borrow().len(), 1, "only asked");
    // With Shift, Remove keeps its selection box: no drag of its own.
    let (ctrl, heard) = armed(SupportDragKind::Box, true);
    let shift = || PointerModifiers {
        shift: true,
        ..PointerModifiers::default()
    };
    ctrl.on_pointer_down(at(100.0, 50.0), shift());
    ctrl.on_pointer_move(at(130.0, 50.0), shift());
    ctrl.on_pointer_up(at(130.0, 50.0), shift());
    assert!(heard.heard.borrow().is_empty());
}
