/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The build volume as the viewport draws and edits against it, and the rules
//! that the viewport applies with it: the fit test, the clamp of translate
//! edits and the camera framing.

use glam::Vec3;
use sc_mesh_core::Aabb;

/// The build volume as the viewport uses it. It holds a validated
/// [`sc_print_primitives::BuildVolume`] and reads its lengths and corners in
/// f32, the precision of mesh vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildVolume {
    volume: sc_print_primitives::BuildVolume,
}

impl From<sc_print_primitives::BuildVolume> for BuildVolume {
    fn from(volume: sc_print_primitives::BuildVolume) -> Self {
        Self { volume }
    }
}

impl BuildVolume {
    /// Builds a volume from its lengths in millimetres. Panics when a length is
    /// 0, negative, or non-finite.
    #[cfg(test)]
    pub(crate) fn new(x_mm: f64, y_mm: f64, z_mm: f64) -> Self {
        Self::from(
            sc_print_primitives::BuildVolume::new(x_mm, y_mm, z_mm)
                .expect("test build volumes have positive finite lengths"),
        )
    }

    /// Returns the X length in millimetres, rounded to f32.
    pub fn width_mm(&self) -> f32 {
        self.volume.x_mm() as f32
    }

    /// Returns the Y length in millimetres, rounded to f32.
    pub fn depth_mm(&self) -> f32 {
        self.volume.y_mm() as f32
    }

    /// Returns the Z length in millimetres, rounded to f32.
    pub fn height_mm(&self) -> f32 {
        self.volume.z_mm() as f32
    }

    /// Returns the minimum corner of
    /// [`sc_print_primitives::BuildVolume::envelope_aabb`].
    ///
    /// For "is geometry inside the envelope" checks, prefer
    /// [`Self::contains_aabb`] over comparing against the raw corner.
    pub fn envelope_min_mm(&self) -> Vec3 {
        Vec3::from(self.volume.envelope_aabb().min().coords())
    }

    /// Returns the maximum corner of
    /// [`sc_print_primitives::BuildVolume::envelope_aabb`].
    ///
    /// For "is geometry inside the envelope" checks, prefer
    /// [`Self::contains_aabb`] over comparing against the raw corner.
    pub fn envelope_max_mm(&self) -> Vec3 {
        Vec3::from(self.volume.envelope_aabb().max().coords())
    }

    /// Returns `true` iff `aabb` is fully inside the build envelope, i.e. every
    /// coordinate of `aabb.min()` is at or above the envelope's minimum corner
    /// and every coordinate of `aabb.max()` is at or below the envelope's
    /// maximum corner. An empty AABB returns `true` (vacuously contained: it
    /// has no extent to breach a wall).
    ///
    /// A convex envelope contains a mesh iff it contains the mesh's AABB, so
    /// the per-vertex shape test reduces to 1 call against the mesh's
    /// (post-transform) bounding box.
    pub fn contains_aabb(&self, aabb: Aabb) -> bool {
        if aabb.is_empty() {
            return true;
        }
        let lo = aabb.min().coords();
        let hi = aabb.max().coords();
        let env_lo = self.envelope_min_mm();
        let env_hi = self.envelope_max_mm();
        lo[0] >= env_lo.x
            && hi[0] <= env_hi.x
            && lo[1] >= env_lo.y
            && hi[1] <= env_hi.y
            && lo[2] >= env_lo.z
            && hi[2] <= env_hi.z
    }

    /// Returns the largest fraction of `delta` (per axis) by which `aabb_pre`
    /// can move and stay inside the envelope, with the additional rule that an
    /// axis already outside the envelope cannot be made worse: motion further
    /// away from the inside is blocked, motion back toward the inside is
    /// allowed up to the opposite wall.
    ///
    /// Per-axis behaviour, with `pre_min`, `pre_max` the input AABB extents on
    /// that axis, `env_min`, `env_max` the envelope extents, and `d` the
    /// proposed delta:
    ///
    /// - both extents inside: `d` clamped to `[env_min - pre_min, env_max -
    ///   pre_max]` (the range that keeps the AABB inside; contains `0`).
    /// - `pre_min < env_min` (already over the `-` wall): negative `d` is
    ///   clamped to `0`; positive `d` is clamped to `env_max - pre_max` (still
    ///   bounded by the opposite wall).
    /// - `pre_max > env_max` (already over the `+` wall): positive `d` is
    ///   clamped to `0`; negative `d` is clamped to `env_min - pre_min`.
    /// - `pre` larger than the envelope on that axis: both rules fire and `d`
    ///   is clamped to `0`.
    pub fn clamp_translation_delta(&self, aabb_pre: Aabb, delta: Vec3) -> Vec3 {
        let pre_min = aabb_pre.min().coords();
        let pre_max = aabb_pre.max().coords();
        let env_min = self.envelope_min_mm();
        let env_max = self.envelope_max_mm();
        let env_min_arr = [env_min.x, env_min.y, env_min.z];
        let env_max_arr = [env_max.x, env_max.y, env_max.z];
        let d_in = [delta.x, delta.y, delta.z];

        let mut out = [0.0f32; 3];
        for i in 0..3 {
            out[i] = clamp_axis(
                pre_min[i],
                pre_max[i],
                env_min_arr[i],
                env_max_arr[i],
                d_in[i],
            );
        }
        Vec3::new(out[0], out[1], out[2])
    }

    /// Returns the orbit-camera framing for the union of an optional scene AABB
    /// and the build envelope: the centre and the bounding-sphere radius
    /// (clamped to `1e-3` mm to keep `OrbitCamera::new` from producing a
    /// distance of 0 for degenerate inputs).
    ///
    /// `scene_aabb` is `Option<(min, max)>` to match
    /// `Renderer::scene_bounds()`; `None` frames on the envelope alone.
    /// Component-wise `min` / `max` against the envelope corners yields the
    /// union, so a degenerate scene AABB (sentinel `(+inf, -inf)`) collapses
    /// safely to envelope-only.
    pub fn framing(&self, scene_aabb: Option<(Vec3, Vec3)>) -> (Vec3, f32) {
        let env_min = self.envelope_min_mm();
        let env_max = self.envelope_max_mm();
        let (min, max) = match scene_aabb {
            Some((sm, sx)) => (sm.min(env_min), sx.max(env_max)),
            None => (env_min, env_max),
        };
        let centre = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(1e-3);
        (centre, radius)
    }
}

fn clamp_axis(pre_min: f32, pre_max: f32, env_min: f32, env_max: f32, d: f32) -> f32 {
    let outside_neg = pre_min < env_min;
    let outside_pos = pre_max > env_max;
    let lower = if outside_neg { 0.0 } else { env_min - pre_min };
    let upper = if outside_pos { 0.0 } else { env_max - pre_max };
    // For a box without a NaN bound, `lower <= 0 <= upper` holds, so `0` is
    // always feasible and the clamp is well-defined even when `lower == upper`.
    d.clamp(lower, upper)
}

/// The build volume that the viewport uses when it knows no printer.
pub const NO_PRINTER_BUILD_VOLUME: BuildVolume =
    match sc_print_primitives::BuildVolume::new(192.0, 120.0, 200.0) {
        Ok(volume) => BuildVolume { volume },
        Err(_) => panic!("the placeholder lengths are positive and finite"),
    };

#[cfg(test)]
mod build_volume_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    use sc_mesh_core::Vertex;

    fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb::from_min_max(Vertex::new(min), Vertex::new(max))
    }

    fn vol() -> BuildVolume {
        // 100 x 60 x 80 mm: small, asymmetric, easy to reason about.
        BuildVolume::new(100.0, 60.0, 80.0)
    }

    #[test]
    fn envelope_corners_are_centred_on_xy_with_z_floor() {
        let v = vol();
        let lo = v.envelope_min_mm();
        let hi = v.envelope_max_mm();
        assert_approx_eq!(f32, lo.x, -50.0);
        assert_approx_eq!(f32, lo.y, -30.0);
        assert_approx_eq!(f32, lo.z, 0.0);
        assert_approx_eq!(f32, hi.x, 50.0);
        assert_approx_eq!(f32, hi.y, 30.0);
        assert_approx_eq!(f32, hi.z, 80.0);
    }

    #[test]
    fn lengths_and_corners_are_read_in_f32() {
        // 143.43 and 89.6 are not exact in f32: the lengths are their f32
        // roundings, and the corners are the halves of those roundings.
        let v = BuildVolume::from(
            sc_print_primitives::BuildVolume::new(143.43, 89.6, 175.0).expect("valid build volume"),
        );
        assert_eq!(v.width_mm(), 143.43);
        assert_eq!(v.depth_mm(), 89.6);
        assert_eq!(v.height_mm(), 175.0);
        assert_eq!(v.envelope_min_mm(), Vec3::new(-71.715, -44.8, 0.0));
        assert_eq!(v.envelope_max_mm(), Vec3::new(71.715, 44.8, 175.0));
    }

    #[test]
    fn zero_delta_is_zero() {
        let v = vol();
        let pre = aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]);
        let out = v.clamp_translation_delta(pre, Vec3::ZERO);
        assert_eq!(out, Vec3::ZERO);
    }

    #[test]
    fn fully_interior_small_delta_passes_through() {
        let v = vol();
        let pre = aabb([-1.0, -1.0, 1.0], [1.0, 1.0, 3.0]);
        let delta = Vec3::new(2.0, 1.0, 0.5);
        let out = v.clamp_translation_delta(pre, delta);
        assert_approx_eq!(f32, out.x, 2.0);
        assert_approx_eq!(f32, out.y, 1.0);
        assert_approx_eq!(f32, out.z, 0.5);
    }

    #[test]
    fn delta_breaching_plus_x_wall_clamps_x_only() {
        let v = vol();
        // pre.max.x = 49, only 1 mm of room before +X wall.
        let pre = aabb([47.0, -1.0, 0.0], [49.0, 1.0, 2.0]);
        let delta = Vec3::new(5.0, 1.0, 1.0);
        let out = v.clamp_translation_delta(pre, delta);
        assert_approx_eq!(f32, out.x, 1.0);
        assert_approx_eq!(f32, out.y, 1.0);
        assert_approx_eq!(f32, out.z, 1.0);
    }

    #[test]
    fn touching_minus_x_wall_blocks_negative_x() {
        let v = vol();
        // pre.min.x = -50: flush against -X wall.
        let pre = aabb([-50.0, -1.0, 0.0], [-48.0, 1.0, 2.0]);
        let delta = Vec3::new(-3.0, 0.5, 0.5);
        let out = v.clamp_translation_delta(pre, delta);
        assert_approx_eq!(f32, out.x, 0.0);
        assert_approx_eq!(f32, out.y, 0.5);
        assert_approx_eq!(f32, out.z, 0.5);
    }

    #[test]
    fn already_outside_plus_x_blocks_further_plus_x() {
        let v = vol();
        // pre.max.x = 60 > env_max.x = 50.
        let pre = aabb([55.0, -1.0, 0.0], [60.0, 1.0, 2.0]);
        let delta = Vec3::new(2.0, 0.0, 0.0);
        let out = v.clamp_translation_delta(pre, delta);
        assert_approx_eq!(f32, out.x, 0.0);
    }

    #[test]
    fn already_outside_plus_x_allows_negative_x_back_inside() {
        let v = vol();
        // pre.max.x = 60 > 50. Allow movement back toward inside up to the
        // opposite wall: lower bound is env_min.x - pre.min.x = -50 - 55 =
        // -105. A small inward delta is preserved.
        let pre = aabb([55.0, -1.0, 0.0], [60.0, 1.0, 2.0]);
        let delta = Vec3::new(-3.0, 0.0, 0.0);
        let out = v.clamp_translation_delta(pre, delta);
        assert_approx_eq!(f32, out.x, -3.0);
    }

    #[test]
    fn z_floor_is_one_sided() {
        let v = vol();
        // Resting on the plate: pre.min.z = 0. -Z motion is blocked, +Z motion
        // is allowed up to env_max.z - pre.max.z = 80 - 5 = 75.
        let pre = aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 5.0]);
        let down = v.clamp_translation_delta(pre, Vec3::new(0.0, 0.0, -2.0));
        assert_approx_eq!(f32, down.z, 0.0);
        let up = v.clamp_translation_delta(pre, Vec3::new(0.0, 0.0, 100.0));
        assert_approx_eq!(f32, up.z, 75.0);
    }

    #[test]
    fn aabb_larger_than_envelope_clamps_to_zero_on_that_axis() {
        let v = vol();
        // pre.x = [-200, 200] is wider than envelope on X. Y/Z fit.
        let pre = aabb([-200.0, -1.0, 0.0], [200.0, 1.0, 2.0]);
        let out = v.clamp_translation_delta(pre, Vec3::new(10.0, 5.0, 5.0));
        assert_approx_eq!(f32, out.x, 0.0);
        assert_approx_eq!(f32, out.y, 5.0);
        assert_approx_eq!(f32, out.z, 5.0);
    }

    #[test]
    fn diagonal_delta_slides_along_a_wall() {
        let v = vol();
        // Touching +X wall, plenty of Y/Z room. A diagonal +X+Y push clamps X
        // to 0 and lets Y through unmodified: classic wall slide.
        let pre = aabb([48.0, -1.0, 0.0], [50.0, 1.0, 2.0]);
        let out = v.clamp_translation_delta(pre, Vec3::new(5.0, 4.0, 0.0));
        assert_approx_eq!(f32, out.x, 0.0);
        assert_approx_eq!(f32, out.y, 4.0);
        assert_approx_eq!(f32, out.z, 0.0);
    }

    #[test]
    fn opposite_walls_breached_gives_per_axis_room() {
        let v = vol();
        // Pre.x = [-50, 50]: exactly fills X. Any X delta clamps to 0; per
        // axis-clamp uses [d_lo=0, d_hi=0] = {0}.
        let pre = aabb([-50.0, -1.0, 0.0], [50.0, 1.0, 2.0]);
        let out = v.clamp_translation_delta(pre, Vec3::new(7.0, 0.0, 0.0));
        assert_approx_eq!(f32, out.x, 0.0);
    }

    #[test]
    fn contains_aabb_returns_true_for_aabb_strictly_inside() {
        let v = vol();
        let inside = aabb([-10.0, -10.0, 1.0], [10.0, 10.0, 50.0]);
        assert!(v.contains_aabb(inside));
    }

    #[test]
    fn contains_aabb_returns_true_when_aabb_touches_walls() {
        // Faces flush against every wall; still contained (contains is
        // inclusive on the boundary).
        let v = vol();
        let flush = aabb([-50.0, -30.0, 0.0], [50.0, 30.0, 80.0]);
        assert!(v.contains_aabb(flush));
    }

    #[test]
    fn contains_aabb_returns_false_when_extending_past_plus_x() {
        let v = vol();
        let past_plus_x = aabb([0.0, 0.0, 1.0], [60.0, 10.0, 10.0]);
        assert!(!v.contains_aabb(past_plus_x));
    }

    #[test]
    fn contains_aabb_returns_false_when_extending_past_minus_x() {
        let v = vol();
        let past_minus_x = aabb([-60.0, 0.0, 1.0], [0.0, 10.0, 10.0]);
        assert!(!v.contains_aabb(past_minus_x));
    }

    #[test]
    fn contains_aabb_returns_false_when_extending_below_plate() {
        let v = vol();
        let below_plate = aabb([-10.0, -10.0, -1.0], [10.0, 10.0, 10.0]);
        assert!(!v.contains_aabb(below_plate));
    }

    #[test]
    fn contains_aabb_returns_false_when_extending_above_ceiling() {
        let v = vol();
        let above_ceiling = aabb([-10.0, -10.0, 1.0], [10.0, 10.0, 100.0]);
        assert!(!v.contains_aabb(above_ceiling));
    }

    #[test]
    fn contains_aabb_returns_true_for_empty_aabb() {
        // Vacuously contained: no extent to breach a wall.
        let v = vol();
        assert!(v.contains_aabb(Aabb::empty()));
    }

    #[test]
    fn no_printer_envelope_is_finite_and_positive() {
        let v = NO_PRINTER_BUILD_VOLUME;
        let lo = v.envelope_min_mm();
        let hi = v.envelope_max_mm();
        assert!(lo.x.is_finite() && lo.y.is_finite() && lo.z.is_finite());
        assert!(hi.x > lo.x && hi.y > lo.y && hi.z > lo.z);
    }

    #[test]
    fn placeholder_is_192_by_120_by_200_mm() {
        let v = NO_PRINTER_BUILD_VOLUME;
        assert_eq!(v.width_mm(), 192.0);
        assert_eq!(v.depth_mm(), 120.0);
        assert_eq!(v.height_mm(), 200.0);
        assert_eq!(v.envelope_min_mm(), Vec3::new(-96.0, -60.0, 0.0));
        assert_eq!(v.envelope_max_mm(), Vec3::new(96.0, 60.0, 200.0));
    }

    #[test]
    fn framing_with_no_scene_returns_plate_geometry() {
        // Envelope 100 x 60 x 80: corners at (-50, -30, 0) and (50, 30, 80).
        // Centre at (0, 0, 40), bounding-sphere radius is half the diagonal =
        // sqrt(100^2 + 60^2 + 80^2) / 2.
        let v = vol();
        let (centre, radius) = v.framing(None);
        let expected_radius =
            (100.0_f32.powi(2) + 60.0_f32.powi(2) + 80.0_f32.powi(2)).sqrt() * 0.5;
        assert_approx_eq!(f32, centre.x, 0.0);
        assert_approx_eq!(f32, centre.y, 0.0);
        assert_approx_eq!(f32, centre.z, 40.0);
        assert_approx_eq!(f32, radius, expected_radius, epsilon = 1e-3);
    }

    #[test]
    fn framing_with_scene_inside_envelope_is_dominated_by_plate() {
        // A small scene AABB strictly inside the envelope must not shift the
        // framing: the union is the envelope itself.
        let v = vol();
        let scene = (Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 2.0));
        let (centre_with, radius_with) = v.framing(Some(scene));
        let (centre_alone, radius_alone) = v.framing(None);
        assert_approx_eq!(f32, centre_with.x, centre_alone.x);
        assert_approx_eq!(f32, centre_with.y, centre_alone.y);
        assert_approx_eq!(f32, centre_with.z, centre_alone.z);
        assert_approx_eq!(f32, radius_with, radius_alone);
    }

    #[test]
    fn framing_with_scene_outside_envelope_extends_bounds() {
        // Scene AABB poking out past +X / +Y. The union takes the envelope on
        // the negative side and the scene on the positive side.
        let v = vol();
        let scene = (Vec3::new(100.0, 100.0, 50.0), Vec3::new(110.0, 110.0, 60.0));
        let (centre, radius) = v.framing(Some(scene));
        // Union: (-50, -30, 0) to (110, 110, 80). Centre = (30, 40, 40).
        // Diagonal = (160, 140, 80); half-length is the radius.
        let expected_radius =
            (160.0_f32.powi(2) + 140.0_f32.powi(2) + 80.0_f32.powi(2)).sqrt() * 0.5;
        assert_approx_eq!(f32, centre.x, 30.0);
        assert_approx_eq!(f32, centre.y, 40.0);
        assert_approx_eq!(f32, centre.z, 40.0);
        assert_approx_eq!(f32, radius, expected_radius, epsilon = 1e-3);
    }
}
