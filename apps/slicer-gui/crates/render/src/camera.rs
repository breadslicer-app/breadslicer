/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use glam::{Mat3, Mat4, Vec3};

const PITCH_EPSILON: f32 = 1e-3;
/// How far from a pole (radians of pitch) an orbit starts turning the view
/// square with the plate, finishing exactly at the pole.
const POLE_ALIGN_BAND: f32 = 10.0 * std::f32::consts::PI / 180.0;
/// At a pole, how far (radians) a drag goes before the view moves: sideways
/// before it leaves a turn square with the plate, up or down before it leaves
/// the pole. The view holds there that long, then follows the drag with no
/// jump.
const POLE_HOLD: f32 = 5.0 * std::f32::consts::PI / 180.0;
const MIN_DISTANCE_FACTOR: f32 = 0.05;
const MAX_DISTANCE_FACTOR: f32 = 50.0;
/// Floor on the zoom-in distance expressed as a fraction of `view_radius`.
/// Keeps the camera ahead of the renderer's near plane
/// (`frame::NEAR_MULT = 0.01` on a framing diagonal of at most
/// `2 * view_radius` gives `0.02 * view_radius`; this leaves headroom for
/// unions that exceed the envelope on one axis).
const NEAR_PLANE_SAFETY_FACTOR: f32 = 0.03;
/// Azimuth the workspace opens at. Lands in the `-X`, `-Y` quadrant so the
/// first thing on screen is the machine's front, the side the operator stands
/// at and the one [`AxisView::Front`] looks from.
const DEFAULT_YAW: f32 = 0.6749 - std::f32::consts::PI;
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
    pub orthographic: bool,

    scene_center: Vec3,
    /// Drives the initial framing distance and the zoom-OUT limit. Anchored to
    /// the build envelope (or the union of envelope and scene) so the user can
    /// always pull back to see the plate.
    view_radius: f32,
    /// Drives the zoom-IN limit. Anchored to the scene's own AABB so a user can
    /// zoom closer than `view_radius * MIN_DISTANCE_FACTOR` on a model much
    /// smaller than the envelope. Equal to `view_radius` for the single-radius
    /// constructor.
    detail_radius: f32,
    /// At a pole, the yaw a sideways drag has asked for and the yaw it shows
    /// for it (see [`held_yaw`]). Any other change of the yaw starts afresh.
    pole_drag: Option<(f32, f32)>,
    /// At a pole, how far a drag has pulled away from it while the view held,
    /// and which pole (+1 up, -1 down).
    pole_pull: Option<(f32, f32)>,
}

impl OrbitCamera {
    /// Single-radius constructor: `radius` is used as both the framing radius
    /// (zoom-out / initial distance) and the detail radius (zoom-in).
    /// Equivalent to `with_detail_radius(scene_center, radius, radius)`.
    /// Test-only for now since production call sites all have a separate
    /// `view_radius` and `detail_radius` to plumb in.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(scene_center: Vec3, scene_radius: f32) -> Self {
        Self::with_detail_radius(scene_center, scene_radius, scene_radius)
    }

    /// Two-radius constructor. `view_radius` drives the initial framing
    /// distance and the zoom-OUT limit; `detail_radius` drives the zoom-IN
    /// limit only. Both are clamped to a minimum of `1e-3` mm to keep
    /// arithmetic well-defined for degenerate inputs.
    ///
    /// Used by the scene-import path so the camera anchors to the build
    /// envelope at first sight (zoom-out / orbit centre) while still letting
    /// the user dolly in close to a small model.
    pub fn with_detail_radius(scene_center: Vec3, view_radius: f32, detail_radius: f32) -> Self {
        let view_radius = view_radius.max(1e-3);
        let detail_radius = detail_radius.max(1e-3);
        Self {
            target: scene_center,
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            distance: view_radius * DEFAULT_DISTANCE_FACTOR,
            orthographic: false,
            scene_center,
            view_radius,
            detail_radius,
            pole_drag: None,
            pole_pull: None,
        }
    }

    pub fn frame_scene(&mut self) {
        self.target = self.scene_center;
        self.yaw = DEFAULT_YAW;
        self.pitch = DEFAULT_PITCH;
        self.distance = self.view_radius * DEFAULT_DISTANCE_FACTOR;
    }

    pub fn set_axis_view(&mut self, view: AxisView) {
        // A direction change must not discard the user's current framing.
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
        self.turn(-dx_pixels * sensitivity, dy_pixels * sensitivity, true);
    }

    /// Turns the camera about its target by `yaw` and `pitch` radians. A
    /// positive yaw moves the eye to its right (the scene turns as under a
    /// drag to the left); a positive pitch raises the eye (as a drag down).
    ///
    /// Climbing toward a pole, the last [`POLE_ALIGN_BAND`] also turns the
    /// view about the vertical, in step with the climb, so that it reaches the
    /// pole square with the plate (its edges level and upright on screen).
    /// Turning sideways at the pole is free; a drag (`held`) holds a moment
    /// at each turn square with the plate before it goes on, and at the pole
    /// before it leaves it.
    fn turn(&mut self, yaw: f32, pitch: f32, held: bool) {
        let before = self.pitch;
        let pitch = if held && at_pole(before) {
            let side = before.signum();
            let pulled = match self.pole_pull {
                Some((pulled, pole)) if pole == side => pulled,
                _ => 0.0,
            };
            // Away from the pole counts up; back toward it, down to nothing.
            let pulled = (pulled - pitch * side).max(0.0);
            if pulled <= POLE_HOLD {
                self.pole_pull = Some((pulled, side));
                0.0
            } else {
                self.pole_pull = None;
                -side * (pulled - POLE_HOLD)
            }
        } else {
            self.pole_pull = None;
            pitch
        };
        if held && at_pole(before) && at_pole(before + pitch) {
            let asked = match self.pole_drag {
                Some((asked, shown)) if shown.to_bits() == self.yaw.to_bits() => asked,
                _ => asked_yaw(self.yaw),
            } + yaw;
            self.yaw = held_yaw(asked);
            self.pole_drag = Some((asked, self.yaw));
        } else {
            self.yaw += yaw;
            self.pole_drag = None;
        }
        self.pitch = clamp_pitch(self.pitch + pitch);
        let limit = std::f32::consts::FRAC_PI_2 - PITCH_EPSILON;
        let left_before = (limit - before.abs()).min(POLE_ALIGN_BAND);
        let left_after = limit - self.pitch.abs();
        if self.pitch.abs() > before.abs() && left_before > 0.0 && left_after < left_before {
            let share = (left_before - left_after) / left_before;
            let square = self.yaw + to_nearest_quarter_turn(self.yaw);
            self.yaw = if left_after <= 0.0 {
                // Exactly square, so a drag at the pole starts mid-hold.
                square
            } else {
                self.yaw + share * (square - self.yaw)
            };
        }
    }

    /// Orbits like [`Self::orbit`], but about `pivot` instead of the target:
    /// the eye and the target turn together around the pivot, so the pivot
    /// keeps its place on screen in both projections and the distance (the
    /// zoom) stays the same.
    pub fn orbit_around(
        &mut self,
        pivot: Vec3,
        dx_pixels: f32,
        dy_pixels: f32,
        viewport_height: f32,
    ) {
        let sensitivity = std::f32::consts::PI / viewport_height.max(1.0);
        self.turn_about(
            pivot,
            -dx_pixels * sensitivity,
            dy_pixels * sensitivity,
            true,
        );
    }

    /// Turns the camera about `pivot` by `yaw` and `pitch` radians, in the
    /// directions of [`Self::turn`]; the pivot keeps its place on screen. A
    /// keyed step: it does not hold at the pole.
    pub fn turn_around(&mut self, pivot: Vec3, yaw: f32, pitch: f32) {
        self.turn_about(pivot, yaw, pitch, false);
    }

    fn turn_about(&mut self, pivot: Vec3, yaw: f32, pitch: f32, held: bool) {
        let before = self.view_basis();
        self.turn(yaw, pitch, held);
        // The rotation that carries the old view frame onto the new one; it
        // carries the eye offset too, so turning the target with it about the
        // pivot turns the whole camera rigidly about the pivot.
        let turn = self.view_basis() * before.transpose();
        self.target = pivot + turn * (self.target - pivot);
    }

    /// The camera frame as columns: right, up and backward (toward the eye).
    fn view_basis(&self) -> Mat3 {
        let (right, up) = self.right_and_up();
        Mat3::from_cols(right, up, right.cross(up))
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
        let (_, up) = self.right_and_up();
        Mat4::look_at_rh(self.eye(), self.target, up)
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

    /// The screen's right and up directions in the world. The orbit turns
    /// about the world's Z, so the right direction is level and set by the
    /// yaw alone, at any pitch: the frame stays continuous all the way to
    /// both poles, and looking straight up or down never flips the view.
    pub fn right_and_up(&self) -> (Vec3, Vec3) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let right = Vec3::new(-sin_yaw, cos_yaw, 0.0);
        let cam_up = right.cross(forward).normalize_or_zero();
        (right, cam_up)
    }

    /// Returns the world-space size of one logical pixel on the plane that
    /// holds `target` and faces the camera. The size is different for points
    /// off that plane: see [`Self::world_per_pixel_at`].
    pub fn world_per_pixel(&self, fov_y: f32, viewport_height: f32) -> f32 {
        world_per_pixel_at_depth(self.distance, fov_y, viewport_height)
    }

    /// Returns the world-space size of one logical pixel at the distance of
    /// `point` from the eye along the view direction. Returns `None` when
    /// `point` is not in front of the eye.
    pub fn world_per_pixel_at(&self, point: Vec3, fov_y: f32, viewport_height: f32) -> Option<f32> {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let depth = (point - self.eye()).dot(forward);
        (depth > 0.0).then(|| {
            if self.orthographic {
                self.world_per_pixel(fov_y, viewport_height)
            } else {
                world_per_pixel_at_depth(depth, fov_y, viewport_height)
            }
        })
    }

    /// Both projection modes have the same scale at the orbit target plane.
    /// Picking, gizmos and rendering must all use this projection.
    pub fn projection(&self, fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        if self.orthographic {
            let half_y = self.distance * (fov_y * 0.5).tan();
            let half_x = half_y * aspect;
            Mat4::orthographic_rh(-half_x, half_x, -half_y, half_y, near, far)
        } else {
            Mat4::perspective_rh(fov_y, aspect, near, far)
        }
    }

    /// A screen ray in either projection. Orthographic rays have parallel
    /// directions and different origins; perspective rays share the eye.
    pub fn screen_ray(&self, fov_y: f32, width: f32, height: f32, x: f32, y: f32) -> (Vec3, Vec3) {
        let inverse = self.view_matrix().inverse();
        let tan_half = (fov_y * 0.5).tan();
        let px = (2.0 * x / width.max(1.0) - 1.0) * width.max(1.0) / height.max(1.0) * tan_half;
        let py = (1.0 - 2.0 * y / height.max(1.0)) * tan_half;
        if self.orthographic {
            (
                inverse.transform_point3(Vec3::new(px * self.distance, py * self.distance, 0.0)),
                inverse.transform_vector3(-Vec3::Z).normalize(),
            )
        } else {
            (
                self.eye(),
                inverse
                    .transform_vector3(Vec3::new(px, py, -1.0))
                    .normalize(),
            )
        }
    }

    /// Fit a bounding sphere, preserving orientation and projection. The
    /// narrower FOV controls portrait windows as well as landscape windows.
    pub fn frame_bounds(&mut self, min: Vec3, max: Vec3, fov_y: f32, aspect: f32) {
        self.target = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(1e-3);
        self.detail_radius = radius;
        self.view_radius = self.view_radius.max(radius);
        self.distance = framing_distance(radius, fov_y, aspect).max(self.min_distance());
    }

    /// Centres the orbit on `point`, keeping the view direction and the
    /// projection. A camera farther away than the distance that frames a
    /// sphere of `radius` around the point comes in to it; a closer one keeps
    /// its distance, so a jump never zooms out.
    pub fn centre_on(&mut self, point: Vec3, radius: f32, fov_y: f32, aspect: f32) {
        self.target = point;
        let framed = framing_distance(radius.max(1e-3), fov_y, aspect)
            .clamp(self.min_distance(), self.max_distance());
        self.distance = self.distance.min(framed);
    }

    fn min_distance(&self) -> f32 {
        // Scene-derived floor: lets the user dolly close on a small model.
        // Bounded below by a fraction of `view_radius` so the camera doesn't
        // slide inside the renderer's near plane (which scales with the framing
        // AABB; see `frame::NEAR_MULT`).
        let scene_floor = self.detail_radius * MIN_DISTANCE_FACTOR;
        let near_safety = self.view_radius * NEAR_PLANE_SAFETY_FACTOR;
        scene_floor.max(near_safety)
    }

    fn max_distance(&self) -> f32 {
        self.view_radius * MAX_DISTANCE_FACTOR
    }
}

/// Whether `pitch` sits at a pole, where the clamp holds it.
fn at_pole(pitch: f32) -> bool {
    pitch.abs() >= std::f32::consts::FRAC_PI_2 - PITCH_EPSILON - 1e-6
}

/// The yaw shown at a pole for the yaw a drag asked for: a turn square with
/// the plate holds while the drag stays within [`POLE_HOLD`] of it, and in
/// between the view follows the drag, stretched so nothing jumps.
fn held_yaw(asked: f32) -> f32 {
    let quarter = std::f32::consts::FRAC_PI_2;
    let turns = (asked / quarter).floor();
    // Square turns are always a whole number times a quarter, so a hold
    // shows the very same yaw whichever side it is reached from.
    let below = turns * quarter;
    let past = asked - below;
    if past <= POLE_HOLD {
        below
    } else if past >= quarter - POLE_HOLD {
        (turns + 1.0) * quarter
    } else {
        below + (past - POLE_HOLD) * quarter / (quarter - 2.0 * POLE_HOLD)
    }
}

/// An asked yaw [`held_yaw`] shows as `shown` (a square turn: the middle of
/// its hold).
fn asked_yaw(shown: f32) -> f32 {
    let quarter = std::f32::consts::FRAC_PI_2;
    let square = (shown / quarter).round() * quarter;
    if (shown - square).abs() < 1e-5 {
        return square;
    }
    let below = (shown / quarter).floor() * quarter;
    let past = shown - below;
    below + POLE_HOLD + past * (quarter - 2.0 * POLE_HOLD) / quarter
}

/// The turn (radians, either way) that brings `yaw` to the nearest quarter
/// turn, where the plate's edges are level and upright on screen.
fn to_nearest_quarter_turn(yaw: f32) -> f32 {
    let quarter = std::f32::consts::FRAC_PI_2;
    (yaw / quarter).round() * quarter - yaw
}

fn clamp_pitch(pitch: f32) -> f32 {
    let limit = std::f32::consts::FRAC_PI_2 - PITCH_EPSILON;
    pitch.clamp(-limit, limit)
}

/// The distance at which a sphere of `radius` fills the narrower field of
/// view, with a little margin.
fn framing_distance(radius: f32, fov_y: f32, aspect: f32) -> f32 {
    let tan_half = (fov_y * 0.5).tan() * aspect.clamp(0.01, 1.0);
    1.12 * radius / tan_half.atan().sin()
}

fn world_per_pixel_at_depth(depth: f32, fov_y: f32, viewport_height: f32) -> f32 {
    2.0 * depth * (fov_y * 0.5).tan() / viewport_height.max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn centring_on_a_point_keeps_the_view_direction() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 100.0);
        let (yaw, pitch, projection) = (cam.yaw, cam.pitch, cam.orthographic);
        let far = cam.distance;
        cam.centre_on(Vec3::new(10.0, -5.0, 3.0), 15.0, 0.8, 1.5);
        assert_eq!(cam.target, Vec3::new(10.0, -5.0, 3.0));
        assert_eq!(
            (cam.yaw, cam.pitch, cam.orthographic),
            (yaw, pitch, projection)
        );
        assert!(
            cam.distance < far,
            "a far camera comes in: {}",
            cam.distance
        );
        assert_approx_eq!(
            f32,
            cam.distance,
            framing_distance(15.0, 0.8, 1.5),
            ulps = 4
        );
    }

    #[test]
    fn centring_on_a_point_never_zooms_out() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 100.0);
        cam.distance = 20.0;
        cam.centre_on(Vec3::new(1.0, 2.0, 3.0), 15.0, 0.8, 1.5);
        assert_eq!(cam.distance, 20.0);
        assert_eq!(cam.target, Vec3::new(1.0, 2.0, 3.0));
    }

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
    fn default_view_opens_on_the_front_left() {
        let cam = OrbitCamera::new(Vec3::new(5.0, 7.0, 1.0), 10.0);
        let offset = cam.eye() - cam.target;
        assert!(
            offset.x < 0.0 && offset.y < 0.0 && offset.z > 0.0,
            "the eye must open above the front-left quadrant, got {offset}"
        );
    }

    #[test]
    fn frame_scene_returns_to_the_front_left_view() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.orbit(500.0, 60.0, 100.0);
        cam.frame_scene();
        let offset = cam.eye() - cam.target;
        assert!(
            offset.x < 0.0 && offset.y < 0.0 && offset.z > 0.0,
            "framing must come back to the front-left quadrant, got {offset}"
        );
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
    fn axis_front_looks_along_plus_y() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 1.0);
        cam.set_axis_view(AxisView::Front);
        let offset = cam.eye() - cam.target;
        assert!(offset.y < 0.0);
        assert!(-offset.y > offset.x.abs() * 100.0);
        assert!(-offset.y > offset.z.abs() * 100.0);
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
        // regardless of `detail_radius`, so the user always opens onto a view
        // sized by the envelope.
        let cam = OrbitCamera::with_detail_radius(Vec3::ZERO, 100.0, 1.0);
        assert_approx_eq!(f32, cam.distance, 100.0 * DEFAULT_DISTANCE_FACTOR);
    }

    #[test]
    fn with_detail_radius_uses_view_radius_for_max_distance() {
        // Pushing zoom out beyond the max clamps to
        // `view_radius * MAX_DISTANCE_FACTOR`, irrespective of `detail_radius`.
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
        // With a tiny `detail_radius` relative to `view_radius`, the zoom-in
        // floor drops below the single-radius
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
        // near-plane-safety floor must be lifted to that floor; the camera
        // never lands inside the renderer's near plane.
        let mut cam = OrbitCamera::with_detail_radius(Vec3::ZERO, 100.0, 0.001);
        for _ in 0..50 {
            cam.zoom_at(0.5, 100.0, 100.0, 1.0, 200.0, 200.0);
        }
        let near_safety = 100.0 * NEAR_PLANE_SAFETY_FACTOR;
        assert_approx_eq!(f32, cam.distance, near_safety, epsilon = 1e-3);
    }

    #[test]
    fn pan_at_top_pole_matches_pan_just_below_clamp() {
        // `clamp_pitch` keeps `pitch` strictly below `FRAC_PI_2`, so production
        // code paths land just below the pole. If a future caller bypasses the
        // clamp (or `set_axis_view` ever stops clamping for `AxisView::Top`),
        // the basis must stay consistent with the just-below-clamp orientation:
        // f32 rounding makes `cos(FRAC_PI_2)` slightly NEGATIVE, which silently
        // flips the sign of `forward.cross(Z)` and therefore the pan direction.
        // The reference orientation below pans in `-X`; the pole orientation
        // must pan in the same half-space.
        let fov_y = std::f32::consts::FRAC_PI_4;
        let viewport_h = 720.0;
        let yaw = -std::f32::consts::FRAC_PI_2;

        let mut reference = OrbitCamera::new(Vec3::ZERO, 10.0);
        reference.yaw = yaw;
        reference.pitch = std::f32::consts::FRAC_PI_2 - PITCH_EPSILON;
        let r0 = reference.target;
        reference.pan(100.0, 0.0, fov_y, viewport_h);
        let reference_dx = reference.target.x - r0.x;

        let mut pole = OrbitCamera::new(Vec3::ZERO, 10.0);
        pole.yaw = yaw;
        pole.pitch = std::f32::consts::FRAC_PI_2;
        let p0 = pole.target;
        pole.pan(100.0, 0.0, fov_y, viewport_h);
        let pole_dx = pole.target.x - p0.x;

        assert!(
            reference_dx * pole_dx > 0.0,
            "pan direction flipped at pitch = +pi/2: \
             reference_dx={reference_dx}, pole_dx={pole_dx}",
        );
    }

    #[test]
    fn pan_at_bottom_pole_matches_pan_just_above_clamp() {
        let fov_y = std::f32::consts::FRAC_PI_4;
        let viewport_h = 720.0;
        let yaw = -std::f32::consts::FRAC_PI_2;

        let mut reference = OrbitCamera::new(Vec3::ZERO, 10.0);
        reference.yaw = yaw;
        reference.pitch = -std::f32::consts::FRAC_PI_2 + PITCH_EPSILON;
        let r0 = reference.target;
        reference.pan(100.0, 0.0, fov_y, viewport_h);
        let reference_dx = reference.target.x - r0.x;

        let mut pole = OrbitCamera::new(Vec3::ZERO, 10.0);
        pole.yaw = yaw;
        pole.pitch = -std::f32::consts::FRAC_PI_2;
        let p0 = pole.target;
        pole.pan(100.0, 0.0, fov_y, viewport_h);
        let pole_dx = pole.target.x - p0.x;

        assert!(
            reference_dx * pole_dx > 0.0,
            "pan direction flipped at pitch = -pi/2: \
             reference_dx={reference_dx}, pole_dx={pole_dx}",
        );
    }

    #[test]
    fn orbiting_to_either_pole_never_turns_the_view() {
        // Dragging up or down in small steps from the side view to the
        // clamp: the screen frame tilts with the step and, near the pole,
        // turns square with the plate a little at a time (at most 45 degrees
        // over the last 10), never in a jump. The frame used to swap its
        // reference axis at ~82 degrees from the horizon, which turned the
        // view by the yaw (from above) or flipped it upside down (from below).
        let limit = std::f32::consts::FRAC_PI_2 - PITCH_EPSILON;
        let step = 0.25_f32.to_radians();
        let most = step * (1.0 + 45.0 / 10.0) * 1.01;
        for yaw_deg in [-180.0_f32, -135.0, -90.0, -30.0, 0.0, 44.0, 90.0, 160.0] {
            for direction in [1.0_f32, -1.0] {
                let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
                cam.yaw = yaw_deg.to_radians();
                cam.pitch = 0.0;
                let (mut right, mut up) = cam.right_and_up();
                while cam.pitch.abs() < limit {
                    cam.turn(0.0, direction * step, true);
                    let (next_right, next_up) = cam.right_and_up();
                    let turned = right
                        .angle_between(next_right)
                        .max(up.angle_between(next_up));
                    assert!(
                        turned <= most,
                        "yaw {yaw_deg}, pitch {}: the view turned {} degrees in one step",
                        cam.pitch.to_degrees(),
                        turned.to_degrees()
                    );
                    (right, up) = (next_right, next_up);
                }
            }
        }
    }

    #[test]
    fn an_orbit_reaches_either_pole_square_with_the_plate() {
        // From any side, dragging up or down to the clamp turns the view so
        // the plate's edges end level and upright on screen; a jump straight
        // to the pole (a numpad step) does the same.
        for yaw_deg in [-170.0_f32, -100.0, -60.0, -12.0, 20.0, 44.0, 46.0, 130.0] {
            for direction in [1.0_f32, -1.0] {
                for step_deg in [0.25_f32, 15.0, 180.0] {
                    let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
                    cam.yaw = yaw_deg.to_radians();
                    cam.pitch = 0.3 * direction;
                    for _ in 0..1000 {
                        cam.turn(0.0, direction * step_deg.to_radians(), true);
                    }
                    let (right, _) = cam.right_and_up();
                    let square = right.x.abs().max(right.y.abs());
                    assert!(
                        square > 1.0 - 1e-5,
                        "yaw {yaw_deg}, step {step_deg}, direction {direction}: right = {right}"
                    );
                    assert!(
                        to_nearest_quarter_turn(cam.yaw).abs() < 1e-5,
                        "the yaw lands on a quarter turn: {}",
                        cam.yaw.to_degrees()
                    );
                }
            }
        }
    }

    #[test]
    fn at_the_pole_a_sideways_drag_still_turns_the_view() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.yaw = 0.4;
        cam.orbit(0.0, 1e6, 100.0);
        let (before, _) = cam.right_and_up();
        cam.orbit(30.0, 0.0, 100.0);
        let (after, _) = cam.right_and_up();
        assert!(
            before.angle_between(after) > 0.5,
            "the view turned about the vertical"
        );
        // Pushing on into the pole leaves that turn alone.
        cam.orbit(0.0, 50.0, 100.0);
        let (pushed, _) = cam.right_and_up();
        assert!(after.angle_between(pushed) < 1e-5);
    }

    #[test]
    fn at_the_pole_a_drag_holds_a_moment_at_each_square_turn() {
        // Looking straight down, square with the plate: a short sideways
        // drag leaves the view where it is; a longer one turns it, from where
        // it held, with no jump; dragging back holds there again.
        let height = 900.0;
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.yaw = 0.4;
        cam.orbit(0.0, 1e6, height);
        let square = cam.yaw;
        assert!(to_nearest_quarter_turn(square).abs() < 1e-6);
        let px_per_rad = height / std::f32::consts::PI;
        let hold_px = POLE_HOLD * px_per_rad;
        // Inside the hold, in little steps either way.
        for _ in 0..10 {
            cam.orbit(hold_px * 0.09, 0.0, height);
        }
        assert_eq!(cam.yaw, square);
        for _ in 0..19 {
            cam.orbit(-hold_px * 0.09, 0.0, height);
        }
        assert_eq!(cam.yaw, square);
        // Out of it, smoothly: one pixel at a time, never a jump.
        let mut last = cam.yaw;
        let step = (std::f32::consts::FRAC_PI_2 / (std::f32::consts::FRAC_PI_2 - 2.0 * POLE_HOLD))
            / px_per_rad;
        for _ in 0..(3.0 * hold_px) as usize {
            cam.orbit(-1.0, 0.0, height);
            assert!(
                (cam.yaw - last).abs() <= step * 1.01,
                "{} -> {}",
                last,
                cam.yaw
            );
            last = cam.yaw;
        }
        assert!(cam.yaw > square + 0.05, "the view turned: {}", cam.yaw);
        // A whole turn the same way passes every square turn, holding at each.
        let mut held = 0;
        for _ in 0..(2.0 * std::f32::consts::PI * px_per_rad) as usize {
            let was = cam.yaw;
            cam.orbit(-1.0, 0.0, height);
            if cam.yaw == was {
                held += 1;
            }
        }
        let per_hold = (2.0 * hold_px) as i32;
        assert!(
            (held - 4 * per_hold).abs() <= 8,
            "{held} pixels held, {per_hold} each"
        );
    }

    #[test]
    fn arriving_at_the_pole_from_any_side_the_hold_is_centred() {
        let height = 900.0;
        let hold_px = POLE_HOLD * height / std::f32::consts::PI;
        for yaw in [0.4_f32, 1.234_567, -2.6, 3.0] {
            for way in [1.0_f32, -1.0] {
                let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
                cam.yaw = yaw;
                for _ in 0..200 {
                    cam.orbit(0.0, 7.0, height);
                }
                let square = cam.yaw;
                assert_eq!(to_nearest_quarter_turn(square), 0.0, "{square}");
                cam.orbit(way * hold_px * 0.95, 0.0, height);
                assert_eq!(cam.yaw, square, "yaw {yaw}, way {way}");
            }
        }
    }

    #[test]
    fn at_the_pole_a_drag_holds_a_moment_before_it_leaves() {
        // Up or down, a short drag away from the pole leaves the view there;
        // a longer one leaves it with no jump; dragging back holds again.
        let height = 900.0;
        let px_per_rad = height / std::f32::consts::PI;
        let hold_px = POLE_HOLD * px_per_rad;
        for pole in [1.0_f32, -1.0] {
            let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
            cam.orbit(0.0, pole * 1e6, height);
            let at = cam.pitch;
            assert!(at_pole(at));
            // Away in little steps, then back: it stays.
            for _ in 0..10 {
                cam.orbit(0.0, -pole * hold_px * 0.09, height);
            }
            for _ in 0..10 {
                cam.orbit(0.0, pole * hold_px * 0.09, height);
            }
            assert_eq!(cam.pitch, at);
            for _ in 0..10 {
                cam.orbit(0.0, -pole * hold_px * 0.09, height);
            }
            assert_eq!(cam.pitch, at, "pole {pole}: still inside the hold");
            // Out of it, a pixel at a time, never more than a pixel's turn.
            let mut last = cam.pitch;
            for _ in 0..(3.0 * hold_px) as usize {
                cam.orbit(0.0, -pole, height);
                assert!(
                    (cam.pitch - last).abs() <= 1.01 / px_per_rad,
                    "{last} -> {}",
                    cam.pitch
                );
                last = cam.pitch;
            }
            assert!(!at_pole(cam.pitch), "pole {pole}: it left");
            // Off the pole a drag turns at once.
            let off = cam.pitch;
            cam.orbit(0.0, -pole, height);
            assert!((cam.pitch - off).abs() > 0.5 / px_per_rad);
        }
    }

    #[test]
    fn at_the_pole_a_small_drag_any_way_leaves_the_view_alone() {
        let height = 900.0;
        let hold_px = POLE_HOLD * height / std::f32::consts::PI;
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.yaw = 1.0;
        cam.orbit(0.0, -1e6, height);
        let (yaw, pitch) = (cam.yaw, cam.pitch);
        for (dx, dy) in [(0.7, 0.7), (-0.7, 0.7), (0.7, 0.0), (0.0, 0.9), (-0.9, 0.0)] {
            cam.orbit(dx * hold_px, dy * hold_px, height);
            assert_eq!((cam.yaw, cam.pitch), (yaw, pitch), "({dx}, {dy})");
            cam.orbit(-dx * hold_px, -dy * hold_px, height);
            assert_eq!((cam.yaw, cam.pitch), (yaw, pitch), "back from ({dx}, {dy})");
        }
    }

    #[test]
    fn a_keyed_step_leaves_the_pole_by_its_full_step() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.set_axis_view(AxisView::Top);
        let pitch = cam.pitch;
        cam.turn_around(Vec3::ZERO, 0.0, -15.0_f32.to_radians());
        assert!((pitch - cam.pitch - 15.0_f32.to_radians()).abs() < 1e-5);
    }

    #[test]
    fn a_keyed_step_at_the_pole_turns_its_full_step() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.set_axis_view(AxisView::Top);
        let yaw = cam.yaw;
        cam.turn_around(Vec3::ZERO, 15.0_f32.to_radians(), 0.0);
        assert!((cam.yaw - yaw - 15.0_f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn the_hold_maps_back_to_what_it_shows() {
        for shown_deg in [
            -180.0_f32, -100.0, -90.0, -3.0, 0.0, 1.0, 44.0, 89.0, 90.0, 270.0,
        ] {
            let shown = shown_deg.to_radians();
            let back = held_yaw(asked_yaw(shown));
            assert!(
                (back - shown).abs() < 1e-5,
                "{shown_deg}: {}",
                back.to_degrees()
            );
        }
    }

    #[test]
    fn far_from_the_poles_an_orbit_keeps_its_yaw() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.yaw = 0.4;
        cam.pitch = 0.0;
        cam.turn(0.0, 70.0_f32.to_radians(), true);
        assert_eq!(cam.yaw, 0.4);
        cam.turn(0.0, -140.0_f32.to_radians(), true);
        assert_eq!(cam.yaw, 0.4);
    }

    #[test]
    fn from_below_the_front_edge_is_at_the_top_of_the_view() {
        // Orbiting down from the front view, the plate's front edge (-Y) ends
        // at the top of the view and +X stays to the right; the underside
        // copy of the FRONT label is drawn for that view.
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.set_axis_view(AxisView::Bottom);
        let (right, up) = cam.right_and_up();
        assert!(right.dot(Vec3::X) > 0.999, "right = {right}");
        assert!(up.dot(-Vec3::Y) > 0.999, "up = {up}");
    }

    #[test]
    fn the_frame_is_level_and_square_at_every_pitch() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.yaw = 0.7;
        for pitch in [
            -std::f32::consts::FRAC_PI_2,
            -1.4,
            -0.3,
            0.0,
            0.9,
            1.5,
            std::f32::consts::FRAC_PI_2,
        ] {
            cam.pitch = pitch;
            let (right, up) = cam.right_and_up();
            let forward = (cam.target - cam.eye()).normalize();
            assert!(right.z.abs() < 1e-6, "pitch {pitch}: right is level");
            assert!(right.dot(up).abs() < 1e-5 && right.dot(forward).abs() < 1e-5);
            assert!(up.dot(forward).abs() < 1e-5, "pitch {pitch}");
            assert!((up.length() - 1.0).abs() < 1e-5, "pitch {pitch}");
        }
    }

    #[test]
    fn single_radius_constructor_keeps_view_radius_eq_detail_radius() {
        // The convenience constructor's zoom limits mirror the prior
        // single-radius behaviour: zoom floor is `radius * MIN_DISTANCE_FACTOR`
        // (above the near-plane safety floor for any radius the prior code path
        // produced).
        let mut cam = OrbitCamera::new(Vec3::ZERO, 100.0);
        for _ in 0..50 {
            cam.zoom_at(0.5, 100.0, 100.0, 1.0, 200.0, 200.0);
        }
        let expected = 100.0 * MIN_DISTANCE_FACTOR;
        assert_approx_eq!(f32, cam.distance, expected, epsilon = 1e-3);
    }

    #[test]
    fn world_per_pixel_at_target_matches_world_per_pixel() {
        let cam = OrbitCamera::new(Vec3::new(1.0, 2.0, 3.0), 10.0);
        let at_target = cam.world_per_pixel_at(cam.target, 0.8, 720.0).unwrap();
        assert_approx_eq!(
            f32,
            at_target,
            cam.world_per_pixel(0.8, 720.0),
            epsilon = 1e-5
        );
    }

    #[test]
    fn world_per_pixel_at_scales_with_depth_along_the_view_direction() {
        let cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        let forward = (cam.target - cam.eye()).normalize();
        let (right, _) = cam.right_and_up();
        let at_target = cam.world_per_pixel(0.8, 720.0);

        let twice_as_deep = cam.eye() + forward * (2.0 * cam.distance);
        let deep = cam.world_per_pixel_at(twice_as_deep, 0.8, 720.0).unwrap();
        assert_approx_eq!(f32, deep, 2.0 * at_target, epsilon = 1e-5);

        // A sideways offset keeps the depth, so the size stays the same.
        let sideways = cam.world_per_pixel_at(right * 7.0, 0.8, 720.0).unwrap();
        assert_approx_eq!(f32, sideways, at_target, epsilon = 1e-5);
    }

    #[test]
    fn world_per_pixel_at_is_none_at_or_behind_the_eye() {
        let cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        let backward = cam.eye() - cam.target;
        assert!(cam.world_per_pixel_at(cam.eye(), 0.8, 720.0).is_none());
        assert!(
            cam.world_per_pixel_at(cam.eye() + backward, 0.8, 720.0)
                .is_none()
        );
    }
    #[test]
    fn projection_toggle_preserves_target_plane_scale() {
        let mut cam = OrbitCamera::new(Vec3::new(4.0, 8.0, 2.0), 20.0);
        let (right, up) = cam.right_and_up();
        let point = cam.target + right * 7.0 + up * 4.0;
        let perspective =
            (cam.projection(0.8, 1.6, 0.01, 1000.0) * cam.view_matrix()).project_point3(point);
        cam.orthographic = true;
        let ortho =
            (cam.projection(0.8, 1.6, 0.01, 1000.0) * cam.view_matrix()).project_point3(point);
        assert!((perspective.truncate() - ortho.truncate()).length() < 1e-5);
    }

    #[test]
    fn orthographic_rays_are_parallel_with_pixel_spaced_origins() {
        let mut cam = OrbitCamera::new(Vec3::ZERO, 10.0);
        cam.orthographic = true;
        let (a, da) = cam.screen_ray(0.8, 1000.0, 600.0, 250.0, 200.0);
        let (b, db) = cam.screen_ray(0.8, 1000.0, 600.0, 750.0, 200.0);
        assert!((da - db).length() < 1e-6);
        assert!(((b - a).length() - 500.0 * cam.world_per_pixel(0.8, 600.0)).abs() < 1e-4);
        assert!((b - a).dot(da).abs() < 1e-4);
        assert_approx_eq!(
            f32,
            cam.world_per_pixel_at(cam.target, 0.8, 600.0).unwrap(),
            cam.world_per_pixel_at(cam.target + da * 10.0, 0.8, 600.0)
                .unwrap(),
            epsilon = 1e-5
        );
    }

    #[test]
    fn rays_reproject_to_requested_pixels_in_both_modes_and_all_views() {
        for ortho in [false, true] {
            for view in [
                AxisView::Front,
                AxisView::Back,
                AxisView::Right,
                AxisView::Left,
                AxisView::Top,
                AxisView::Bottom,
            ] {
                let mut cam = OrbitCamera::new(Vec3::new(4.0, 2.0, 8.0), 15.0);
                cam.orthographic = ortho;
                cam.set_axis_view(view);
                let vp = cam.projection(0.8, 1.5, 0.01, 1000.0) * cam.view_matrix();
                for (x, y) in [(200.0, 150.0), (450.0, 300.0), (750.0, 500.0)] {
                    let (origin, direction) = cam.screen_ray(0.8, 900.0, 600.0, x, y);
                    let ndc = vp.project_point3(origin + direction * cam.distance);
                    assert!(((ndc.x + 1.0) * 450.0 - x).abs() < 0.01);
                    assert!(((1.0 - ndc.y) * 300.0 - y).abs() < 0.01);
                }
            }
        }
    }

    #[test]
    fn fit_selected_bounds_survive_axis_switch_and_portrait_projection() {
        for ortho in [false, true] {
            for aspect in [0.5, 1.8] {
                let mut cam = OrbitCamera::new(Vec3::ZERO, 100.0);
                cam.orthographic = ortho;
                let min = Vec3::new(40.0, 20.0, -5.0);
                let max = Vec3::new(70.0, 30.0, 60.0);
                cam.frame_bounds(min, max, 0.8, aspect);
                let target = cam.target;
                let distance = cam.distance;
                for view in [AxisView::Front, AxisView::Right, AxisView::Top] {
                    cam.set_axis_view(view);
                    assert_eq!(cam.target, target);
                    assert_eq!(cam.distance, distance);
                    let vp = cam.projection(0.8, aspect, 0.01, 10000.0) * cam.view_matrix();
                    for x in [min.x, max.x] {
                        for y in [min.y, max.y] {
                            for z in [min.z, max.z] {
                                let ndc = vp.project_point3(Vec3::new(x, y, z));
                                assert!(
                                    ndc.x.abs() < 1.0 && ndc.y.abs() < 1.0,
                                    "bounds clipped: {ndc}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn orbit_around_keeps_the_pivot_still_on_screen() {
        let (width, height, fov) = (1200.0, 800.0, 0.8);
        for ortho in [false, true] {
            for pivot in [
                Vec3::new(80.0, -60.0, 5.0),
                Vec3::new(-30.0, 40.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
            ] {
                let mut cam = OrbitCamera::new(Vec3::new(0.0, 0.0, 50.0), 150.0);
                cam.orthographic = ortho;
                let screen = |cam: &OrbitCamera| {
                    let ndc = (cam.projection(fov, width / height, 0.1, 1.0e5) * cam.view_matrix())
                        .project_point3(pivot);
                    Vec3::new(
                        (ndc.x + 1.0) * width * 0.5,
                        (1.0 - ndc.y) * height * 0.5,
                        0.0,
                    )
                };
                let start = screen(&cam);
                let distance = cam.distance;
                // Small and large steps, and one that runs into the pitch clamp.
                for (dx, dy) in [(3.0, 2.0), (-40.0, 25.0), (250.0, -90.0), (0.0, 5000.0)] {
                    cam.orbit_around(pivot, dx, dy, height);
                    let drift = screen(&cam) - start;
                    assert!(drift.length() < 0.05, "drift {drift} (ortho {ortho})");
                    assert_eq!(cam.distance, distance);
                }
            }
        }
    }

    #[test]
    fn orbit_around_turns_like_orbit() {
        let mut free = OrbitCamera::new(Vec3::ZERO, 100.0);
        let mut pinned = free.clone();
        free.orbit(120.0, -40.0, 600.0);
        pinned.orbit_around(Vec3::new(30.0, 10.0, 0.0), 120.0, -40.0, 600.0);
        assert_eq!(free.yaw, pinned.yaw);
        assert_eq!(free.pitch, pinned.pitch);
    }

    #[test]
    fn orbit_around_the_target_is_a_plain_orbit() {
        let mut free = OrbitCamera::new(Vec3::new(5.0, 6.0, 7.0), 100.0);
        let mut pinned = free.clone();
        free.orbit(80.0, 30.0, 600.0);
        pinned.orbit_around(free.target, 80.0, 30.0, 600.0);
        assert!((free.eye() - pinned.eye()).length() < 1e-3);
        assert!((free.target - pinned.target).length() < 1e-3);
    }
}
