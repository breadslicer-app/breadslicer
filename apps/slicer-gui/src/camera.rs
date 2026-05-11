/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use glam::{Mat4, Vec3};

const PITCH_EPSILON: f32 = 1e-3;
const MIN_DISTANCE_FACTOR: f32 = 0.05;
const MAX_DISTANCE_FACTOR: f32 = 50.0;
/// Floor on the zoom-in distance expressed as a fraction of
/// `view_radius`. Keeps the camera ahead of the renderer's near plane
/// (`frame::NEAR_MULT = 0.01` on a framing diagonal of at most
/// `2 * view_radius` gives `0.02 * view_radius`; this leaves headroom
/// for unions that exceed the envelope on one axis).
const NEAR_PLANE_SAFETY_FACTOR: f32 = 0.03;
const DEFAULT_YAW: f32 = 0.6749;
const DEFAULT_PITCH: f32 = 0.6669;
pub(crate) const DEFAULT_DISTANCE_FACTOR: f32 = 3.0;

#[derive(Copy, Clone, Debug)]
pub enum AxisView {
    Front,
    Back,
    Right,
    Left,
    Top,
    Bottom,
}

#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub up: Vec3,

    scene_center: Vec3,
    /// Drives the initial framing distance and the zoom-OUT limit.
    /// Anchored to the build envelope (or the union of envelope and
    /// scene) so the user can always pull back to see the plate.
    view_radius: f32,
    /// Drives the zoom-IN limit. Anchored to the scene's own AABB so
    /// a user can zoom closer than `view_radius * MIN_DISTANCE_FACTOR`
    /// on a model much smaller than the envelope. Equal to
    /// `view_radius` for the single-radius constructor.
    detail_radius: f32,
}

impl OrbitCamera {
    /// Single-radius constructor: `radius` is used as both the framing
    /// radius (zoom-out / initial distance) and the detail radius
    /// (zoom-in). Equivalent to
    /// `with_detail_radius(scene_center, radius, radius)`. Test-only
    /// for now since production call sites all have a separate
    /// `view_radius` and `detail_radius` to plumb in.
    #[cfg(test)]
    pub fn new(scene_center: Vec3, scene_radius: f32) -> Self {
        Self::with_detail_radius(scene_center, scene_radius, scene_radius)
    }

    /// Two-radius constructor. `view_radius` drives the initial
    /// framing distance and the zoom-OUT limit; `detail_radius`
    /// drives the zoom-IN limit only. Both are clamped to a minimum
    /// of `1e-3` mm to keep arithmetic well-defined for degenerate
    /// inputs.
    ///
    /// Used by the scene-import path so the camera anchors to the
    /// build envelope at first sight (zoom-out / orbit centre) while
    /// still letting the user dolly in close to a small model.
    pub fn with_detail_radius(scene_center: Vec3, view_radius: f32, detail_radius: f32) -> Self {
        let view_radius = view_radius.max(1e-3);
        let detail_radius = detail_radius.max(1e-3);
        Self {
            target: scene_center,
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            distance: view_radius * DEFAULT_DISTANCE_FACTOR,
            up: Vec3::Z,
            scene_center,
            view_radius,
            detail_radius,
        }
    }

    pub fn frame_scene(&mut self) {
        self.target = self.scene_center;
        self.yaw = DEFAULT_YAW;
        self.pitch = DEFAULT_PITCH;
        self.distance = self.view_radius * DEFAULT_DISTANCE_FACTOR;
    }

    pub fn set_axis_view(&mut self, view: AxisView) {
        self.target = self.scene_center;
        self.distance = self.view_radius * DEFAULT_DISTANCE_FACTOR;
        let (yaw, pitch) = match view {
            AxisView::Front => (-std::f32::consts::FRAC_PI_2, 0.0),
            AxisView::Back => (std::f32::consts::FRAC_PI_2, 0.0),
            AxisView::Right => (0.0, 0.0),
            AxisView::Left => (std::f32::consts::PI, 0.0),
            AxisView::Top => (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2),
            AxisView::Bottom => (-std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_2),
        };
        self.yaw = yaw;
        self.pitch = clamp_pitch(pitch);
    }

    pub fn orbit(&mut self, dx_pixels: f32, dy_pixels: f32, viewport_height: f32) {
        let sensitivity = std::f32::consts::PI / viewport_height.max(1.0);
        self.yaw -= dx_pixels * sensitivity;
        self.pitch = clamp_pitch(self.pitch + dy_pixels * sensitivity);
    }

    pub fn pan(&mut self, dx_pixels: f32, dy_pixels: f32, fov_y: f32, viewport_height: f32) {
        let world_per_pixel = self.world_per_pixel(fov_y, viewport_height);
        let (right, up) = self.right_and_up();
        self.target += -dx_pixels * world_per_pixel * right + dy_pixels * world_per_pixel * up;
    }

    pub fn zoom_at(
        &mut self,
        factor: f32,
        cursor_x: f32,
        cursor_y: f32,
        fov_y: f32,
        viewport_width: f32,
        viewport_height: f32,
    ) {
        let factor = factor.clamp(0.1, 10.0);
        let world_per_pixel = self.world_per_pixel(fov_y, viewport_height);
        let (right, up) = self.right_and_up();
        let cx = cursor_x - viewport_width * 0.5;
        let cy = cursor_y - viewport_height * 0.5;
        let world_at_cursor =
            self.target + cx * world_per_pixel * right - cy * world_per_pixel * up;

        self.target += (1.0 - factor) * (world_at_cursor - self.target);
        self.distance = (self.distance * factor).clamp(self.min_distance(), self.max_distance());
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, self.up)
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.forward_offset()
    }

    fn forward_offset(&self) -> Vec3 {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(
            self.distance * cos_pitch * cos_yaw,
            self.distance * cos_pitch * sin_yaw,
            self.distance * sin_pitch,
        )
    }

    fn right_and_up(&self) -> (Vec3, Vec3) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let right = forward.cross(self.up).normalize_or_zero();
        let cam_up = right.cross(forward).normalize_or_zero();
        (right, cam_up)
    }

    /// World-space units per logical pixel at the camera's current
    /// distance. Used by the camera's own pan/zoom code, by
    /// `gizmo::gizmo_scale`/`hit_radius_world`, and by the controller
    /// to derive world-space sizes for the gizmo's cube and arc
    /// handles from their screen-pixel constants.
    pub fn world_per_pixel(&self, fov_y: f32, viewport_height: f32) -> f32 {
        let h = viewport_height.max(1.0);
        2.0 * self.distance * (fov_y * 0.5).tan() / h
    }

    fn min_distance(&self) -> f32 {
        // Scene-derived floor: lets the user dolly close on a small
        // model. Bounded below by a fraction of `view_radius` so the
        // camera doesn't slide inside the renderer's near plane
        // (which scales with the framing AABB; see `frame::NEAR_MULT`).
        let scene_floor = self.detail_radius * MIN_DISTANCE_FACTOR;
        let near_safety = self.view_radius * NEAR_PLANE_SAFETY_FACTOR;
        scene_floor.max(near_safety)
    }

    fn max_distance(&self) -> f32 {
        self.view_radius * MAX_DISTANCE_FACTOR
    }
}

fn clamp_pitch(pitch: f32) -> f32 {
    let limit = std::f32::consts::FRAC_PI_2 - PITCH_EPSILON;
    pitch.clamp(-limit, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn frame_resets_to_defaults() {
        let mut cam = OrbitCamera::new(Vec3::new(1.0, 2.0, 3.0), 5.0);
        cam.yaw = 0.0;
        cam.pitch = 1.0;
        cam.distance = 1.0;
        cam.target = Vec3::ZERO;
        cam.frame_scene();
        assert_approx_eq!(f32, cam.target.x, 1.0);
        assert_approx_eq!(f32, cam.distance, 5.0 * DEFAULT_DISTANCE_FACTOR);
        assert_approx_eq!(f32, cam.yaw, DEFAULT_YAW);
        assert_approx_eq!(f32, cam.pitch, DEFAULT_PITCH);
    }

    #[test]
    fn orbit_clamps_pitch() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 1.0);
        cam.orbit(0.0, 1e6, 100.0);
        assert!(cam.pitch < std::f32::consts::FRAC_PI_2);
        cam.orbit(0.0, -1e6, 100.0);
        assert!(cam.pitch > -std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn zoom_in_reduces_distance() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        let d0 = cam.distance;
        cam.zoom_at(0.9, 100.0, 100.0, 1.0, 200.0, 200.0);
        assert!(cam.distance < d0);
    }

    #[test]
    fn zoom_at_center_does_not_move_target() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        let t0 = cam.target;
        cam.zoom_at(0.8, 200.0, 150.0, 1.0, 400.0, 300.0);
        assert_approx_eq!(f32, cam.target.x, t0.x, epsilon = 1e-4);
        assert_approx_eq!(f32, cam.target.y, t0.y, epsilon = 1e-4);
        assert_approx_eq!(f32, cam.target.z, t0.z, epsilon = 1e-4);
    }

    #[test]
    fn axis_top_looks_down_z() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 1.0);
        cam.set_axis_view(AxisView::Top);
        let offset = cam.eye() - cam.target;
        assert!(offset.z > offset.x.abs() * 100.0);
        assert!(offset.z > offset.y.abs() * 100.0);
        assert!(offset.z > 0.0);
    }

    #[test]
    fn with_detail_radius_uses_view_radius_for_initial_distance() {
        // Initial framing distance is `view_radius * DEFAULT_DISTANCE_FACTOR`
        // regardless of `detail_radius`, so the user always opens onto
        // a view sized by the envelope.
        let cam = OrbitCamera::with_detail_radius(Vec3::ZERO, 100.0, 1.0);
        assert_approx_eq!(f32, cam.distance, 100.0 * DEFAULT_DISTANCE_FACTOR);
    }

    #[test]
    fn with_detail_radius_uses_view_radius_for_max_distance() {
        // Pushing zoom out beyond the max clamps to
        // `view_radius * MAX_DISTANCE_FACTOR`, irrespective of
        // `detail_radius`.
        let mut cam = OrbitCamera::with_detail_radius(Vec3::ZERO, 100.0, 1.0);
        for _ in 0..50 {
            cam.zoom_at(1.5, 100.0, 100.0, 1.0, 200.0, 200.0);
        }
        assert_approx_eq!(
            f32,
            cam.distance,
            100.0 * MAX_DISTANCE_FACTOR,
            epsilon = 1e-3
        );
    }

    #[test]
    fn small_detail_radius_lets_zoom_in_below_view_radius_factor() {
        // With a tiny `detail_radius` relative to `view_radius`, the
        // zoom-in floor drops below the single-radius
        // `view_radius * MIN_DISTANCE_FACTOR` and bottoms out at the
        // near-plane-safety floor `view_radius * NEAR_PLANE_SAFETY_FACTOR`.
        let mut cam = OrbitCamera::with_detail_radius(Vec3::ZERO, 150.0, 0.1);
        for _ in 0..50 {
            cam.zoom_at(0.5, 100.0, 100.0, 1.0, 200.0, 200.0);
        }
        let single_radius_floor = 150.0 * MIN_DISTANCE_FACTOR;
        let near_safety = 150.0 * NEAR_PLANE_SAFETY_FACTOR;
        assert!(
            cam.distance < single_radius_floor,
            "dual-radius camera should zoom in past the single-radius floor: \
             distance={}, single_radius_floor={}",
            cam.distance,
            single_radius_floor
        );
        assert_approx_eq!(f32, cam.distance, near_safety, epsilon = 1e-3);
    }

    #[test]
    fn detail_radius_floors_at_near_plane_safety() {
        // A `detail_radius * MIN_DISTANCE_FACTOR` smaller than the
        // near-plane-safety floor must be lifted to that floor; the
        // camera never lands inside the renderer's near plane.
        let mut cam = OrbitCamera::with_detail_radius(Vec3::ZERO, 100.0, 0.001);
        for _ in 0..50 {
            cam.zoom_at(0.5, 100.0, 100.0, 1.0, 200.0, 200.0);
        }
        let near_safety = 100.0 * NEAR_PLANE_SAFETY_FACTOR;
        assert_approx_eq!(f32, cam.distance, near_safety, epsilon = 1e-3);
    }

    #[test]
    fn single_radius_constructor_keeps_view_radius_eq_detail_radius() {
        // The convenience constructor's zoom limits mirror the prior
        // single-radius behaviour: zoom floor is
        // `radius * MIN_DISTANCE_FACTOR` (above the near-plane safety
        // floor for any radius the prior code path produced).
        let mut cam = OrbitCamera::new(Vec3::ZERO, 100.0);
        for _ in 0..50 {
            cam.zoom_at(0.5, 100.0, 100.0, 1.0, 200.0, 200.0);
        }
        let expected = 100.0 * MIN_DISTANCE_FACTOR;
        assert_approx_eq!(f32, cam.distance, expected, epsilon = 1e-3);
    }
}
