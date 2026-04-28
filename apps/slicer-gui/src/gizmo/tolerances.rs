/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Numerical thresholds that gate gizmo interaction: lock-out
//! cosines for the translate / rotate hit tests, and the scale-factor
//! clamp.

/// Cosine of the lock-out angle: when
/// `|axis_world · ray_dir| > LOCKOUT_COS`, the angle between the
/// camera ray and the axis is below 8° and the closest-point math
/// becomes unstable — `1 − (axis · ray_dir)²` drops below `sin²(8°)`
/// and a one-pixel cursor jitter would translate the object far more
/// than the user expects. Such an axis is dropped from the hit test
/// and rendered greyed so the user sees why it is unresponsive.
///
/// `cos(8°) ≈ 0.99027`.
pub(super) const LOCKOUT_COS: f32 = 0.990_268_07;

/// Cosine of the rotate-arc lock-out's complementary threshold:
/// when `|axis_world · ray_dir| < ROTATE_LOCKOUT_COS`, the camera ray
/// is within 8° of the arc's plane. The ray-vs-arc-plane intersection
/// then becomes ill-defined and the angular position around the axis
/// jumps under one-pixel cursor jitter. Threshold magnitude matches
/// `LOCKOUT_COS` (8° margin); the geometric configuration is the
/// complementary one (axis perpendicular to ray instead of parallel).
///
/// `cos(82°) ≈ 0.13917`.
pub(super) const ROTATE_LOCKOUT_COS: f32 = 0.139_173_1;

/// Lower bound on a single-axis or uniform scale factor. Factors
/// below this are clamped up; factors above `1.0 / EPS_SCALE` are
/// clamped down. Mirroring (negative scale) is disallowed through
/// the gizmo, so the bound also rejects sign flips. The user sees
/// the gizmo "stick" at the limit.
pub(super) const EPS_SCALE: f32 = 1e-3;
