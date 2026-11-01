/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-frame composition of the renderer-facing [`GizmoFrame`].
//!
//! Reads the current gizmo FSM state from [`InputDispatch`], translates
//! it into the renderer's `GizmoVisibility` / `GizmoDragVisual` enums,
//! and (when a leaf is selected) calls
//! [`crate::gizmo::frame::build_gizmo_frame`] to produce the
//! [`GizmoFrame`]. Lives in its own module so the viewport's `on_paint`
//! body does not have to mix input-state reading, FSM-to-render-enum
//! translation, and spatial gizmo-frame assembly inline.

use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use super::gizmo_drag::{DragOp, GizmoState};
use super::gizmo_fsm::gizmo_pivot;
use super::input_dispatch::InputDispatch;
use super::modal_transform::{ModalGesture, ModalOp};
use crate::camera::OrbitCamera;
use crate::gizmo::axis::ALL_AXES;
use crate::gizmo::dimensions::MIDPOINT_FRACTION;
use crate::gizmo::frame::{build_gizmo_frame, build_gizmo_frame_at};
use crate::mesh_render::{GizmoDragVisual, GizmoFrame, GizmoVisibility, default_fov_y};

pub(super) struct GizmoFrameComposer;

impl GizmoFrameComposer {
    /// Reads the current drag / hover state from `input`'s gizmo FSM, or the
    /// modal G/R/S gesture in flight, translates it into the renderer's
    /// per-frame enums, and (when `selection` names a reachable leaf) builds
    /// the [`GizmoFrame`] the renderer's gizmo pass consumes. A modal gesture
    /// shows the gizmo as the matching drag does. Returns `None` when no leaf
    /// is selected or the selection has no pickable geometry. With several
    /// leaves in `group` (#13), the gizmo stands at their shared centre.
    pub(super) fn compose(
        input: &InputDispatch,
        spatial: &SceneSpatial,
        camera: &OrbitCamera,
        selection: Option<NodeId>,
        group: &[NodeId],
        viewport_h_logical: f32,
    ) -> Option<GizmoFrame> {
        let sel = selection?;
        let pivot = gizmo_pivot(spatial, sel, group)?;
        let (visibility, drag_visual) = match input.modal.get() {
            Some(gesture) => {
                let shaft =
                    crate::gizmo::gizmo_scale(camera, pivot, default_fov_y(), viewport_h_logical);
                modal_look(&gesture, shaft)
            }
            None => drag_look(input.gizmo_fsm().current()),
        };
        let hover = input.gizmo_fsm().current_hover();
        if group.len() > 1 {
            return build_gizmo_frame_at(
                pivot,
                camera,
                viewport_h_logical,
                visibility,
                hover,
                drag_visual,
            );
        }
        build_gizmo_frame(
            spatial,
            camera,
            sel,
            viewport_h_logical,
            visibility,
            hover,
            drag_visual,
        )
    }
}

/// The gizmo of a drag in flight: the dragged handle, and its live visual.
fn drag_look(state: GizmoState) -> (GizmoVisibility, Option<GizmoDragVisual>) {
    match state {
        GizmoState::Drag { op, pivot, .. } => match op {
            DragOp::Translate { axis, .. } => (GizmoVisibility::DraggingTranslate { axis }, None),
            DragOp::Rotate {
                axis,
                start_angle,
                applied,
                ..
            } => (
                GizmoVisibility::DraggingRotate { axis },
                Some(GizmoDragVisual::Rotate {
                    axis,
                    start: start_angle,
                    sweep: applied,
                }),
            ),
            DragOp::ScaleAxis {
                axis,
                current_anchor,
                ..
            } => {
                let axial_distance = (current_anchor - pivot).dot(axis.world_dir());
                (
                    GizmoVisibility::DraggingScaleAxis { axis },
                    Some(GizmoDragVisual::ScaleAxis {
                        axis,
                        axial_distance,
                    }),
                )
            }
            DragOp::ScaleUniform { .. } => (GizmoVisibility::DraggingScaleUniform, None),
        },
        GizmoState::Idle => (GizmoVisibility::Idle, None),
    }
}

/// The gizmo of a modal G/R/S, as the matching drag shows it: G and S light
/// the arrow or cube of the locked axis (the cube slides with the scale), R
/// shows its ring whole with the slice of the turn. Without an axis, G lights
/// every arrow, S the centre ball, and R a screen ring in the camera plane.
/// `shaft` is the world length of the gizmo shaft at the pivot.
pub(super) fn modal_look(
    gesture: &ModalGesture,
    shaft: Option<f32>,
) -> (GizmoVisibility, Option<GizmoDragVisual>) {
    let axis = gesture.axis.map(|k| ALL_AXES[k]);
    match (gesture.op, axis) {
        (ModalOp::Move, Some(axis)) => (GizmoVisibility::DraggingTranslate { axis }, None),
        (ModalOp::Move, None) => (GizmoVisibility::MovingFree, None),
        (ModalOp::Scale, Some(axis)) => (
            GizmoVisibility::DraggingScaleAxis { axis },
            shaft.map(|s| GizmoDragVisual::ScaleAxis {
                axis,
                axial_distance: s * MIDPOINT_FRACTION * gesture.factor,
            }),
        ),
        (ModalOp::Scale, None) => (GizmoVisibility::DraggingScaleUniform, None),
        (ModalOp::Rotate, Some(axis)) => (
            GizmoVisibility::DraggingRotate { axis },
            Some(GizmoDragVisual::Rotate {
                axis,
                start: gesture.ring_start(),
                sweep: gesture.applied_turn(),
            }),
        ),
        (ModalOp::Rotate, None) => (
            GizmoVisibility::DraggingViewRotate,
            Some(GizmoDragVisual::ViewRotate {
                right: gesture.right,
                up: gesture.up,
                start: gesture.ring_start(),
                sweep: gesture.applied_turn(),
            }),
        ),
    }
}

#[cfg(test)]
mod modal_look_tests {
    use std::sync::Arc;

    use float_cmp::assert_approx_eq;
    use glam::Vec3;
    use sc_mesh_core::transform::Transform3D;
    use sc_scene::{LeafPayload, Scene};

    use super::*;
    use crate::gizmo::axis::Axis;
    use crate::gizmo::modifiers::{DragModifiers, PrecisionCursor};

    /// A modal gesture of `op` seen from above: right = +X, up = +Y, the eye
    /// along +Z; the pivot at the origin shows at (100, 100), the press 100
    /// pixels right of it.
    fn gesture(op: ModalOp, axis: Option<usize>) -> ModalGesture {
        let mut scene = Scene::new();
        let node_id = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(sc_mesh_core::IndexedMesh {
                    meta: None,
                    vertices: vec![],
                    faces: vec![],
                })),
            )
            .unwrap();
        ModalGesture {
            node_id,
            original: Transform3D::Identity,
            parent: Transform3D::Identity,
            pivot: Vec3::ZERO,
            start: [200.0, 100.0],
            cursor: [200.0, 100.0],
            screen_pivot: [100.0, 100.0],
            right: Vec3::X,
            up: Vec3::Y,
            view_axis: Vec3::Z,
            world_per_pixel: 1.0,
            op,
            axis,
            last_angle: 0.0,
            angle: 0.0,
            precision: PrecisionCursor::new([200.0, 100.0], false),
            modifiers: DragModifiers::default(),
            factor: 1.0,
        }
    }

    #[test]
    fn modal_g_and_s_light_the_handle_a_drag_would() {
        let (v, d) = modal_look(&gesture(ModalOp::Move, Some(0)), Some(1.0));
        assert_eq!(v, GizmoVisibility::DraggingTranslate { axis: Axis::X });
        assert!(d.is_none());
        let (v, _) = modal_look(&gesture(ModalOp::Move, None), Some(1.0));
        assert_eq!(v, GizmoVisibility::MovingFree);
        let (v, _) = modal_look(&gesture(ModalOp::Scale, None), Some(1.0));
        assert_eq!(v, GizmoVisibility::DraggingScaleUniform);

        // S X: the cube slides out with the scale, as in a cube drag.
        let mut g = gesture(ModalOp::Scale, Some(0));
        g.propose([300.0, 100.0], DragModifiers::default()).unwrap();
        let (v, d) = modal_look(&g, Some(2.0));
        assert_eq!(v, GizmoVisibility::DraggingScaleAxis { axis: Axis::X });
        match d {
            Some(GizmoDragVisual::ScaleAxis {
                axis,
                axial_distance,
            }) => {
                assert_eq!(axis, Axis::X);
                assert_approx_eq!(f32, axial_distance, 2.0 * MIDPOINT_FRACTION * 2.0);
            }
            other => panic!("expected the cube slide, got {other:?}"),
        }
    }

    #[test]
    fn modal_r_with_an_axis_shows_its_ring_and_the_slice_of_the_turn() {
        let mut g = gesture(ModalOp::Rotate, Some(2));
        // A quarter turn counter-clockwise on screen around the pivot.
        g.propose([100.0, 0.0], DragModifiers::default()).unwrap();
        let (v, d) = modal_look(&g, Some(1.0));
        assert_eq!(v, GizmoVisibility::DraggingRotate { axis: Axis::Z });
        match d {
            Some(GizmoDragVisual::Rotate { axis, start, sweep }) => {
                assert_eq!(axis, Axis::Z);
                // Pressed right of the pivot: +X, angle 0 in the Z basis.
                assert_approx_eq!(f32, start, 0.0, epsilon = 1e-6);
                assert_approx_eq!(f32, sweep, std::f32::consts::FRAC_PI_2, epsilon = 1e-5);
                assert_approx_eq!(f32, sweep, g.applied_turn());
            }
            other => panic!("expected the turn marks, got {other:?}"),
        }
    }

    #[test]
    fn modal_r_without_an_axis_shows_the_screen_ring_and_its_slice_follows_the_cursor() {
        let mut g = gesture(ModalOp::Rotate, None);
        // The cursor goes from right of the pivot to above it.
        g.propose([100.0, 0.0], DragModifiers::default()).unwrap();
        let (v, d) = modal_look(&g, Some(1.0));
        assert_eq!(v, GizmoVisibility::DraggingViewRotate);
        match d {
            Some(GizmoDragVisual::ViewRotate {
                right,
                up,
                start,
                sweep,
            }) => {
                assert_eq!((right, up), (Vec3::X, Vec3::Y));
                // The end of the slice points where the cursor is: up.
                let end = start + sweep;
                let dir = right * end.cos() + up * end.sin();
                assert!((dir - Vec3::Y).length() < 1e-5, "{dir}");
            }
            other => panic!("expected the screen ring, got {other:?}"),
        }
    }

    #[test]
    fn a_press_above_the_pivot_starts_the_ring_a_quarter_turn_on() {
        let mut g = gesture(ModalOp::Rotate, Some(2));
        g.start = [100.0, 0.0];
        assert_approx_eq!(
            f32,
            g.ring_start(),
            std::f32::consts::FRAC_PI_2,
            epsilon = 1e-6
        );
    }
}
