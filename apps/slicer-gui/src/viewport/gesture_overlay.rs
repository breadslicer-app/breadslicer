/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The 2D aids of a gesture in flight (#69), in viewport pixels: the line
//! along the axis through the start position (a line along each of its two
//! axes for a move in a plane, #207, #209), the figure next to the dragged
//! handle (or the cursor, in G/R/S), and, while moving, a dot at the start
//! position and a dashed line to the current one. The chip at the top and the
//! rotate slice are drawn elsewhere (the chip by the controller, the slice by
//! the gizmo pass).

use glam::{Vec2, Vec3};

use crate::camera::OrbitCamera;
use crate::gizmo::axis::{Axis, axis_basis};
use crate::gizmo::dimensions::px;
use crate::gizmo::look::GizmoLook;
use crate::gizmo::style::AXIS_COLORS;
use crate::mesh_render::default_fov_y;
use crate::picking;

use super::gizmo_drag::DragOp;
use super::modal_transform::{ModalGesture, ModalOp};

/// Dashes of the move trail, in pixels.
const DASH: f32 = 6.0;
const GAP: f32 = 4.0;
/// A trail longer than this many dashes is drawn up to here.
const MAX_DASHES: usize = 400;
/// The trail shows once the object is this many pixels from its start.
const TRAIL_MIN_PIXELS: f32 = 4.0;
/// How far the axis line reaches each way from the start position.
const GUIDE_REACH: f32 = 4000.0;
/// Opacity of the axis line: present, but under the figures.
const GUIDE_ALPHA: u8 = 165;
/// The figure sits this far past the handle tip, the ring and the cube.
const TAG_PAST_ARROW_PIXELS: f32 = 16.0;
const TAG_PAST_RING_PIXELS: f32 = 22.0;
const TAG_PAST_CUBE_PIXELS: f32 = 20.0;
const TAG_PAST_PLANE_PIXELS: f32 = 12.0;
/// In G/R/S, and next to the centre ball, the figure sits this far down and
/// right of the anchor.
const TAG_CURSOR_OFFSET: f32 = 16.0;

/// What the overlay shows this frame.
#[derive(Debug, Default, PartialEq)]
pub(super) struct GestureAids {
    /// The axis line: path commands and colour (RGBA).
    pub guide: Option<(String, [u8; 4])>,
    /// The line along the second axis of a move in a plane.
    pub guide2: Option<(String, [u8; 4])>,
    /// The anchor point of the figure next to the handle; the text is the
    /// gesture's own (see `tag_text`).
    pub tag_anchor: Option<(f32, f32)>,
    /// The move trail: dashed path commands and the start dot.
    pub trail: Option<(String, (f32, f32))>,
}

/// Where the gesture is: its handle and the world positions it needs.
pub(super) enum GestureView<'a> {
    Modal {
        gesture: &'a ModalGesture,
        /// Where the object's pivot is now.
        pivot_now: Vec3,
    },
    Drag {
        op: DragOp,
        /// The pivot at press.
        pivot_start: Vec3,
        pivot_now: Vec3,
    },
}

/// Projection of world points into logical viewport pixels, and the gizmo
/// the user picked (where its handles sit and how large it is).
pub(super) struct Screen<'a> {
    pub camera: &'a OrbitCamera,
    pub width: f32,
    pub height: f32,
    pub look: GizmoLook,
}

impl Screen<'_> {
    fn project(&self, p: Vec3) -> Option<Vec2> {
        picking::project_world_to_screen(self.camera, default_fov_y(), self.width, self.height, p)
            .map(|(x, y)| Vec2::new(x, y))
    }

    /// World length of the gizmo shaft at `p` (see `gizmo::gizmo_scale`), at
    /// the gizmo's size.
    fn shaft(&self, p: Vec3) -> Option<f32> {
        crate::gizmo::gizmo_scale_sized(
            self.camera,
            p,
            default_fov_y(),
            self.height,
            self.look.size,
        )
    }
}

/// The aids of `view` on `screen`.
pub(super) fn compose(view: &GestureView<'_>, screen: &Screen<'_>) -> GestureAids {
    match view {
        GestureView::Modal { gesture, pivot_now } => {
            let guide = gesture.axis.and_then(|k| {
                let world_axis = [Vec3::X, Vec3::Y, Vec3::Z][k];
                let dir = Vec2::new(world_axis.dot(gesture.right), -world_axis.dot(gesture.up));
                let [px, py] = gesture.screen_pivot;
                guide_line(Vec2::new(px, py), dir, k)
            });
            let trail = (gesture.op == ModalOp::Move)
                .then(|| trail(screen, gesture.pivot, *pivot_now))
                .flatten();
            let [cx, cy] = gesture.cursor;
            GestureAids {
                guide,
                guide2: None,
                tag_anchor: modal_tag_anchor(gesture, *pivot_now, screen)
                    .or(Some((cx + TAG_CURSOR_OFFSET, cy + TAG_CURSOR_OFFSET))),
                trail,
            }
        }
        GestureView::Drag {
            op,
            pivot_start,
            pivot_now,
        } => {
            let (first, second) = match *op {
                DragOp::Translate { axis, .. }
                | DragOp::Rotate { axis, .. }
                | DragOp::ScaleAxis { axis, .. } => (Some(axis), None),
                DragOp::Plane { normal, .. } => {
                    let (u, v) = normal.plane_axes();
                    (Some(u), Some(v))
                }
                DragOp::ScaleUniform { .. } => (None, None),
            };
            let line = |a: Axis| {
                let shaft = screen.shaft(*pivot_start)?;
                let p0 = screen.project(*pivot_start)?;
                let p1 = screen.project(*pivot_start + a.world_dir() * shaft)?;
                guide_line(p0, p1 - p0, a.index())
            };
            let trail = matches!(op, DragOp::Translate { .. } | DragOp::Plane { .. })
                .then(|| trail(screen, *pivot_start, *pivot_now))
                .flatten();
            GestureAids {
                guide: first.and_then(line),
                guide2: second.and_then(line),
                tag_anchor: drag_tag_anchor(*op, *pivot_start, *pivot_now, screen),
                trail,
            }
        }
    }
}

/// The figure's anchor for a gizmo drag: past the arrow tip, outside the ring
/// at the current angle, past the cube, or beside the centre ball.
fn drag_tag_anchor(
    op: DragOp,
    pivot_start: Vec3,
    pivot_now: Vec3,
    screen: &Screen<'_>,
) -> Option<(f32, f32)> {
    let shaft = screen.shaft(pivot_now)?;
    let m = screen.look.metrics();
    let world = match op {
        DragOp::Translate { axis, .. } => {
            pivot_now + axis.world_dir() * shaft * (m.cone_tip + px(TAG_PAST_ARROW_PIXELS))
        }
        DragOp::Plane { normal, body, .. } => {
            if body {
                // A model dragged by its body: next to its centre, where the
                // gizmo stands.
                let p = screen.project(pivot_now)?;
                return Some((p.x + TAG_CURSOR_OFFSET, p.y + TAG_CURSOR_OFFSET));
            }
            // Past the outer corner of the square.
            let (u, v) = normal.plane_axes();
            let view_dir = (screen.camera.target - screen.camera.eye()).normalize_or_zero();
            let (su, sv) = crate::gizmo::hit_test::plane_quadrant(normal, view_dir);
            let corner = px(m.plane_offset_px + m.plane_size_px * 0.5 + TAG_PAST_PLANE_PIXELS);
            pivot_now + (u.world_dir() * su + v.world_dir() * sv) * shaft * corner
        }
        DragOp::Rotate {
            axis,
            start_angle,
            applied,
            ..
        } => {
            let (u, v) = axis_basis(axis);
            let a = start_angle + applied;
            let r = shaft * (m.ring_radius + px(TAG_PAST_RING_PIXELS));
            pivot_start + (u * a.cos() + v * a.sin()) * r
        }
        DragOp::ScaleAxis {
            axis,
            current_anchor,
            ..
        } => current_anchor + axis.world_dir() * shaft * px(TAG_PAST_CUBE_PIXELS),
        DragOp::ScaleUniform { .. } => {
            let p = screen.project(pivot_now)?;
            return Some((p.x + TAG_CURSOR_OFFSET, p.y + TAG_CURSOR_OFFSET));
        }
    };
    screen.project(world).map(|p| (p.x, p.y))
}

/// The figure's anchor for a modal G/R/S, where the matching drag puts it:
/// past the arrow tip, outside the ring at the current angle (the screen ring
/// without an axis), or past the sliding cube. `None` (next to the cursor) for
/// a free G and a uniform S.
fn modal_tag_anchor(
    gesture: &ModalGesture,
    pivot_now: Vec3,
    screen: &Screen<'_>,
) -> Option<(f32, f32)> {
    let axis = gesture.axis.map(|k| [Axis::X, Axis::Y, Axis::Z][k]);
    let m = screen.look.metrics();
    let world = match (gesture.op, axis) {
        (ModalOp::Move, Some(axis)) => {
            let shaft = screen.shaft(pivot_now)?;
            pivot_now + axis.world_dir() * shaft * (m.cone_tip + px(TAG_PAST_ARROW_PIXELS))
        }
        (ModalOp::Rotate, _) => {
            let shaft = screen.shaft(gesture.pivot)?;
            let (u, v) = gesture.ring_basis();
            let a = gesture.ring_start() + gesture.applied_turn();
            let r = shaft * (m.ring_radius + px(TAG_PAST_RING_PIXELS));
            gesture.pivot + (u * a.cos() + v * a.sin()) * r
        }
        (ModalOp::Scale, Some(axis)) => {
            let shaft = screen.shaft(gesture.pivot)?;
            let along = m.cube_at * gesture.factor + px(TAG_PAST_CUBE_PIXELS);
            gesture.pivot + axis.world_dir() * shaft * along
        }
        _ => return None,
    };
    screen.project(world).map(|p| (p.x, p.y))
}

/// The line through `through` along the screen direction `dir`, in the colour
/// of axis `k`; `None` when the axis points at the camera.
fn guide_line(through: Vec2, dir: Vec2, k: usize) -> Option<(String, [u8; 4])> {
    if dir.length() < 1e-3 || !through.is_finite() {
        return None;
    }
    let dir = dir.normalize();
    let (a, b) = (through - dir * GUIDE_REACH, through + dir * GUIDE_REACH);
    let [r, g, b2] = AXIS_COLORS[k].map(|c| (c * 255.0).round() as u8);
    Some((
        format!("M {} {} L {} {}", a.x, a.y, b.x, b.y),
        [r, g, b2, GUIDE_ALPHA],
    ))
}

/// The dashed line from `start` to `now` and the start dot, once the object
/// has moved a few pixels.
fn trail(screen: &Screen<'_>, start: Vec3, now: Vec3) -> Option<(String, (f32, f32))> {
    let a = screen.project(start)?;
    let b = screen.project(now)?;
    let commands = dashed_path(a, b)?;
    Some((commands, (a.x, a.y)))
}

/// SVG path commands for a dashed segment from `a` to `b`, or `None` when it
/// is shorter than `TRAIL_MIN_PIXELS`.
fn dashed_path(a: Vec2, b: Vec2) -> Option<String> {
    let len = a.distance(b);
    if len < TRAIL_MIN_PIXELS || !len.is_finite() {
        return None;
    }
    let dir = (b - a) / len;
    let mut out = String::new();
    let mut at = 0.0;
    let mut dashes = 0;
    while at < len && dashes < MAX_DASHES {
        let p = a + dir * at;
        let q = a + dir * (at + DASH).min(len);
        out.push_str(&format!("M {} {} L {} {} ", p.x, p.y, q.x, q.y));
        at += DASH + GAP;
        dashes += 1;
    }
    Some(out)
}

/// The figure next to the handle: the chip's text without the axis name, and
/// for a free move only the distance.
pub(super) fn tag_text(readout: &str) -> String {
    if let (Some(open), Some(close)) = (readout.find('('), readout.rfind(')')) {
        return readout[open + 1..close].to_string();
    }
    for prefix in ["X ", "Y ", "Z "] {
        if let Some(rest) = readout.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    readout.to_string()
}

/// The axis an op is locked to, if any. A move in a plane has two.
pub(super) fn drag_axis(op: DragOp) -> Option<Axis> {
    match op {
        DragOp::Translate { axis, .. }
        | DragOp::Rotate { axis, .. }
        | DragOp::ScaleAxis { axis, .. } => Some(axis),
        DragOp::Plane { .. } | DragOp::ScaleUniform { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::AxisView;

    fn front_screen(camera: &OrbitCamera) -> Screen<'_> {
        Screen {
            camera,
            width: 800.0,
            height: 600.0,
            look: GizmoLook::default(),
        }
    }

    fn front_camera() -> OrbitCamera {
        let mut camera = OrbitCamera::new(Vec3::ZERO, 10.0);
        camera.set_axis_view(AxisView::Front);
        camera
    }

    #[test]
    fn the_tag_drops_the_axis_name_and_keeps_the_figure() {
        assert_eq!(tag_text("X +3.00 mm"), "+3.00 mm");
        assert_eq!(tag_text("Z -30.0°"), "-30.0°");
        assert_eq!(tag_text("150.0 %"), "150.0 %");
        assert_eq!(
            tag_text("X +1.00  Y +2.00  Z +0.00 mm  (2.24 mm)"),
            "2.24 mm"
        );
    }

    #[test]
    fn a_dashed_path_alternates_dashes_and_gaps_to_the_end() {
        let path = dashed_path(Vec2::ZERO, Vec2::new(25.0, 0.0)).unwrap();
        // Dashes at 0-6, 10-16 and 20-25.
        assert_eq!(path.matches('M').count(), 3);
        assert!(path.contains("M 20 0 L 25 0"));
        assert!(dashed_path(Vec2::ZERO, Vec2::new(2.0, 0.0)).is_none());
        let long = dashed_path(Vec2::ZERO, Vec2::new(1e6, 0.0)).unwrap();
        assert_eq!(long.matches('M').count(), MAX_DASHES);
    }

    #[test]
    fn an_axis_drag_draws_its_axis_line_through_the_start() {
        let camera = front_camera();
        let screen = front_screen(&camera);
        let view = GestureView::Drag {
            op: DragOp::Translate {
                axis: Axis::X,
                anchor: Vec3::ZERO,
            },
            pivot_start: Vec3::ZERO,
            pivot_now: Vec3::new(3.0, 0.0, 0.0),
        };
        let aids = compose(&view, &screen);
        let (path, color) = aids.guide.expect("an axis line");
        // Seen from the front, X runs across the screen through its middle.
        let n: Vec<f32> = path
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        assert_eq!(n.len(), 4, "{path}");
        assert!(
            (n[1] - 300.0).abs() < 0.5 && (n[3] - 300.0).abs() < 0.5,
            "{path}"
        );
        assert_eq!(&color[..3], &[239, 91, 91]);
        // The object moved: a trail from the start, where the dot sits.
        let (_, dot) = aids.trail.expect("a trail");
        assert!((dot.0 - 400.0).abs() < 0.5 && (dot.1 - 300.0).abs() < 0.5);
        // The figure sits right of the arrow tip.
        let (tx, ty) = aids.tag_anchor.unwrap();
        assert!(tx > 400.0 && (ty - 300.0).abs() < 0.5);
    }

    #[test]
    fn an_axis_toward_the_camera_draws_no_line() {
        let camera = front_camera();
        let screen = front_screen(&camera);
        let view = GestureView::Drag {
            op: DragOp::Translate {
                axis: Axis::Y,
                anchor: Vec3::ZERO,
            },
            pivot_start: Vec3::ZERO,
            pivot_now: Vec3::ZERO,
        };
        let aids = compose(&view, &screen);
        assert!(aids.guide.is_none());
        assert!(aids.trail.is_none());
    }

    #[test]
    fn a_turn_puts_its_figure_outside_the_ring_at_the_current_angle() {
        let camera = front_camera();
        let screen = front_screen(&camera);
        // The Y ring faces the front camera. In the Y basis (u = Z, v = X),
        // a turn to 90° from +Z points along +X: right of the pivot.
        let view = GestureView::Drag {
            op: DragOp::Rotate {
                axis: Axis::Y,
                start_angle: 0.0,
                turn: crate::gizmo::drag_math::ScreenTurn::new((400.0, 300.0), (500.0, 300.0)),
                sign: -1.0,
                applied: std::f32::consts::FRAC_PI_2,
            },
            pivot_start: Vec3::ZERO,
            pivot_now: Vec3::ZERO,
        };
        let aids = compose(&view, &screen);
        let (tx, ty) = aids.tag_anchor.unwrap();
        let ring = crate::gizmo::dimensions::ROTATE_ARC_RADIUS_FRACTION
            * crate::gizmo::dimensions::HANDLE_PIXEL_LENGTH;
        assert!(tx - 400.0 > ring, "{tx}");
        assert!((ty - 300.0).abs() < 0.5);
        assert!(aids.trail.is_none());
    }

    #[test]
    fn a_plane_drag_draws_a_line_along_each_of_its_axes_and_a_trail() {
        let camera = front_camera();
        let screen = front_screen(&camera);
        let view = GestureView::Drag {
            op: DragOp::Plane {
                normal: Axis::Y,
                origin: Vec3::ZERO,
                anchor: Vec3::ZERO,
                body: false,
            },
            pivot_start: Vec3::ZERO,
            pivot_now: Vec3::new(2.0, 0.0, 1.0),
        };
        let aids = compose(&view, &screen);
        let (_, first) = aids.guide.expect("a line along X");
        let (_, second) = aids.guide2.expect("a line along Z");
        assert_eq!(&first[..3], &[239, 91, 91]);
        assert_eq!(&second[..3], &[79, 143, 247]);
        assert!(aids.trail.is_some());
        // The figure sits up and right, past the square.
        let (tx, ty) = aids.tag_anchor.unwrap();
        assert!(tx > 400.0 && ty < 300.0, "{tx} {ty}");
    }

    #[test]
    fn a_body_drag_puts_its_figure_by_the_model() {
        let mut camera = OrbitCamera::new(Vec3::ZERO, 10.0);
        camera.set_axis_view(AxisView::Top);
        let screen = front_screen(&camera);
        let view = GestureView::Drag {
            op: DragOp::Plane {
                normal: Axis::Z,
                origin: Vec3::ZERO,
                anchor: Vec3::ZERO,
                body: true,
            },
            pivot_start: Vec3::ZERO,
            pivot_now: Vec3::new(3.0, 0.0, 0.0),
        };
        let aids = compose(&view, &screen);
        assert!(aids.guide.is_some() && aids.guide2.is_some());
        let now = screen.project(Vec3::new(3.0, 0.0, 0.0)).unwrap();
        let (tx, ty) = aids.tag_anchor.unwrap();
        assert!((tx - now.x - TAG_CURSOR_OFFSET).abs() < 0.5);
        assert!((ty - now.y - TAG_CURSOR_OFFSET).abs() < 0.5);
    }
}
