/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Pixel-space and handle-local-space dimensions for the gizmo handles. Every
//! constant is consumed either by the renderer (`gizmo_geometry`,
//! `mesh_render`) or by the per-frame world-space conversion in
//! `gizmo::gizmo_scale` / `gizmo::hit_radius_world`.
//!
//! Handle-local space puts the pivot at the origin with 1 unit = the shaft
//! length, `HANDLE_PIXEL_LENGTH` pixels on screen; [`px`] converts a pixel
//! size into it.

/// Screen-pixel length of the gizmo shaft. Drives `gizmo_scale`. Sized to leave
/// room for the per-axis scale cube *and* the cone tip without either
/// occluding the other.
pub const HANDLE_PIXEL_LENGTH: f32 = 120.0;

/// Converts a size in screen pixels (at the pivot) into handle-local units.
pub const fn px(pixels: f32) -> f32 {
    pixels / HANDLE_PIXEL_LENGTH
}

/// Screen-pixel radius of the move-arrow hit cylinder. The drawn shaft is
/// thinner than this; the hit cylinder is fattened so a click near (but not
/// on) the shaft still registers.
pub const HANDLE_PIXEL_RADIUS: f32 = 8.0;

/// Width of the dark outline around every handle, in pixels.
pub(crate) const OUTLINE_PIXELS: f32 = 1.6;

/// Radius of the move shaft (a 3 px line) and where it starts, clear of the
/// centre ball.
pub(crate) const SHAFT_RADIUS_PIXELS: f32 = 1.5;
pub(crate) const SHAFT_START_PIXELS: f32 = 10.0;

/// Handle-local x of the cone base (the end of the shaft) and of its tip.
pub(crate) const CONE_BASE_X: f32 = 1.0;
pub(crate) const CONE_TIP_X: f32 = 1.18;
/// Base radius of the move cone.
pub(crate) const CONE_RADIUS_PIXELS: f32 = 6.5;

/// Tube radius of a rotate ring (a 3 px line).
pub(crate) const RING_TUBE_PIXELS: f32 = 1.5;

/// How far from a rotate ring, in pixels, a press still grabs it.
pub const RING_HIT_PIXELS: f32 = 7.5;

/// Side length, in screen pixels, of the per-axis scale cube.
pub(crate) const AXIS_SCALE_CUBE_PIXELS: f32 = 10.0;

/// How much larger than the drawn cube its hit box is, in pixels.
pub(crate) const AXIS_SCALE_CUBE_HIT_MARGIN_PIXELS: f32 = 8.0;

/// Fraction of the shaft length at which the scale cube sits
/// (`pivot + axis * scale * MIDPOINT_FRACTION`), clear of the cone.
pub const MIDPOINT_FRACTION: f32 = 0.6;

/// Radius of the uniform-scale ball at the centre, and of its hit sphere.
pub(crate) const CENTRE_BALL_PIXELS: f32 = 6.0;
pub(crate) const CENTRE_BALL_HIT_PIXELS: f32 = 10.0;

/// Major (circle) radius of the rotate ring, expressed as a fraction of the
/// translate shaft length. 0.85 keeps the ring inside the cone tips so the two
/// handle types don't fight for the same screen pixels.
pub const ROTATE_ARC_RADIUS_FRACTION: f32 = 0.85;

/// Lower bound of the move arrow's hit cylinder, in handle-local space: the
/// whole shaft grabs, from just past the centre ball.
pub(super) const TRANSLATE_HIT_S_MIN: f32 = px(16.0);

/// Upper bound on the move arrow's hit cylinder. Extends past the visual
/// tip (`CONE_TIP_X = 1.18`) by a small forgiving margin.
pub(super) const TRANSLATE_HIT_S_MAX: f32 = 1.25;
