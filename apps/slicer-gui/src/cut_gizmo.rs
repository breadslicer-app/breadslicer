/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Cut tool's gizmo (#56): the handles that move the plane in the scene.
//!
//! While the tool is open the object's gizmo goes and these come in its
//! place, around the plane: an arrow along the plane's normal (blue) moves it
//! up or down along it, a ring about each of the plane's two directions (red
//! and green) tilts it, a light square at each of the four corners moves that
//! corner on the plane alone, and a drag on the plane moves the whole shape
//! along it. Handles are picked on the screen, a few pixels around what is
//! drawn, so a small one is as easy to grab as a big one.
//!
//! This module only knows the geometry: where the handles are, which one a
//! pointer is on, and the meshes that draw them. [`crate::cut_run`] turns a
//! drag into new settings.

use std::f64::consts::TAU;

use crate::mesh_render::{OverlayMesh, OverlayTint};

/// A pointer over the viewport, as the Cut tool sees it.
pub(crate) struct ScenePointer {
    /// The ray from the eye through the cursor, world mm.
    pub(crate) origin: [f64; 3],
    pub(crate) direction: [f64; 3],
    /// The cursor, viewport logical pixels (y down).
    pub(crate) cursor: [f64; 2],
    /// Where a world point lands on the viewport, `None` behind the eye.
    pub(crate) project: Box<dyn Fn([f64; 3]) -> Option<[f64; 2]>>,
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
    /// The pointer moved with no button down (`None`: it left the view).
    fn hover(&self, pointer: Option<&ScenePointer>);
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
    /// The size handles are drawn at, mm: the model's reach.
    pub(crate) reach: f64,
}

/// How near the cursor a handle is picked, logical pixels.
const CORNER_PICK_PX: f64 = 11.0;
const ARROW_PICK_PX: f64 = 8.0;
const RING_PICK_PX: f64 = 7.0;
const RING_SEGMENTS: usize = 64;

impl Gizmo {
    /// The width handles are drawn at, mm.
    fn unit(&self) -> f64 {
        (self.reach * 0.016).clamp(0.25, 3.0)
    }

    fn arrow_tip(&self) -> [f64; 3] {
        add(self.center, scale(self.n, self.reach * 0.38))
    }

    pub(crate) fn ring_radius(&self) -> f64 {
        self.reach * 0.28
    }

    /// The axis ring `k` turns about.
    pub(crate) fn ring_axis(&self, k: usize) -> [f64; 3] {
        if k == 0 { self.u } else { self.v }
    }

    /// The point at angle `a` round ring `k`.
    fn ring_point(&self, k: usize, a: f64) -> [f64; 3] {
        // The ring lies across its axis: in the plane of the normal and the
        // other direction.
        let side = if k == 0 { self.v } else { self.u };
        let r = self.ring_radius();
        add(
            self.center,
            add(scale(side, r * a.cos()), scale(self.n, r * a.sin())),
        )
    }

    /// The point of ring `k` nearest the cursor on the screen, and how far
    /// from it, pixels.
    pub(crate) fn ring_point_near(
        &self,
        k: usize,
        pointer: &ScenePointer,
    ) -> Option<([f64; 3], f64)> {
        let at = pointer.cursor;
        (0..RING_SEGMENTS)
            .filter_map(|s| {
                let p = self.ring_point(k, TAU * s as f64 / RING_SEGMENTS as f64);
                let q = (pointer.project)(p)?;
                Some((p, (q[0] - at[0]).hypot(q[1] - at[1])))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The handle `pointer` is on, nearest first among the handles, the
    /// plane last.
    pub(crate) fn pick(&self, pointer: &ScenePointer) -> Option<Handle> {
        let at = pointer.cursor;
        let near = |p: [f64; 3], px: f64| {
            (pointer.project)(p)
                .map(|s| (s[0] - at[0]).hypot(s[1] - at[1]))
                .filter(|&d| d <= px)
        };
        let corner = (0..4)
            .filter_map(|k| near(self.corners[k], CORNER_PICK_PX).map(|d| (k, d)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((k, _)) = corner {
            return Some(Handle::Corner(k));
        }
        let segment_near = |a: [f64; 3], b: [f64; 3], px: f64| -> Option<f64> {
            let (sa, sb) = ((pointer.project)(a)?, (pointer.project)(b)?);
            let d = segment_distance(at, sa, sb);
            (d <= px).then_some(d)
        };
        if segment_near(self.center, self.arrow_tip(), ARROW_PICK_PX).is_some() {
            return Some(Handle::Arrow);
        }
        let ring = (0..2)
            .filter_map(|k| {
                (0..RING_SEGMENTS)
                    .filter_map(|s| {
                        let a = TAU * s as f64 / RING_SEGMENTS as f64;
                        let b = TAU * (s + 1) as f64 / RING_SEGMENTS as f64;
                        segment_near(self.ring_point(k, a), self.ring_point(k, b), RING_PICK_PX)
                    })
                    .min_by(f64::total_cmp)
                    .map(|d| (k, d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((k, _)) = ring {
            return Some(Handle::Ring(k));
        }
        let hit = pointer.on_plane(self.center, self.n)?;
        let flat = |p: [f64; 3]| {
            let d = sub(p, self.center);
            [dot(d, self.u), dot(d, self.v)]
        };
        let outline = self.corners.map(flat);
        inside(flat(hit), &outline).then_some(Handle::Plane)
    }

    /// The meshes that draw the handles, `hot` the one under the cursor or
    /// being dragged.
    pub(crate) fn meshes(&self, hot: Option<Handle>) -> Vec<OverlayMesh> {
        let tint = |handle: Handle, cold: OverlayTint| {
            if hot == Some(handle) {
                OverlayTint::HandleHot
            } else {
                cold
            }
        };
        let w = self.unit();
        let mut out = Vec::new();
        // The shape's edges, thin, so its outline reads over the model.
        let mut edges = Builder::default();
        for k in 0..4 {
            edges.rod(self.corners[k], self.corners[(k + 1) % 4], w * 0.18, 4);
        }
        out.push(edges.mesh(tint(Handle::Plane, OverlayTint::Handle)));
        for (k, ring_tint) in [OverlayTint::AxisX, OverlayTint::AxisY]
            .into_iter()
            .enumerate()
        {
            let mut ring = Builder::default();
            for s in 0..RING_SEGMENTS {
                let a = TAU * s as f64 / RING_SEGMENTS as f64;
                let b = TAU * (s + 1) as f64 / RING_SEGMENTS as f64;
                ring.rod(self.ring_point(k, a), self.ring_point(k, b), w * 0.28, 6);
            }
            out.push(ring.mesh(tint(Handle::Ring(k), ring_tint)));
        }
        let mut arrow = Builder::default();
        let tip = self.arrow_tip();
        let neck = sub(tip, scale(self.n, w * 3.2));
        arrow.rod(self.center, neck, w * 0.32, 10);
        arrow.cone(neck, tip, w * 1.05, 16);
        out.push(arrow.mesh(tint(Handle::Arrow, OverlayTint::AxisZ)));
        for k in 0..4 {
            let mut corner = Builder::default();
            corner.cube(self.corners[k], [self.u, self.v, self.n], w * 0.95);
            out.push(corner.mesh(tint(Handle::Corner(k), OverlayTint::Handle)));
        }
        out
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

/// Builds a handle's triangles.
#[derive(Default)]
struct Builder {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl Builder {
    fn mesh(self, tint: OverlayTint) -> OverlayMesh {
        OverlayMesh {
            positions: self.positions,
            indices: self.indices,
            tint,
        }
    }

    fn point(&mut self, p: [f64; 3]) -> u32 {
        self.positions.push(p.map(|c| c as f32));
        (self.positions.len() - 1) as u32
    }

    /// A tube of `sides` from `a` to `b`, open at the ends.
    fn rod(&mut self, a: [f64; 3], b: [f64; 3], radius: f64, sides: usize) {
        let axis = sub(b, a);
        let len = norm(axis);
        if len < 1e-9 {
            return;
        }
        let (x, y) = across(scale(axis, 1.0 / len));
        let base = self.positions.len() as u32;
        for s in 0..sides {
            let t = TAU * s as f64 / sides as f64;
            let off = add(scale(x, radius * t.cos()), scale(y, radius * t.sin()));
            self.point(add(a, off));
            self.point(add(b, off));
        }
        for s in 0..sides as u32 {
            let (i, j) = (base + 2 * s, base + 2 * ((s + 1) % sides as u32));
            self.indices
                .extend_from_slice(&[i, j, i + 1, i + 1, j, j + 1]);
        }
    }

    /// A cone standing on a disc at `base` with its point at `tip`.
    fn cone(&mut self, base: [f64; 3], tip: [f64; 3], radius: f64, sides: usize) {
        let axis = sub(tip, base);
        let len = norm(axis);
        if len < 1e-9 {
            return;
        }
        let (x, y) = across(scale(axis, 1.0 / len));
        let top = self.point(tip);
        let middle = self.point(base);
        let first = self.positions.len() as u32;
        for s in 0..sides {
            let t = TAU * s as f64 / sides as f64;
            self.point(add(
                base,
                add(scale(x, radius * t.cos()), scale(y, radius * t.sin())),
            ));
        }
        for s in 0..sides as u32 {
            let (i, j) = (first + s, first + (s + 1) % sides as u32);
            self.indices.extend_from_slice(&[i, j, top, j, i, middle]);
        }
    }

    /// A cube of half side `half` round `c`, along `axes`.
    fn cube(&mut self, c: [f64; 3], axes: [[f64; 3]; 3], half: f64) {
        let base = self.positions.len() as u32;
        for k in 0..8 {
            let s = |bit: usize| if k & (1 << bit) != 0 { half } else { -half };
            self.point(add(
                c,
                add(
                    scale(axes[0], s(0)),
                    add(scale(axes[1], s(1)), scale(axes[2], s(2))),
                ),
            ));
        }
        const FACES: [[u32; 4]; 6] = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        for f in FACES {
            let [a, b, c, d] = f.map(|i| base + i);
            self.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
}

/// Two unit directions across `axis` (unit), at right angles.
fn across(axis: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let pick = if axis[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let x = normalize(cross(axis, pick));
    (x, cross(axis, x))
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

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

pub(crate) fn normalize(a: [f64; 3]) -> [f64; 3] {
    let n = norm(a);
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

    /// A camera straight above, looking down: 10 px per mm, the world
    /// origin at the middle of an 800 × 600 viewport.
    fn from_above(cursor: [f64; 2]) -> ScenePointer {
        let project = |p: [f64; 3]| Some([400.0 + p[0] * 10.0, 300.0 - p[1] * 10.0]);
        let at = [(cursor[0] - 400.0) / 10.0, (300.0 - cursor[1]) / 10.0];
        ScenePointer {
            origin: [at[0], at[1], 100.0],
            direction: [0.0, 0.0, -1.0],
            cursor,
            project: Box::new(project),
        }
    }

    fn level() -> Gizmo {
        Gizmo {
            center: [0.0; 3],
            n: [0.0, 0.0, 1.0],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            corners: [
                [-10.0, -10.0, 0.0],
                [10.0, -10.0, 0.0],
                [10.0, 10.0, 0.0],
                [-10.0, 10.0, 0.0],
            ],
            reach: 40.0,
        }
    }

    #[test]
    fn each_handle_is_picked_where_it_shows() {
        let g = level();
        // A corner, a few pixels off.
        assert_eq!(
            g.pick(&from_above([400.0 + 100.0 + 6.0, 300.0 - 100.0])),
            Some(Handle::Corner(2))
        );
        // Seen from above the ring about x lies along y, the one about y
        // along x, at 11.2 mm.
        assert_eq!(
            g.pick(&from_above([400.0, 300.0 - 112.0])),
            Some(Handle::Ring(0))
        );
        assert_eq!(
            g.pick(&from_above([400.0 + 112.0, 300.0])),
            Some(Handle::Ring(1))
        );
        // The arrow points at the eye: its foot is the middle.
        assert_eq!(g.pick(&from_above([401.0, 301.0])), Some(Handle::Arrow));
        // Elsewhere on the shape, the plane; off it, nothing.
        assert_eq!(g.pick(&from_above([450.0, 250.0])), Some(Handle::Plane));
        assert_eq!(g.pick(&from_above([700.0, 300.0])), None);
    }

    #[test]
    fn a_turn_about_an_axis_is_right_handed() {
        let p = turned([0.0, 1.0, 0.0], [0.0; 3], [1.0, 0.0, 0.0], TAU / 4.0);
        assert!((p[2] - 1.0).abs() < 1e-12 && p[1].abs() < 1e-12, "{p:?}");
    }

    #[test]
    fn the_handles_draw_closed_triangle_lists() {
        for mesh in level().meshes(Some(Handle::Arrow)) {
            assert!(!mesh.indices.is_empty());
            assert_eq!(mesh.indices.len() % 3, 0);
            assert!(
                mesh.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.positions.len())
            );
        }
    }
}
