/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-frame `GizmoFrame` assembly for the renderer. Bridges the
//! controller's per-frame state (pivot, scale, visibility,
//! lock-out mask) into the renderer's struct.

use sc_mesh_core::scene::Scene;

use crate::camera::OrbitCamera;
use crate::mesh_render::{GizmoFrame, GizmoVisibility, default_fov_y};
use crate::picking;

use super::axis::ALL_AXES;
use super::hit_test::{arc_pickable, axis_pickable};
use super::{gizmo_scale, pivot_for_object};

/// Builds the per-frame `GizmoFrame` for the renderer when a single
/// object is selected. Returns `None` if the object has no geometry
/// (no pivot to attach to). The locked-axes mask uses the camera's
/// forward direction as a single representative ray — for a small
/// gizmo near the screen centre, all rays through the gizmo region
/// are within a degree or so of the camera forward, so a per-frame
/// "is this axis grabbable from this view" check is uniform across
/// the handle.
pub fn build_gizmo_frame(
    scene: &Scene,
    camera: &OrbitCamera,
    selection: usize,
    viewport_h_logical: f32,
    visibility: GizmoVisibility,
) -> Option<GizmoFrame> {
    let pivot = pivot_for_object(scene, selection)?;
    let fov_y = default_fov_y();
    let scale = gizmo_scale(camera, fov_y, viewport_h_logical);
    let forward = (camera.target - camera.eye()).normalize_or_zero();
    let view_ray = picking::Ray {
        origin: camera.eye(),
        direction: forward,
    };
    let locked_translate = ALL_AXES.map(|a| !axis_pickable(&view_ray, a));
    let locked_rotate = ALL_AXES.map(|a| !arc_pickable(&view_ray, a));
    Some(GizmoFrame {
        pivot,
        scale,
        visibility,
        locked_translate,
        locked_rotate,
    })
}
