/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Cut tool's gizmo (#56): the handles that move the plane in the scene.
//!
//! While the tool is open the object's gizmo goes and this one comes in its
//! place, drawn by the same pass in the same style ([`ToolGizmo`]): an arrow
//! along the plane's normal (blue) moves it along it, a ring about each of
//! the plane's two directions (red and green) tilts it, a cube at each of the
//! four corners moves that corner on the plane alone, and a drag on the plane
//! moves the whole shape along it. Like the object's gizmo it keeps its size
//! on the screen, and its handles are picked within the same pixels.
//!
//! This module only knows the geometry: where the handles are and which one
//! a pointer is on. [`crate::cut_run`] turns a drag into new settings.

use std::f64::consts::TAU;

use crate::gizmo::dimensions::{HANDLE_PIXEL_RADIUS, RING_HIT_PIXELS, px};
use crate::mesh_render::{ToolGizmo, ToolHandle};

/// A pointer over the viewport, as the Cut tool sees it.
pub(crate) struct ScenePointer {
    /// The ray from the eye through the cursor, world mm.
    pub(crate) origin: [f64; 3],
    pub(crate) direction: [f64; 3],
    /// The cursor, viewport logical pixels (y down).
    pub(crate) cursor: [f64; 2],
    /// Where a world point lands on the viewport, `None` behind the eye.
    pub(crate) project: Box<dyn Fn([f64; 3]) -> Option<[f64; 2]>>,
    /// How long, world mm, the gizmo's arrow shaft is at a point: its size
    /// on the screen, as the object's gizmo is drawn.
    pub(crate) scale_at: Box<dyn Fn([f64; 3]) -> Option<f64>>,
    /// The rings' radius and where the arrow's cone tip ends, in shafts.
    pub(crate) ring_radius: f64,
    pub(crate) arrow_tip: f64,
}

impl ScenePointer {
    /// Where the ray meets the plane through `point` with normal `n`.
    pub(crate) fn on_plane(&self, point: [f64; 3], n: [f64; 3]) -> Option<[f64; 3]> {
        let along = dot(n, self.direction);
        if along.abs() < 1e-9 {
            return None;
        }
        let t = dot(n, sub(point, self.origin)) / along;
        (t >= 0.0).then(|| add(self.origin, scale(self.direction, t)))
    }

    /// The point of the line through `point` along `axis` (unit) closest to
    /// the ray, as how far along `axis` it lies. `None` when the ray runs
    /// along the line.
    pub(crate) fn along_line(&self, point: [f64; 3], axis: [f64; 3]) -> Option<f64> {
        let d = self.direction;
        let w = sub(point, self.origin);
        let b = dot(axis, d);
        let denom = 1.0 - b * b;
        if denom < 1e-6 {
            return None;
        }
        Some((b * dot(w, d) - dot(w, axis)) / denom)
    }
}

/// What the viewport offers the Cut tool while it is open, in the
/// Transform tab: the pointer over the scene, before the object's gizmo.
pub(crate) trait CutHandles {
    /// Whether the tool is open: the object's gizmo then goes.
    fn active(&self) -> bool;
    /// The gizmo to draw in the object's gizmo's place, and the turn of a
    /// ring being dragged, which the pass marks as it marks the object's.
    /// Where it stands (its centre).
    fn gizmo(&self) -> Option<([f64; 3], ToolGizmo, Option<RingTurn>)>;
    /// The pointer moved with no button down (`None`: it left the view);
    /// whether the handle under it changed.
    fn hover(&self, pointer: Option<&ScenePointer>) -> bool;
    /// A left press; `true` when it is on the tool's gizmo or plane, and
    /// the drag that follows is the tool's.
    fn press(&self, pointer: &ScenePointer) -> bool;
    /// A press the tool took moves.
    fn drag(&self, pointer: &ScenePointer);
    /// It ends: kept, with the pointer where it ended and whether it moved,
    /// or given up (`None`).
    fn release(&self, pointer: Option<&ScenePointer>, moved: bool);
    /// A click the tool did not take at the press: on a part of the cut,
    /// takes it or leaves it.
    fn click(&self, pointer: &ScenePointer) -> bool;
}

/// A ring's turn in flight, for its marks: the ring lies across `right`
/// and `up`, the turn starts at angle `start` from `right` toward `up` and
/// has gone `sweep` radians that way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RingTurn {
    pub(crate) right: [f64; 3],
    pub(crate) up: [f64; 3],
    pub(crate) start: f64,
    pub(crate) sweep: f64,
}

/// One of the gizmo's handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handle {
    /// The arrow along the normal.
    Arrow,
    /// The ring about the plane's first direction (0) or its second (1).
    Ring(usize),
    /// A corner of the shape.
    Corner(usize),
    /// The plane itself: a drag moves the shape along it.
    Plane,
}

impl Handle {
    /// The handle as the gizmo pass draws it; the plane is not one.
    pub(crate) fn drawn(self) -> Option<ToolHandle> {
        match self {
            Handle::Arrow => Some(ToolHandle::Arrow),
            Handle::Ring(k) => Some(ToolHandle::Ring(k)),
            Handle::Corner(k) => Some(ToolHandle::Corner(k)),
            Handle::Plane => None,
        }
    }
}

/// Where the gizmo stands, in the world.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Gizmo {
    /// The shape's centre: the arrow's foot and the rings' centre.
    pub(crate) center: [f64; 3],
    /// The plane's normal and its two directions.
    pub(crate) n: [f64; 3],
    pub(crate) u: [f64; 3],
    pub(crate) v: [f64; 3],
    /// The four corners of the shape on show, in order around it.
    pub(crate) corners: [[f64; 3]; 4],
}

/// How near the cursor a corner cube is picked, logical pixels: half the
/// cube and the margin the object's scale cubes have.
const CORNER_PICK_PX: f64 = 13.0;
/// Where the arrow starts grabbing, past the centre, in shafts.
const ARROW_FROM: f64 = px(16.0) as f64;
/// The arrow cannot be grabbed pointing within 8° of the eye, nor a ring
/// seen within 8° of edge on, as the object's gizmo (`gizmo::tolerances`).
const LOCKOUT_COS: f64 = 0.990_268_07;
const ROTATE_LOCKOUT_COS: f64 = 0.139_173_1;
const RING_SEGMENTS: usize = 64;

impl Gizmo {
    /// The axis ring `k` turns about.
    pub(crate) fn ring_axis(&self, k: usize) -> [f64; 3] {
        if k == 0 { self.u } else { self.v }
    }

    /// The directions ring `k` lies across, as the gizmo pass draws it: a
    /// turn about its axis runs from `right` toward `up`.
    pub(crate) fn ring_plane(&self, k: usize) -> ([f64; 3], [f64; 3]) {
        if k == 0 {
            (self.v, self.n)
        } else {
            (self.n, self.u)
        }
    }

    /// The point at angle `a` round ring `k`, of radius `r`.
    fn ring_point(&self, k: usize, a: f64, r: f64) -> [f64; 3] {
        let (right, up) = self.ring_plane(k);
        add(
            self.center,
            add(scale(right, r * a.cos()), scale(up, r * a.sin())),
        )
    }

    /// The point of ring `k` nearest the cursor on the screen, its angle
    /// round the ring, and how far from the cursor it is, pixels.
    pub(crate) fn ring_point_near(
        &self,
        k: usize,
        pointer: &ScenePointer,
    ) -> Option<([f64; 3], f64, f64)> {
        let r = (pointer.scale_at)(self.center)? * pointer.ring_radius;
        let at = pointer.cursor;
        (0..RING_SEGMENTS)
            .filter_map(|s| {
                let a = TAU * s as f64 / RING_SEGMENTS as f64;
                let p = self.ring_point(k, a, r);
                let q = (pointer.project)(p)?;
                Some((p, a, (q[0] - at[0]).hypot(q[1] - at[1])))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
    }

    /// The handle `pointer` is on: the corners first, then the arrow, the
    /// rings, and the plane last.
    pub(crate) fn pick(&self, pointer: &ScenePointer) -> Option<Handle> {
        let at = pointer.cursor;
        let corner = (0..4)
            .filter_map(|k| {
                let s = (pointer.project)(self.corners[k])?;
                let d = (s[0] - at[0]).hypot(s[1] - at[1]);
                (d <= CORNER_PICK_PX).then_some((k, d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((k, _)) = corner {
            return Some(Handle::Corner(k));
        }
        if let Some(shaft) = (pointer.scale_at)(self.center) {
            let from = add(self.center, scale(self.n, shaft * ARROW_FROM));
            let tip = add(self.center, scale(self.n, shaft * pointer.arrow_tip));
            let near = (pointer.project)(from)
                .zip((pointer.project)(tip))
                .is_some_and(|(a, b)| segment_distance(at, a, b) <= f64::from(HANDLE_PIXEL_RADIUS));
            if near && dot(self.n, pointer.direction).abs() <= LOCKOUT_COS {
                return Some(Handle::Arrow);
            }
            let ring = (0..2)
                .filter(|&k| dot(self.ring_axis(k), pointer.direction).abs() >= ROTATE_LOCKOUT_COS)
                .filter_map(|k| {
                    let (_, _, d) = self.ring_point_near(k, pointer)?;
                    (d <= f64::from(RING_HIT_PIXELS)).then_some((k, d))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((k, _)) = ring {
                return Some(Handle::Ring(k));
            }
        }
        let hit = pointer.on_plane(self.center, self.n)?;
        let flat = |p: [f64; 3]| {
            let d = sub(p, self.center);
            [dot(d, self.u), dot(d, self.v)]
        };
        let outline = self.corners.map(flat);
        inside(flat(hit), &outline).then_some(Handle::Plane)
    }

    /// The gizmo as the gizmo pass draws it.
    pub(crate) fn drawn(&self, hover: Option<Handle>, active: Option<Handle>) -> ToolGizmo {
        let v3 = |p: [f64; 3]| glam::Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
        ToolGizmo {
            axes: [v3(self.u), v3(self.v), v3(self.n)],
            corners: self.corners.map(v3),
            hover: hover.and_then(Handle::drawn),
            active: active.and_then(Handle::drawn),
        }
    }
}

/// Whether `p` lies inside the polygon `ring` (even-odd).
fn inside(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut holds = false;
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1])
        {
            holds = !holds;
        }
    }
    holds
}

/// How far `p` lies from the segment `a`–`b`, on the screen.
fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    let t = if len2 < 1e-12 {
        0.0
    } else {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
    };
    (p[0] - a[0] - d[0] * t).hypot(p[1] - a[1] - d[1] * t)
}

pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn normalize(a: [f64; 3]) -> [f64; 3] {
    let n = dot(a, a).sqrt();
    if n < 1e-15 { a } else { scale(a, 1.0 / n) }
}

/// `p` turned by `angle` radians about the line through `pivot` along
/// `axis` (unit), right-handed.
pub(crate) fn turned(p: [f64; 3], pivot: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    add(pivot, turned_dir(sub(p, pivot), axis, angle))
}

/// A direction turned by `angle` about `axis` (unit), right-handed.
pub(crate) fn turned_dir(d: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    let (s, c) = angle.sin_cos();
    add(
        add(scale(d, c), scale(cross(axis, d), s)),
        scale(axis, dot(axis, d) * (1.0 - c)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pointer whose camera puts the world point it maps to `(a, b)` by
    /// `onto` at `(400 + 10 a, 300 - 10 b)`, looking along `direction`; the
    /// shaft is 12 mm (120 px), as at 100 %.
    fn pointer(
        cursor: [f64; 2],
        direction: [f64; 3],
        onto: fn([f64; 3]) -> [f64; 2],
        origin: [f64; 3],
    ) -> ScenePointer {
        ScenePointer {
            origin,
            direction,
            cursor,
            project: Box::new(move |p| {
                let [a, b] = onto(p);
                Some([400.0 + a * 10.0, 300.0 - b * 10.0])
            }),
            scale_at: Box::new(|_| Some(12.0)),
            ring_radius: 0.85,
            arrow_tip: 1.18,
        }
    }

    fn at(cursor: [f64; 2]) -> [f64; 2] {
        [(cursor[0] - 400.0) / 10.0, (300.0 - cursor[1]) / 10.0]
    }

    /// Seen from straight above.
    fn from_above(cursor: [f64; 2]) -> ScenePointer {
        let [x, y] = at(cursor);
        pointer(cursor, [0.0, 0.0, -1.0], |p| [p[0], p[1]], [x, y, 100.0])
    }

    /// Seen from the front, looking along +y.
    fn from_front(cursor: [f64; 2]) -> ScenePointer {
        let [x, z] = at(cursor);
        pointer(cursor, [0.0, 1.0, 0.0], |p| [p[0], p[2]], [x, -100.0, z])
    }

    fn level() -> Gizmo {
        Gizmo {
            center: [0.0; 3],
            n: [0.0, 0.0, 1.0],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            corners: [
                [-20.0, -20.0, 0.0],
                [20.0, -20.0, 0.0],
                [20.0, 20.0, 0.0],
                [-20.0, 20.0, 0.0],
            ],
        }
    }

    #[test]
    fn each_handle_is_picked_where_it_shows() {
        let g = level();
        // From above: a corner a few pixels off. The arrow points at the
        // eye and both rings are edge on, as the object's gizmo's are, so
        // the middle is the plane.
        assert_eq!(
            g.pick(&from_above([600.0 + 8.0, 100.0])),
            Some(Handle::Corner(2))
        );
        assert_eq!(g.pick(&from_above([400.0, 300.0])), Some(Handle::Plane));
        assert_eq!(g.pick(&from_above([700.0, 300.0])), None);
        // From the front the arrow stands up, and the ring about y faces
        // the eye: a circle of 0.85 shafts, 102 px.
        assert_eq!(
            g.pick(&from_front([401.0, 300.0 - 80.0])),
            Some(Handle::Arrow)
        );
        assert_eq!(
            g.pick(&from_front([400.0 + 102.0, 302.0])),
            Some(Handle::Ring(1))
        );
    }

    #[test]
    fn a_turn_about_an_axis_is_right_handed() {
        let p = turned([0.0, 1.0, 0.0], [0.0; 3], [1.0, 0.0, 0.0], TAU / 4.0);
        assert!((p[2] - 1.0).abs() < 1e-12 && p[1].abs() < 1e-12, "{p:?}");
    }

    #[test]
    fn a_ring_turns_from_right_toward_up() {
        // As the pass lays the ring: handle-local +X onto the axis, +Y onto
        // `right`, +Z onto `up`; a turn about the axis moves `right` to `up`.
        let g = level();
        for k in 0..2 {
            let (right, up) = g.ring_plane(k);
            let axis = g.ring_axis(k);
            assert_eq!(cross(axis, right), up);
            let q = turned_dir(right, axis, TAU / 4.0);
            assert!((0..3).all(|i| (q[i] - up[i]).abs() < 1e-12));
        }
    }
}
