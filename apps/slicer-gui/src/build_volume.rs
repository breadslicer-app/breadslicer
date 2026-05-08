/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Build envelope of a (resin) 3d printer: the rectangular volume the
//! print head can reach, expressed in millimetres.
//!
//! The plate sits on the XY plane at `Z = 0` and is centred on the
//! world origin: `X ∈ [-W/2, W/2]`, `Y ∈ [-D/2, D/2]`. The build
//! volume extends upwards to `Z = H`. Coordinates are right-handed,
//! `+Z` away from the plate (the workspace convention).
//!
//! This module also hosts [`BuildVolume::clamp_translation_delta`], the
//! per-axis clamp the viewport applies to translate edits so an
//! object's transformed AABB cannot leave the envelope.
//!
//! For now, the envelope is a hardcoded constant
//! ([`HARDCODED_DEFAULT_BUILD_VOLUME`]). Future iterations layer an
//! [`AppSettings`](crate::app_settings::AppSettings) override on top
//! and, eventually, a per-project override; the canonical type will
//! migrate to a shared printer-profile crate when one exists.

// The translate-clamp integration that consumes the public surface
// here is staged separately; until it lands, the binary target sees
// the methods only via tests.
#![allow(dead_code)]

use glam::Vec3;
use sc_mesh_core::{Aabb, Vertex};

/// Rectangular build envelope of a printer, in millimetres.
///
/// Construct via [`BuildVolume::new`]; query the envelope corners via
/// [`BuildVolume::envelope_min_mm`] / [`BuildVolume::envelope_max_mm`]
/// or as an [`Aabb`] via [`BuildVolume::envelope_aabb`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildVolume {
    width_mm: f32,
    depth_mm: f32,
    height_mm: f32,
}

impl BuildVolume {
    /// Construct from the three side lengths. All three must be
    /// strictly positive and finite; debug-asserted on construction
    /// (release builds trust the caller, since the only producers in
    /// this iteration are compile-time constants).
    pub const fn new(width_mm: f32, depth_mm: f32, height_mm: f32) -> Self {
        debug_assert!(width_mm > 0.0 && depth_mm > 0.0 && height_mm > 0.0);
        Self {
            width_mm,
            depth_mm,
            height_mm,
        }
    }

    pub const fn width_mm(&self) -> f32 {
        self.width_mm
    }

    pub const fn depth_mm(&self) -> f32 {
        self.depth_mm
    }

    pub const fn height_mm(&self) -> f32 {
        self.height_mm
    }

    /// Envelope minimum corner: `(-W/2, -D/2, 0)`.
    pub fn envelope_min_mm(&self) -> Vec3 {
        Vec3::new(-0.5 * self.width_mm, -0.5 * self.depth_mm, 0.0)
    }

    /// Envelope maximum corner: `(W/2, D/2, H)`.
    pub fn envelope_max_mm(&self) -> Vec3 {
        Vec3::new(0.5 * self.width_mm, 0.5 * self.depth_mm, self.height_mm)
    }

    /// Envelope as an [`Aabb`]: useful when callers want to reuse
    /// `mesh-core`'s AABB primitives (union with a scene AABB, etc.).
    pub fn envelope_aabb(&self) -> Aabb {
        let lo = self.envelope_min_mm();
        let hi = self.envelope_max_mm();
        Aabb::from_min_max(
            Vertex::new([lo.x, lo.y, lo.z]),
            Vertex::new([hi.x, hi.y, hi.z]),
        )
    }

    /// Largest fraction of `delta` (per axis) that keeps `aabb_pre +
    /// delta` inside the envelope, with the additional rule that an
    /// axis already outside the envelope cannot be made worse: motion
    /// further away from the inside is blocked, motion back toward
    /// the inside is allowed up to the opposite wall.
    ///
    /// Per-axis behaviour, with `pre_min`, `pre_max` the input AABB
    /// extents on that axis, `env_min`, `env_max` the envelope
    /// extents, and `d` the proposed delta:
    ///
    /// - both extents inside: `d` clamped to
    ///   `[env_min - pre_min, env_max - pre_max]` (the range that
    ///   keeps the AABB strictly inside; contains `0`).
    /// - `pre_min < env_min` (already over the `-` wall): negative
    ///   `d` is clamped to `0`; positive `d` is clamped to
    ///   `env_max - pre_max` (still bounded by the opposite wall).
    /// - `pre_max > env_max` (already over the `+` wall): positive
    ///   `d` is clamped to `0`; negative `d` is clamped to
    ///   `env_min - pre_min`.
    /// - `pre` larger than the envelope on that axis: both rules fire
    ///   and `d` is clamped to `0`.
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
}

fn clamp_axis(pre_min: f32, pre_max: f32, env_min: f32, env_max: f32, d: f32) -> f32 {
    let outside_neg = pre_min < env_min;
    let outside_pos = pre_max > env_max;
    let lower = if outside_neg { 0.0 } else { env_min - pre_min };
    let upper = if outside_pos { 0.0 } else { env_max - pre_max };
    // `lower <= 0 <= upper` always holds, so `0` is always feasible
    // and the clamp is well-defined even when `lower == upper`.
    d.clamp(lower, upper)
}

/// Hardcoded default until a printer-profile source is wired in.
/// Roughly Mars-3-class: 192 mm x 120 mm plate, 200 mm Z.
pub const HARDCODED_DEFAULT_BUILD_VOLUME: BuildVolume = BuildVolume::new(192.0, 120.0, 200.0);

#[cfg(test)]
mod build_volume_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

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
    fn envelope_aabb_matches_corners() {
        let v = vol();
        let a = v.envelope_aabb();
        assert_eq!(a.min().coords(), [-50.0, -30.0, 0.0]);
        assert_eq!(a.max().coords(), [50.0, 30.0, 80.0]);
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
        // pre.max.x = 60 > 50. Allow movement back toward inside up
        // to the opposite wall: lower bound is env_min.x - pre.min.x
        // = -50 - 55 = -105. A small inward delta is preserved.
        let pre = aabb([55.0, -1.0, 0.0], [60.0, 1.0, 2.0]);
        let delta = Vec3::new(-3.0, 0.0, 0.0);
        let out = v.clamp_translation_delta(pre, delta);
        assert_approx_eq!(f32, out.x, -3.0);
    }

    #[test]
    fn z_floor_is_one_sided() {
        let v = vol();
        // Resting on the plate: pre.min.z = 0. -Z motion is blocked,
        // +Z motion is allowed up to env_max.z - pre.max.z = 80 - 5
        // = 75.
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
        // Touching +X wall, plenty of Y/Z room. A diagonal +X+Y push
        // clamps X to 0 and lets Y through unmodified: classic wall
        // slide.
        let pre = aabb([48.0, -1.0, 0.0], [50.0, 1.0, 2.0]);
        let out = v.clamp_translation_delta(pre, Vec3::new(5.0, 4.0, 0.0));
        assert_approx_eq!(f32, out.x, 0.0);
        assert_approx_eq!(f32, out.y, 4.0);
        assert_approx_eq!(f32, out.z, 0.0);
    }

    #[test]
    fn opposite_walls_breached_gives_per_axis_room() {
        let v = vol();
        // Pre.x = [-50, 50]: exactly fills X. Any X delta clamps to
        // 0; per axis-clamp uses [d_lo=0, d_hi=0] = {0}.
        let pre = aabb([-50.0, -1.0, 0.0], [50.0, 1.0, 2.0]);
        let out = v.clamp_translation_delta(pre, Vec3::new(7.0, 0.0, 0.0));
        assert_approx_eq!(f32, out.x, 0.0);
    }

    #[test]
    fn hardcoded_default_is_finite_and_positive() {
        let v = HARDCODED_DEFAULT_BUILD_VOLUME;
        assert!(v.width_mm() > 0.0 && v.width_mm().is_finite());
        assert!(v.depth_mm() > 0.0 && v.depth_mm().is_finite());
        assert!(v.height_mm() > 0.0 && v.height_mm().is_finite());
    }
}
