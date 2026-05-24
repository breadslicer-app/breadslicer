/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-viewport orbit-camera concern. Owns the [`OrbitCamera`] cell,
//! the framing-init math, and the five pointer-driven camera
//! callbacks. Lives next to the scene rather than inside the renderer
//! so the user's view survives `RenderingTeardown` -> `RenderingSetup`
//! cycles and so the controller can mutate the camera before a GPU
//! device is available.
//!
//! Callbacks return `bool` matching the project's established
//! pattern (see `gizmo_fsm::on_move` / `keyboard_drive::integrate`):
//! `true` means the caller (the coordinator) should mark the frame
//! dirty.

use std::cell::RefCell;

use glam::Vec3;

use super::types::{ScreenDelta, ScreenPoint, ViewportSize};
use crate::build_volume::BuildVolume;
use crate::camera::{AxisView, OrbitCamera};
use crate::mesh_render::default_fov_y;

/// `exp(-delta * RATE)` converts the raw scroll delta (in scroll
/// units) to a multiplicative distance factor. The exponential keeps
/// successive scroll notches symmetric: N notches in, then N notches
/// out, returns to the same distance.
const ZOOM_RATE_PER_DELTA: f32 = 0.005;

/// Per-event clamp on the zoom factor. Caps pathological scroll
/// deltas (very large absolute values that `exp` would otherwise
/// round to 0 or infinity) to `[1.0 / MAX_STEP, MAX_STEP]`.
const ZOOM_FACTOR_MAX_STEP: f32 = 10.0;

pub(super) struct CameraController {
    camera: RefCell<Option<OrbitCamera>>,
}

impl CameraController {
    pub(super) fn new() -> Self {
        Self {
            camera: RefCell::new(None),
        }
    }

    /// Borrow access for sub-modules and coordinator paths that need
    /// to read or write the live camera (picking, rendering, gizmo
    /// hit-tests). The other concerns rely on the existing
    /// `&RefCell<Option<OrbitCamera>>` shape in their `Ctx` structs,
    /// so this accessor is the integration point until those Ctx
    /// shapes are also updated.
    pub(super) fn camera_cell(&self) -> &RefCell<Option<OrbitCamera>> {
        &self.camera
    }

    /// True once `replace_with_framing` has populated the camera.
    pub(super) fn is_initialised(&self) -> bool {
        self.camera.borrow().is_some()
    }

    /// Drops the camera. The next framing pass populates a fresh
    /// view; called on scene install (so a freshly-loaded scene gets
    /// re-framed) and on scene clear.
    pub(super) fn reset(&self) {
        *self.camera.borrow_mut() = None;
    }

    /// Frames the orbit camera on the union of the scene's world AABB
    /// (when known) and the build envelope, so the plate is always
    /// inside the view. Detail radius is anchored to the scene's own
    /// AABB so the user can pull close to a model much smaller than
    /// the envelope; falls back to `view_radius` when there's no
    /// scene bounds yet.
    ///
    /// Unconditional write: callers gate on `is_initialised` (or
    /// arrange for `reset` to fire first via the scene-installed
    /// listener).
    pub(super) fn replace_with_framing(
        &self,
        scene_bounds: Option<(Vec3, Vec3)>,
        build_volume: BuildVolume,
    ) {
        let (centre, view_radius) = build_volume.framing(scene_bounds);
        let detail_radius = scene_only_radius(scene_bounds).unwrap_or(view_radius);
        *self.camera.borrow_mut() = Some(OrbitCamera::with_detail_radius(
            centre,
            view_radius,
            detail_radius,
        ));
    }

    /// Orbits the camera around its target. Returns `true` if a
    /// camera and viewport were present (and the caller should mark
    /// the frame dirty).
    pub(super) fn on_rotate(&self, delta: ScreenDelta, viewport: Option<ViewportSize>) -> bool {
        let Some(vp) = viewport else {
            return false;
        };
        let mut slot = self.camera.borrow_mut();
        let Some(cam) = slot.as_mut() else {
            return false;
        };
        cam.orbit(delta.dx, delta.dy, vp.height());
        true
    }

    /// Pans the camera in the view plane. Returns `true` if a camera
    /// and viewport were present.
    pub(super) fn on_pan(&self, delta: ScreenDelta, viewport: Option<ViewportSize>) -> bool {
        let Some(vp) = viewport else {
            return false;
        };
        let mut slot = self.camera.borrow_mut();
        let Some(cam) = slot.as_mut() else {
            return false;
        };
        cam.pan(delta.dx, delta.dy, default_fov_y(), vp.height());
        true
    }

    /// Zooms the camera at the cursor position. The raw scroll
    /// `wheel_delta` is converted to a multiplicative factor via
    /// `exp(-wheel_delta * ZOOM_RATE_PER_DELTA)` and then clamped to
    /// `[1.0 / ZOOM_FACTOR_MAX_STEP, ZOOM_FACTOR_MAX_STEP]` so a
    /// pathological delta cannot drive `factor` to 0 or infinity.
    /// `OrbitCamera::zoom_at` clamps again internally with the same
    /// bounds, this outer clamp keeps `factor` finite before any
    /// further arithmetic.
    pub(super) fn on_zoom(
        &self,
        wheel_delta: f32,
        cursor: ScreenPoint,
        viewport: Option<ViewportSize>,
    ) -> bool {
        let Some(vp) = viewport else {
            return false;
        };
        let factor = (-wheel_delta * ZOOM_RATE_PER_DELTA)
            .exp()
            .clamp(1.0 / ZOOM_FACTOR_MAX_STEP, ZOOM_FACTOR_MAX_STEP);
        let mut slot = self.camera.borrow_mut();
        let Some(cam) = slot.as_mut() else {
            return false;
        };
        cam.zoom_at(
            factor,
            cursor.x,
            cursor.y,
            default_fov_y(),
            vp.width(),
            vp.height(),
        );
        true
    }

    /// Resets the view to the default framing. Returns `true` if a
    /// camera was present.
    pub(super) fn on_frame_view(&self) -> bool {
        let mut slot = self.camera.borrow_mut();
        let Some(cam) = slot.as_mut() else {
            return false;
        };
        cam.frame_scene();
        true
    }

    /// Snaps the view to one of the six axis-aligned orientations.
    /// Returns `true` if a camera was present.
    pub(super) fn on_axis_view(&self, view: AxisView) -> bool {
        let mut slot = self.camera.borrow_mut();
        let Some(cam) = slot.as_mut() else {
            return false;
        };
        cam.set_axis_view(view);
        true
    }
}

/// Half-length of the scene's AABB diagonal in millimetres, used as
/// the `detail_radius` argument to `OrbitCamera::with_detail_radius`.
/// Returns `None` when no scene bounds are known, so callers can fall
/// back to the envelope-derived `view_radius`.
fn scene_only_radius(scene_bounds: Option<(Vec3, Vec3)>) -> Option<f32> {
    scene_bounds.map(|(min, max)| ((max - min).length() * 0.5).max(1e-3))
}

#[cfg(test)]
mod camera_controller_tests {
    use super::*;

    fn default_volume() -> BuildVolume {
        crate::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME
    }

    fn fhd_viewport() -> Option<ViewportSize> {
        ViewportSize::new(1920.0, 1080.0)
    }

    fn frame(cc: &CameraController) {
        cc.replace_with_framing(None, default_volume());
    }

    #[test]
    fn new_is_uninitialised() {
        let cc = CameraController::new();
        assert!(!cc.is_initialised());
    }

    #[test]
    fn replace_with_framing_initialises() {
        let cc = CameraController::new();
        cc.replace_with_framing(None, default_volume());
        assert!(cc.is_initialised());
    }

    #[test]
    fn reset_drops_the_camera() {
        let cc = CameraController::new();
        frame(&cc);
        cc.reset();
        assert!(!cc.is_initialised());
    }

    #[test]
    fn framing_with_no_scene_anchors_on_envelope() {
        let cc = CameraController::new();
        cc.replace_with_framing(None, default_volume());
        let centre = cc.camera_cell().borrow().as_ref().unwrap().target;
        // Plate is centred on world XY, so the framing centre's X / Y
        // are at the origin; Z anchors on half the envelope height.
        assert_eq!(centre.x, 0.0);
        assert_eq!(centre.y, 0.0);
        assert!(centre.z > 0.0);
    }

    #[test]
    fn framing_with_scene_outside_envelope_extends() {
        let cc = CameraController::new();
        let far = Vec3::new(1000.0, 1000.0, 1000.0);
        cc.replace_with_framing(Some((Vec3::ZERO, far)), default_volume());
        // The view target sits at the union centre; the union grows
        // in the +x/+y/+z direction, so the target shifts away from
        // the envelope-only origin.
        let centre = cc.camera_cell().borrow().as_ref().unwrap().target;
        assert!(centre.x > 0.0);
        assert!(centre.y > 0.0);
        assert!(centre.z > 0.0);
    }

    #[test]
    fn on_rotate_without_viewport_returns_false() {
        let cc = CameraController::new();
        frame(&cc);
        assert!(!cc.on_rotate(ScreenDelta::new(10.0, 5.0), None));
    }

    #[test]
    fn on_rotate_without_camera_returns_false() {
        let cc = CameraController::new();
        assert!(!cc.on_rotate(ScreenDelta::new(10.0, 5.0), fhd_viewport()));
    }

    #[test]
    fn on_rotate_with_camera_and_viewport_returns_true_and_mutates_yaw() {
        let cc = CameraController::new();
        frame(&cc);
        let yaw_before = cc.camera_cell().borrow().as_ref().unwrap().yaw;
        assert!(cc.on_rotate(ScreenDelta::new(50.0, 0.0), fhd_viewport()));
        let yaw_after = cc.camera_cell().borrow().as_ref().unwrap().yaw;
        assert!((yaw_after - yaw_before).abs() > 0.0);
    }

    #[test]
    fn on_pan_without_viewport_returns_false() {
        let cc = CameraController::new();
        frame(&cc);
        assert!(!cc.on_pan(ScreenDelta::new(10.0, 5.0), None));
    }

    #[test]
    fn on_pan_without_camera_returns_false() {
        let cc = CameraController::new();
        assert!(!cc.on_pan(ScreenDelta::new(10.0, 5.0), fhd_viewport()));
    }

    #[test]
    fn on_pan_with_camera_and_viewport_returns_true_and_mutates_target() {
        let cc = CameraController::new();
        frame(&cc);
        let target_before = cc.camera_cell().borrow().as_ref().unwrap().target;
        assert!(cc.on_pan(ScreenDelta::new(10.0, 5.0), fhd_viewport()));
        let target_after = cc.camera_cell().borrow().as_ref().unwrap().target;
        assert_ne!(target_before, target_after);
    }

    #[test]
    fn on_zoom_without_viewport_returns_false() {
        let cc = CameraController::new();
        frame(&cc);
        assert!(!cc.on_zoom(120.0, ScreenPoint::new(100.0, 100.0), None));
    }

    #[test]
    fn on_zoom_without_camera_returns_false() {
        let cc = CameraController::new();
        assert!(!cc.on_zoom(120.0, ScreenPoint::new(100.0, 100.0), fhd_viewport()));
    }

    #[test]
    fn on_zoom_with_camera_and_viewport_returns_true_and_mutates_distance() {
        let cc = CameraController::new();
        frame(&cc);
        let dist_before = cc.camera_cell().borrow().as_ref().unwrap().distance;
        // Positive wheel_delta is a wheel-down step; the controller maps
        // it through `exp(-wheel_delta * RATE)` to a factor < 1, which
        // dollies in.
        assert!(cc.on_zoom(120.0, ScreenPoint::new(960.0, 540.0), fhd_viewport()));
        let dist_after = cc.camera_cell().borrow().as_ref().unwrap().distance;
        assert!(dist_after < dist_before);
    }

    #[test]
    fn on_zoom_clamps_pathological_positive_delta_to_finite_step() {
        // A huge positive wheel_delta would let `exp(-wheel_delta * RATE)`
        // round to 0 without the outer clamp; the camera's distance
        // would then collapse to its lower bound in one event. The
        // clamp caps the per-event ratio at `1 / ZOOM_FACTOR_MAX_STEP`.
        let cc = CameraController::new();
        frame(&cc);
        let dist_before = cc.camera_cell().borrow().as_ref().unwrap().distance;
        assert!(cc.on_zoom(1.0e6, ScreenPoint::new(960.0, 540.0), fhd_viewport()));
        let dist_after = cc.camera_cell().borrow().as_ref().unwrap().distance;
        // Per-event factor floors at 1/10; the camera applies its own
        // floor below that, but the final distance must be finite and
        // strictly positive.
        assert!(dist_after.is_finite());
        assert!(dist_after > 0.0);
        assert!(dist_after <= dist_before);
    }

    #[test]
    fn on_zoom_clamps_pathological_negative_delta_to_finite_step() {
        let cc = CameraController::new();
        frame(&cc);
        let dist_before = cc.camera_cell().borrow().as_ref().unwrap().distance;
        assert!(cc.on_zoom(-1.0e6, ScreenPoint::new(960.0, 540.0), fhd_viewport()));
        let dist_after = cc.camera_cell().borrow().as_ref().unwrap().distance;
        assert!(dist_after.is_finite());
        assert!(dist_after >= dist_before);
    }

    #[test]
    fn on_frame_view_without_camera_returns_false() {
        let cc = CameraController::new();
        assert!(!cc.on_frame_view());
    }

    #[test]
    fn on_frame_view_resets_to_defaults() {
        let cc = CameraController::new();
        frame(&cc);
        cc.on_rotate(ScreenDelta::new(120.0, 60.0), fhd_viewport());
        let before = cc.camera_cell().borrow().as_ref().unwrap().clone();
        assert!(cc.on_frame_view());
        let after = cc.camera_cell().borrow().as_ref().unwrap().clone();
        assert_ne!(before.yaw, after.yaw);
    }

    #[test]
    fn on_axis_view_without_camera_returns_false() {
        let cc = CameraController::new();
        assert!(!cc.on_axis_view(AxisView::Top));
    }

    #[test]
    fn on_axis_view_with_camera_snaps_orientation() {
        let cc = CameraController::new();
        frame(&cc);
        assert!(cc.on_axis_view(AxisView::Top));
        let cam = cc.camera_cell().borrow().as_ref().unwrap().clone();
        // Top view points along -Z with the pitch latched at +pi/2
        // (modulo the camera's pitch clamp).
        assert!(cam.pitch > 0.0);
    }
}
