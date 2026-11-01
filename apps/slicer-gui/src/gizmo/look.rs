/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Which gizmo the user chose in Preferences: the style (#274), the size
//! (#276) and, for the focused style, the tool the left rail picked. The sizes
//! of each style's handles live in [`Metrics`]; the colours in `style`.

use serde::{Deserialize, Serialize};

use super::dimensions::{
    AXIS_SCALE_CUBE_HIT_MARGIN_PIXELS, AXIS_SCALE_CUBE_PIXELS, CENTRE_BALL_HIT_PIXELS,
    CENTRE_BALL_PIXELS, CONE_RADIUS_PIXELS, CONE_TIP_X, HANDLE_PIXEL_LENGTH, MIDPOINT_FRACTION,
    OUTLINE_PIXELS, RING_TUBE_PIXELS, ROTATE_ARC_RADIUS_FRACTION, SHAFT_RADIUS_PIXELS,
};

/// The gizmo styles of Preferences > UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GizmoStyle {
    /// Style B: every handle at once, flat with a dark outline (#208).
    #[default]
    Outline,
    /// Style C: the same drawing a bit thicker, one tool's handles at a time,
    /// picked on the left rail (#274).
    Focused,
}

impl GizmoStyle {
    /// Position in the Preferences combo.
    pub(crate) fn combo_index(self) -> i32 {
        match self {
            GizmoStyle::Outline => 0,
            GizmoStyle::Focused => 1,
        }
    }

    pub(crate) fn from_combo_index(index: i32) -> Self {
        if index == 1 {
            GizmoStyle::Focused
        } else {
            GizmoStyle::Outline
        }
    }
}

/// The tool of the left rail (Move, Rotate, Scale). The focused style shows
/// only its handles; the outline style shows every handle whatever the tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GizmoTool {
    #[default]
    Move,
    Rotate,
    Scale,
}

impl GizmoTool {
    /// Position on the rail, as the UI shows it.
    pub(crate) fn index(self) -> i32 {
        match self {
            GizmoTool::Move => 0,
            GizmoTool::Rotate => 1,
            GizmoTool::Scale => 2,
        }
    }
}

/// The gizmo size choices of Preferences > UI, in percent.
pub(crate) const GIZMO_SIZES: [u32; 6] = [75, 90, 100, 110, 125, 150];
pub(crate) const DEFAULT_GIZMO_SIZE: u32 = 100;

/// `percent` held to the range Preferences offers.
pub(crate) fn clamp_size(percent: u32) -> u32 {
    percent.clamp(GIZMO_SIZES[0], GIZMO_SIZES[GIZMO_SIZES.len() - 1])
}

/// Position of `percent` in the Preferences combo: the nearest choice.
pub(crate) fn size_combo_index(percent: u32) -> i32 {
    GIZMO_SIZES
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| s.abs_diff(percent))
        .map(|(i, _)| i as i32)
        .unwrap_or(2)
}

pub(crate) fn size_from_combo_index(index: i32) -> u32 {
    usize::try_from(index)
        .ok()
        .and_then(|i| GIZMO_SIZES.get(i).copied())
        .unwrap_or(DEFAULT_GIZMO_SIZE)
}

/// A family of handles: what a tool of the focused style shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    Arrows,
    Planes,
    Rings,
    Cubes,
    Ball,
}

/// The sizes of one style's handles, in screen pixels at 100 % (or in
/// handle-local units, where 1 is the shaft length, for the fields so named).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    pub shaft_radius_px: f32,
    pub cone_radius_px: f32,
    /// Handle-local x of the cone tip.
    pub cone_tip: f32,
    /// Handle-local radius of the rotate rings.
    pub ring_radius: f32,
    pub ring_tube_px: f32,
    pub outline_px: f32,
    pub cube_px: f32,
    pub cube_hit_margin_px: f32,
    /// Handle-local x of the scale cube's centre.
    pub cube_at: f32,
    /// Radius of the thin shaft that carries a scale cube when no arrow
    /// does; 0 draws none.
    pub stalk_radius_px: f32,
    /// Where the centre of a plane square sits along each of its two axes,
    /// its side, and the radius of its corners.
    pub plane_offset_px: f32,
    pub plane_size_px: f32,
    pub plane_round_px: f32,
    /// Fill opacity of a plane square at rest.
    pub plane_alpha: f32,
    pub ball_px: f32,
    pub ball_hit_px: f32,
}

/// Style B "Outline", the default.
pub(crate) const OUTLINE_METRICS: Metrics = Metrics {
    shaft_radius_px: SHAFT_RADIUS_PIXELS,
    cone_radius_px: CONE_RADIUS_PIXELS,
    cone_tip: CONE_TIP_X,
    ring_radius: ROTATE_ARC_RADIUS_FRACTION,
    ring_tube_px: RING_TUBE_PIXELS,
    outline_px: OUTLINE_PIXELS,
    cube_px: AXIS_SCALE_CUBE_PIXELS,
    cube_hit_margin_px: AXIS_SCALE_CUBE_HIT_MARGIN_PIXELS,
    cube_at: MIDPOINT_FRACTION,
    stalk_radius_px: 0.0,
    plane_offset_px: 28.0,
    plane_size_px: 19.0,
    plane_round_px: 0.0,
    plane_alpha: 0.42,
    ball_px: CENTRE_BALL_PIXELS,
    ball_hit_px: CENTRE_BALL_HIT_PIXELS,
};

/// Style C "Focused": the drawing of B, thicker, with larger rings, the scale
/// cubes at the tips and a larger centre ball (the R7 mockup).
pub(crate) const FOCUSED_METRICS: Metrics = Metrics {
    shaft_radius_px: 2.2,
    cone_radius_px: 8.5,
    cone_tip: 1.0 + 26.0 / HANDLE_PIXEL_LENGTH,
    ring_radius: 108.0 / HANDLE_PIXEL_LENGTH,
    ring_tube_px: 2.3,
    outline_px: 1.8,
    cube_px: 14.0,
    cube_hit_margin_px: 10.0,
    cube_at: 1.0,
    stalk_radius_px: 1.3,
    plane_offset_px: 36.0,
    plane_size_px: 25.0,
    plane_round_px: 5.0,
    plane_alpha: 0.45,
    ball_px: 9.0,
    ball_hit_px: 15.0,
};

impl Metrics {
    pub(crate) fn of(style: GizmoStyle) -> &'static Metrics {
        match style {
            GizmoStyle::Outline => &OUTLINE_METRICS,
            GizmoStyle::Focused => &FOCUSED_METRICS,
        }
    }
}

/// The gizmo as the user set it up: style, size and the rail's tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoLook {
    pub style: GizmoStyle,
    /// 1 at 100 %.
    pub size: f32,
    pub tool: GizmoTool,
}

impl Default for GizmoLook {
    fn default() -> Self {
        Self {
            style: GizmoStyle::Outline,
            size: 1.0,
            tool: GizmoTool::Move,
        }
    }
}

impl GizmoLook {
    pub(crate) fn metrics(&self) -> &'static Metrics {
        Metrics::of(self.style)
    }

    /// Whether the handles of `family` show and can be grabbed.
    pub(crate) fn shows(&self, family: Family) -> bool {
        match self.style {
            GizmoStyle::Outline => true,
            GizmoStyle::Focused => matches!(
                (self.tool, family),
                (GizmoTool::Move, Family::Arrows | Family::Planes)
                    | (GizmoTool::Rotate, Family::Rings)
                    | (GizmoTool::Scale, Family::Cubes | Family::Ball)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outline_style_shows_every_family_whatever_the_tool() {
        for tool in [GizmoTool::Move, GizmoTool::Rotate, GizmoTool::Scale] {
            let look = GizmoLook {
                tool,
                ..GizmoLook::default()
            };
            for family in [
                Family::Arrows,
                Family::Planes,
                Family::Rings,
                Family::Cubes,
                Family::Ball,
            ] {
                assert!(look.shows(family), "{tool:?} {family:?}");
            }
        }
    }

    #[test]
    fn the_focused_style_shows_the_tool_families_only() {
        let look = |tool| GizmoLook {
            style: GizmoStyle::Focused,
            size: 1.0,
            tool,
        };
        let m = look(GizmoTool::Move);
        assert!(m.shows(Family::Arrows) && m.shows(Family::Planes));
        assert!(!m.shows(Family::Rings) && !m.shows(Family::Cubes) && !m.shows(Family::Ball));
        let r = look(GizmoTool::Rotate);
        assert!(r.shows(Family::Rings) && !r.shows(Family::Arrows));
        let s = look(GizmoTool::Scale);
        assert!(s.shows(Family::Cubes) && s.shows(Family::Ball) && !s.shows(Family::Planes));
    }

    #[test]
    fn the_focused_style_is_thicker_with_larger_rings() {
        let (b, c) = (OUTLINE_METRICS, FOCUSED_METRICS);
        assert!(c.shaft_radius_px > b.shaft_radius_px);
        assert!(c.ring_tube_px > b.ring_tube_px);
        assert!(c.outline_px > b.outline_px);
        assert!(c.ring_radius > b.ring_radius);
        assert!((c.shaft_radius_px * 2.0 - 4.4).abs() < 1e-6);
        assert!((c.ring_tube_px * 2.0 - 4.6).abs() < 1e-6);
    }

    #[test]
    fn sizes_map_to_the_combo_and_back() {
        for (i, &s) in GIZMO_SIZES.iter().enumerate() {
            assert_eq!(size_combo_index(s), i as i32);
            assert_eq!(size_from_combo_index(i as i32), s);
        }
        assert_eq!(size_from_combo_index(-1), DEFAULT_GIZMO_SIZE);
        assert_eq!(clamp_size(10), 75);
        assert_eq!(clamp_size(400), 150);
        assert_eq!(size_combo_index(DEFAULT_GIZMO_SIZE), 2);
    }
}
