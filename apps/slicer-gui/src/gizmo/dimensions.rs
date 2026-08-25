/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pixel-space and handle-local-space dimensions for the gizmo handles.
//! Every constant is consumed either by the renderer (`gizmo_geometry`,
//! `mesh_render`) or by the per-frame world-space conversion in
//! `gizmo::gizmo_scale` / `gizmo::hit_radius_world`.

/// Screen-pixel length of the gizmo shaft. Drives `gizmo_scale`.
/// Sized to leave room for the per-axis midpoint cube *and* the cone
/// tip without either occluding the other.
pub(crate) const HANDLE_PIXEL_LENGTH: f32 = 120.0;

/// Screen-pixel radius of the cylinder hit volume. The visual shaft
/// is thinner than this; the hit cylinder is fattened so a click
/// near (but not on) the shaft still registers.
pub(crate) const HANDLE_PIXEL_RADIUS: f32 = 6.0;

/// Length, in handle-local space, of each cone in the rotate
/// "two-cone spindle" hit target. The two cones share their bases at
/// the arc's camera-facing midpoint and point along ±tangent (i.e.
/// ±Z in handle-local space) with this length, so the tips run along
/// the arc and the base disc is perpendicular to it. Mirrors the
/// translate cone's `1.18 - 1.0 = 0.18`.
pub(crate) const ROTATE_CONE_LENGTH: f32 = 0.18;

/// Base radius, in handle-local space, of each rotate cone, same as
/// the translate cone's base radius for visual consistency.
pub(crate) const ROTATE_CONE_RADIUS: f32 = 0.07;

/// Side length, in screen pixels, of the per-axis scale cube. The
/// cube sits at the midpoint of the translate shaft (`scale * 0.5`)
/// and is small enough to leave the cone tip clearly distinct.
pub(crate) const AXIS_SCALE_CUBE_PIXELS: f32 = 10.0;

/// Fraction of the translate shaft length at which the scale-axis
/// midpoint cube sits (`pivot + axis * scale * MIDPOINT_FRACTION`).
/// Pinned at the *midpoint*, not the tip, so per-axis scale doesn't
/// collide with the translate cone.
pub(crate) const MIDPOINT_FRACTION: f32 = 0.5;

/// Major (circle) radius of the rotate arc, expressed as a fraction
/// of the translate shaft length. 0.85 keeps the arc inside the
/// translate cone tip so the two handle types don't fight for the
/// same screen pixels.
pub(crate) const ROTATE_ARC_RADIUS_FRACTION: f32 = 0.85;

/// Lower bound on the translate cone's hit cylinder, in handle-local
/// space. Below this `s` along the axis the cylinder is dormant;
/// the *shaft line* between the gizmo origin and the cone is purely
/// decorative (clicks there fall through to other handle categories
/// or the picker), the per-axis scale cube at the midpoint owns its
/// own discrete target.
pub(super) const TRANSLATE_HIT_S_MIN: f32 = 1.0;

/// Upper bound on the translate cone's hit cylinder. Extends past
/// the visual tip (`CONE_TIP_X = 1.18`) by a small forgiving margin.
pub(super) const TRANSLATE_HIT_S_MAX: f32 = 1.25;
