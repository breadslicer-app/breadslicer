/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use glam::{Mat4, Vec3};

const PITCH_EPSILON: f32 = 1e-3;
const MIN_DISTANCE_FACTOR: f32 = 0.05;
const MAX_DISTANCE_FACTOR: f32 = 50.0;
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
    scene_radius: f32,
}

impl OrbitCamera {
    pub fn new(scene_center: Vec3, scene_radius: f32) -> Self {
        let radius = scene_radius.max(1e-3);
        Self {
            target: scene_center,
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            distance: radius * DEFAULT_DISTANCE_FACTOR,
            up: Vec3::Z,
            scene_center,
            scene_radius: radius,
        }
    }

    pub fn frame_scene(&mut self) {
        self.target = self.scene_center;
        self.yaw = DEFAULT_YAW;
        self.pitch = DEFAULT_PITCH;
        self.distance = self.scene_radius * DEFAULT_DISTANCE_FACTOR;
    }

    pub fn set_axis_view(&mut self, view: AxisView) {
        self.target = self.scene_center;
        self.distance = self.scene_radius * DEFAULT_DISTANCE_FACTOR;
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

    fn world_per_pixel(&self, fov_y: f32, viewport_height: f32) -> f32 {
        let h = viewport_height.max(1.0);
        2.0 * self.distance * (fov_y * 0.5).tan() / h
    }

    fn min_distance(&self) -> f32 {
        self.scene_radius * MIN_DISTANCE_FACTOR
    }

    fn max_distance(&self) -> f32 {
        self.scene_radius * MAX_DISTANCE_FACTOR
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
}
