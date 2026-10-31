/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The look of the gizmo, style B "Outline" (#208): flat handles with a dark
//! outline, all together. Colours, alphas and the fades of the handles that
//! face the camera. Sizes on screen live in `dimensions`.

use glam::Vec3;

use super::axis::Axis;

/// X, Y and Z, tuned to the app theme: the colour is read against the dark
/// outline (5 to 9 : 1), not against the model.
pub(crate) const AXIS_COLORS: [[f32; 3]; 3] = [
    rgb(0xef, 0x5b, 0x5b),
    rgb(0x6c, 0xc4, 0x6a),
    rgb(0x4f, 0x8f, 0xf7),
];

/// The uniform-scale ball at the centre.
pub(crate) const CENTRE_COLOR: [f32; 3] = rgb(0xdf, 0xe3, 0xe6);

/// The outline drawn around every handle (`#0e1011` at 88 %), so a handle
/// reads over the amber selected model and over the dark bed alike.
pub(crate) const OUTLINE_COLOR: [f32; 3] = rgb(0x0e, 0x10, 0x11);
pub(crate) const OUTLINE_ALPHA: f32 = 0.88;

/// How far the handle under the cursor lightens toward white. Never toward
/// amber: amber already means "selected" and "warning".
pub(crate) const HOVER_MIX: f32 = 0.55;
/// How far the handle being dragged lightens toward white.
pub(crate) const ACTIVE_MIX: f32 = 0.25;

/// The back half of a rotate ring, so the rings read as a sphere.
pub(crate) const RING_BACK_ALPHA: f32 = 0.18;
/// A ring seen edge-on: it cannot be grabbed and fades to this.
pub(crate) const RING_EDGE_ALPHA: f32 = 0.35;
/// While moving, the other arrows stay as faint references.
pub(crate) const REFERENCE_ALPHA: f32 = 0.22;

/// The swept slice while rotating, and its ticks and spokes.
pub(crate) const WEDGE_ALPHA: f32 = 0.28;
pub(crate) const TICK_MIX: f32 = 0.3;
pub(crate) const TICK_ALPHA: f32 = 0.9;

/// An arrow or a scale cube fades out as its axis turns toward the camera,
/// between these `|axis · view|` values.
pub(crate) const AXIS_FADE_START: f32 = 0.95;
pub(crate) const AXIS_FADE_END: f32 = 0.99;

const fn rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

pub(crate) fn axis_color(axis: Axis) -> [f32; 3] {
    AXIS_COLORS[axis.index()]
}

/// `color` moved `amount` of the way toward white.
pub(crate) fn lighten(color: [f32; 3], amount: f32) -> [f32; 3] {
    color.map(|c| c + (1.0 - c) * amount)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Opacity of an arrow or a scale cube along `axis` seen along `view_dir`
/// (unit): 1 across the screen, 0 pointing at the camera.
pub(crate) fn axis_fade(axis: Axis, view_dir: Vec3) -> f32 {
    let along = axis.world_dir().dot(view_dir).abs();
    1.0 - smoothstep(AXIS_FADE_START, AXIS_FADE_END, along)
}

/// Opacity of the rotate ring around `axis`: faded once the ring is edge-on
/// and can no longer be grabbed.
pub(crate) fn ring_fade(axis: Axis, view_dir: Vec3) -> f32 {
    let along = axis.world_dir().dot(view_dir).abs();
    if along < super::tolerances::ROTATE_LOCKOUT_COS {
        RING_EDGE_ALPHA
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn palette_matches_the_direction_comment() {
        assert_eq!(
            AXIS_COLORS[0].map(|c| (c * 255.0).round() as u8),
            [239, 91, 91]
        );
        assert_eq!(
            AXIS_COLORS[1].map(|c| (c * 255.0).round() as u8),
            [108, 196, 106]
        );
        assert_eq!(
            AXIS_COLORS[2].map(|c| (c * 255.0).round() as u8),
            [79, 143, 247]
        );
    }

    #[test]
    fn lighten_moves_toward_white_and_keeps_white() {
        let c = lighten([0.2, 0.4, 1.0], 0.5);
        assert_approx_eq!(f32, c[0], 0.6);
        assert_approx_eq!(f32, c[1], 0.7);
        assert_approx_eq!(f32, c[2], 1.0);
    }

    #[test]
    fn hover_never_tints_toward_amber() {
        // Lightening keeps the channel order of each axis colour, so no axis
        // turns into the amber of the selected model (red > green > blue).
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            let base = axis_color(axis);
            let hover = lighten(base, HOVER_MIX);
            for i in 0..3 {
                for j in 0..3 {
                    if base[i] > base[j] {
                        assert!(hover[i] >= hover[j]);
                    }
                }
            }
        }
    }

    #[test]
    fn axis_fade_runs_from_across_to_toward_the_camera() {
        assert_approx_eq!(f32, axis_fade(Axis::X, Vec3::Z), 1.0);
        assert_approx_eq!(f32, axis_fade(Axis::X, Vec3::X), 0.0);
        assert_approx_eq!(f32, axis_fade(Axis::X, -Vec3::X), 0.0);
        let mid = Vec3::new(0.97, (1.0f32 - 0.97 * 0.97).sqrt(), 0.0);
        let f = axis_fade(Axis::X, mid);
        assert!(f > 0.2 && f < 0.8, "{f}");
    }

    #[test]
    fn an_edge_on_ring_fades_and_a_facing_ring_does_not() {
        assert_approx_eq!(f32, ring_fade(Axis::Z, Vec3::Z), 1.0);
        assert_approx_eq!(f32, ring_fade(Axis::Z, Vec3::X), RING_EDGE_ALPHA);
    }
}
