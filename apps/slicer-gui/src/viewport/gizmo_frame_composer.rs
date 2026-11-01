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
use super::group_move::group_pivot;
use super::input_dispatch::InputDispatch;
use crate::camera::OrbitCamera;
use crate::gizmo::frame::{build_gizmo_frame, build_gizmo_frame_at};
use crate::mesh_render::{GizmoDragVisual, GizmoFrame, GizmoVisibility};

pub(super) struct GizmoFrameComposer;

impl GizmoFrameComposer {
    /// Reads the current drag / hover state from `input`'s gizmo FSM,
    /// translates it into the renderer's per-frame enums, and (when
    /// `selection` names a reachable leaf) builds the
    /// [`GizmoFrame`] the renderer's gizmo pass consumes. Returns
    /// `None` when no leaf is selected or the selection has no
    /// pickable geometry. With several leaves in `group` (#13), the
    /// gizmo stands at their shared centre.
    pub(super) fn compose(
        input: &InputDispatch,
        spatial: &SceneSpatial,
        camera: &OrbitCamera,
        selection: Option<NodeId>,
        group: &[NodeId],
        viewport_h_logical: f32,
    ) -> Option<GizmoFrame> {
        let drag_state = input.gizmo_fsm().current();
        let (visibility, drag_visual) = match drag_state {
            GizmoState::Drag { op, pivot, .. } => match op {
                DragOp::Translate { axis, .. } => {
                    (GizmoVisibility::DraggingTranslate { axis }, None)
                }
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
        };
        let hover = input.gizmo_fsm().current_hover();
        let sel = selection?;
        if group.len() > 1 {
            return build_gizmo_frame_at(
                group_pivot(spatial, group)?,
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
