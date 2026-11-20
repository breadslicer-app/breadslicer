/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Cut tool (#56): a plane cut in the Transform tab.
//!
//! The tool (the rail's Cut button, or `K`) shows a plane through the
//! selected model, with handles of its own in place of the object's gizmo
//! ([`crate::cut_gizmo`]): the arrow moves it along its normal, the rings
//! tilt it, and its four corners bound it to a shape on the plane, each
//! corner moved on the plane alone (#56: cutting a miniature at the waist
//! while its long arms reach past it); a drag on the plane moves the shape
//! along it. The card sets the same by numbers. Where the plane crosses the
//! model shows as the pieces of the cross-section ([`sc_cut`]): green the
//! ones the cut takes, grey the ones it leaves whole, amber the ones the
//! shape's edge crosses (the cut waits until the shape clears them, since
//! cutting part of a piece leaves a slit). A click on a piece takes it or
//! leaves it.
//!
//! The section follows the card at once (a few milliseconds). Once the plane
//! settles, the cut itself is worked out on a worker, so the card says how
//! many pieces it leaves before anything changes, and says when a bounded
//! cut leaves one piece, marking in red what joins it. Cut then puts that
//! result in the scene: the model goes, its pieces come where it stood, each
//! closed, and its supports go with it, in one undo entry.
//!
//! A model that is not closed is repaired first, on a copy, as Slice asks:
//! the pieces come from the repaired model.
//!
//! **Strokes (#440, #441).** The card's Stroke mode puts the plane aside: a
//! drag over the model draws a stroke, and the cut follows it, swept along
//! the view through the model ([`sc_cut::stroke_cut`]). A stroke from one
//! side of the model to the other cuts it in two along it; a loop drawn
//! round a part (a head, a hand, a shoulder pad) cuts that part off, flat
//! at the plane the loop's points on the surface best fit. The stroke shows
//! as it is drawn, the cut is worked out when it ends, and Cut puts it in
//! the scene as a plane cut does.
//!
//! **Suggestions (#443).** Suggest puts the plane, level, at the narrowest
//! place of the model's cross-section (a neck, a waist, an ankle:
//! [`sc_cut::narrowings`]); pressing it again goes to the next one. Nothing
//! is cut until Cut.
//!
//! **Joints (#109, #442).** The card can add a pin on the part below the
//! plane and its socket on the part above, or a magnet pocket on each side,
//! in the middle of each part of the cross-section the cut takes. They are
//! worked out with the cut, so the card says how many fit before anything
//! changes; one that does not fit (too near the edge of the cut, or too deep
//! for the part behind it) is left out and said.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sc_cut::{
    Cut, CutError, CutModel, Fit, Joint, MAGNET_PRESETS, Outline, PinShape, Plane, Section,
    add_joints, joint_points,
};
use sc_geom::df_supports::is_any_support_mesh;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::RepairPasses;
use sc_mesh_core::indexed_mesh::repair::repaired_copy;
use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;
use sc_scene::node::SceneMetadata;
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::cut_gizmo::{
    CutHandles, Gizmo, Handle, RingTurn, ScenePointer, add, dot, normalize, scale, sub, turned,
    turned_dir,
};
use crate::error_dialog::{DialogManager, ErrorTexts};
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::mesh_render::ToolGizmo;
use crate::mesh_render::{Overlay, OverlayMesh, OverlayTint};
use crate::scene_document::{EditNotApplied, SceneDocument, SceneEvent, SceneSubscription};
use crate::selection::{LeafId, Selection};
use crate::split_bodies::{leaf_name, removal_edit, split_edit};
use crate::support_persist::sets_of_model;
use crate::viewport::ViewportController;

/// How long the plane has to stay still before the cut is worked out.
const SETTLE: Duration = Duration::from_millis(180);

/// What the card sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CutSettings {
    /// The plane's height at the model's centre, world mm.
    pub(crate) height: f64,
    /// Its tilt about x and about y, degrees.
    pub(crate) tilt: [f64; 2],
    /// Whether the cut keeps to the shape the corners make on the plane.
    pub(crate) bounded: bool,
    /// The shape's corners, in order around it, along the plane's two
    /// directions from where the plane crosses the model's centre line, mm.
    pub(crate) corners: [[f64; 2]; 4],
}

impl CutSettings {
    /// The box round the corners: its least and greatest corner.
    fn extent(&self) -> ([f64; 2], [f64; 2]) {
        let mut lo = [f64::MAX; 2];
        let mut hi = [f64::MIN; 2];
        for c in self.corners {
            for k in 0..2 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        (lo, hi)
    }

    /// The shape's width and depth, as the card shows them.
    fn size(&self) -> [f64; 2] {
        let (lo, hi) = self.extent();
        [hi[0] - lo[0], hi[1] - lo[1]]
    }

    /// Where the middle of its box lies, as the card shows it.
    fn offset(&self) -> [f64; 2] {
        let (lo, hi) = self.extent();
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
    }

    /// The corners stretched along direction `k` to `size`, about the
    /// middle of their box.
    fn stretched(&self, k: usize, size: f64) -> [[f64; 2]; 4] {
        let old = self.size()[k];
        let mid = self.offset()[k];
        if old < 1e-9 {
            let mut half = self.size().map(|s| s / 2.0);
            half[k] = size / 2.0;
            return rect_corners(self.offset(), half);
        }
        let mut corners = self.corners;
        for c in &mut corners {
            c[k] = mid + (c[k] - mid) * size / old;
        }
        corners
    }
}

/// The joints the card asks for (#109, #442), each kind keeping its own
/// sizes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct JointSettings {
    /// 0 none, 1 pin and socket, 2 magnet pockets.
    pub(crate) kind: u8,
    pub(crate) square: bool,
    /// Pin: its size, depth and clearance, mm.
    pub(crate) pin: [f64; 3],
    /// Magnet: its diameter, height and clearance, mm.
    pub(crate) magnet: [f64; 3],
}

impl Default for JointSettings {
    fn default() -> Self {
        let (
            Joint::Pin {
                size_mm,
                depth_mm,
                clearance_mm,
                ..
            },
            Joint::Magnet {
                diameter_mm,
                height_mm,
                clearance_mm: magnet_clearance,
            },
        ) = (Joint::default_pin(), Joint::default_magnet())
        else {
            unreachable!("the defaults are a pin and a magnet")
        };
        Self {
            kind: 0,
            square: false,
            pin: [size_mm, depth_mm, clearance_mm],
            magnet: [diameter_mm, height_mm, magnet_clearance],
        }
    }
}

impl JointSettings {
    /// The joint to make, if any.
    pub(crate) fn joint(&self) -> Option<Joint> {
        match self.kind {
            1 => Some(Joint::Pin {
                shape: if self.square {
                    PinShape::Square
                } else {
                    PinShape::Round
                },
                size_mm: self.pin[0],
                depth_mm: self.pin[1],
                clearance_mm: self.pin[2],
            }),
            2 => Some(Joint::Magnet {
                diameter_mm: self.magnet[0],
                height_mm: self.magnet[1],
                clearance_mm: self.magnet[2],
            }),
            _ => None,
        }
    }

    /// The sizes of the kind on show: size, depth and clearance.
    fn shown(&self) -> [f64; 3] {
        if self.kind == 2 {
            self.magnet
        } else {
            self.pin
        }
    }

    /// The magnet preset the sizes are, or -1.
    fn preset(&self) -> i32 {
        MAGNET_PRESETS
            .iter()
            .position(|&(d, h)| {
                (d - self.magnet[0]).abs() < 1e-6 && (h - self.magnet[1]).abs() < 1e-6
            })
            .map_or(-1, |i| i as i32)
    }
}

/// A rectangle's corners, in order round it.
fn rect_corners(center: [f64; 2], half: [f64; 2]) -> [[f64; 2]; 4] {
    let [x, y] = center;
    let [w, h] = half;
    [
        [x - w, y - h],
        [x + w, y - h],
        [x + w, y + h],
        [x - w, y + h],
    ]
}

/// The model the tool works on, and where it stands.
#[derive(Clone)]
struct Target {
    leaf: NodeId,
    mesh: Arc<IndexedMesh>,
    /// The leaf's mesh to the world.
    world: Affine,
    /// Its box in the world.
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Target {
    fn selected(document: &SceneDocument) -> Option<Self> {
        let leaf = document.selection_primary_node_id()?;
        document
            .with_scene(|spatial| {
                let query = spatial.leaf(leaf)?;
                let mesh = spatial.graph().leaf_mesh_arc(leaf)?.clone();
                if is_any_support_mesh(&mesh) || mesh.faces().is_empty() {
                    return None;
                }
                let world = Affine::of(&query.world_transform());
                let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
                for v in mesh.vertices() {
                    let p = world.apply(v.as_array().map(f64::from));
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
                Some(Target {
                    leaf,
                    mesh,
                    world,
                    lo,
                    hi,
                })
            })
            .flatten()
    }

    fn center(&self) -> [f64; 3] {
        [0, 1, 2].map(|k| (self.lo[k] + self.hi[k]) / 2.0)
    }

    /// Its longest side, mm.
    fn reach(&self) -> f64 {
        (0..3)
            .map(|k| self.hi[k] - self.lo[k])
            .fold(0.0f64, f64::max)
    }

    /// The same model in the same place.
    fn same(&self, other: &Self) -> bool {
        self.leaf == other.leaf && Arc::ptr_eq(&self.mesh, &other.mesh) && self.world == other.world
    }
}

/// An affine map in f64: `linear · p + translation`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine {
    linear: [[f64; 3]; 3],
    translation: [f64; 3],
}

impl Affine {
    fn of(t: &Transform3D) -> Self {
        match t {
            Transform3D::Identity => Self {
                linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                translation: [0.0; 3],
            },
            Transform3D::Affine(a) => Self {
                linear: *a.linear(),
                translation: *a.translation(),
            },
        }
    }

    fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|r| {
            self.linear[r][0] * p[0]
                + self.linear[r][1] * p[1]
                + self.linear[r][2] * p[2]
                + self.translation[r]
        })
    }

    fn inverse(&self) -> Option<Self> {
        let m = self.linear;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let c = |r: usize, k: usize| {
            let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
            let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
            m[r1][k1] * m[r2][k2] - m[r1][k2] * m[r2][k1]
        };
        // The inverse is the transposed cofactors over the determinant.
        let linear = [0, 1, 2].map(|r| [0, 1, 2].map(|k| c(k, r) / det));
        let t = self.translation;
        let translation =
            [0, 1, 2].map(|r| -(linear[r][0] * t[0] + linear[r][1] * t[1] + linear[r][2] * t[2]));
        Some(Self {
            linear,
            translation,
        })
    }

    /// Where a plane's normal goes: through the transposed linear part.
    fn normal_back(&self, n: [f64; 3]) -> [f64; 3] {
        [0, 1, 2]
            .map(|k| self.linear[0][k] * n[0] + self.linear[1][k] * n[1] + self.linear[2][k] * n[2])
    }
}

/// The plane and shape the settings make for a model, in the world.
#[derive(Clone)]
struct Placed {
    plane: Plane,
    /// The plane's two directions in the world.
    u: [f64; 3],
    v: [f64; 3],
    /// Where the plane crosses the model's centre line: the corners are
    /// measured from it.
    anchor: [f64; 3],
    /// The shape on show: the settings' corners, or, not bounded, a square
    /// wider than the model.
    corners: [[f64; 2]; 4],
    /// The model's longest side, mm.
    reach: f64,
}

impl Placed {
    /// The world point at `q` on the plane.
    fn world(&self, q: [f64; 2]) -> [f64; 3] {
        add(self.anchor, add(scale(self.u, q[0]), scale(self.v, q[1])))
    }

    /// Where world point `p` lies along the plane's directions.
    fn flat(&self, p: [f64; 3]) -> [f64; 2] {
        let d = sub(p, self.anchor);
        [dot(d, self.u), dot(d, self.v)]
    }

    fn gizmo(&self) -> Gizmo {
        let mut lo = [f64::MAX; 2];
        let mut hi = [f64::MIN; 2];
        for c in self.corners {
            for k in 0..2 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        Gizmo {
            center: self.world([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]),
            n: self.plane.normal(),
            u: self.u,
            v: self.v,
            corners: self.corners.map(|c| self.world(c)),
        }
    }
}

/// The normal a tilt about x, then about y, gives.
fn tilted_normal(tilt: [f64; 2]) -> [f64; 3] {
    let (ax, ay) = (tilt[0].to_radians(), tilt[1].to_radians());
    [ay.sin() * ax.cos(), -ax.sin(), ay.cos() * ax.cos()]
}

fn placed(settings: &CutSettings, target: &Target) -> Option<Placed> {
    let c = target.center();
    let anchor = [c[0], c[1], settings.height];
    let plane = Plane::through(anchor, tilted_normal(settings.tilt))?;
    let (u, v) = plane.frame();
    let reach = target.reach();
    let corners = if settings.bounded {
        settings.corners
    } else {
        let half = reach * 0.75 + 2.0;
        rect_corners([0.0; 2], [half; 2])
    };
    Some(Placed {
        plane,
        u,
        v,
        anchor,
        corners,
        reach,
    })
}

/// The settings for a plane with normal `n` through `through`, whose
/// corners (all four, bounded or not) stand at `corners` in the world. The
/// tilt keeps within 89 degrees; corners off the plane it then makes are
/// taken along it.
fn posed(
    base: &CutSettings,
    target: &Target,
    n: [f64; 3],
    through: [f64; 3],
    corners: [[f64; 3]; 4],
) -> CutSettings {
    let n = normalize(n);
    let tilt = [
        (-n[1])
            .clamp(-1.0, 1.0)
            .asin()
            .to_degrees()
            .clamp(-89.0, 89.0),
        n[0].atan2(n[2]).to_degrees().clamp(-89.0, 89.0),
    ];
    let n = tilted_normal(tilt);
    let c = target.center();
    let height = through[2] - (n[0] * (c[0] - through[0]) + n[1] * (c[1] - through[1])) / n[2];
    let mut settings = CutSettings {
        height,
        tilt,
        ..*base
    };
    if let Some(at) = placed(&settings, target) {
        settings.corners = corners.map(|w| at.flat(w));
    }
    settings
}

/// The two triangles of a simple quad, by the diagonal that lies inside it.
fn quad_triangles(q: &[[f64; 2]; 4]) -> [u32; 6] {
    let side = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
    };
    if side(q[0], q[2], q[1]) * side(q[0], q[2], q[3]) < 0.0 {
        [0, 1, 2, 0, 2, 3]
    } else {
        [0, 1, 3, 1, 2, 3]
    }
}

/// A drag of one of the gizmo's handles, from where it was pressed.
struct Grab {
    handle: Handle,
    /// The settings, the plane and the gizmo at the press.
    settings: CutSettings,
    at: Placed,
    gizmo: Gizmo,
    target: Target,
    /// Where the press met the plane.
    hit: Option<[f64; 3]>,
    /// How far along the arrow the press lay.
    along: Option<f64>,
    /// How a ring's drag turns it, where on the ring it was grabbed and
    /// how far it has turned, radians.
    turn: Option<Turn>,
    start: f64,
    swept: f64,
}

/// How a drag on a ring turns the plane.
enum Turn {
    /// By the cursor's angle round the gizmo's centre on the screen, as the
    /// object's gizmo turns (`sign` -1 when the ring's axis points away from
    /// the eye): `last` the angle at the last move, `so_far` the turn.
    Angle {
        pivot: [f64; 2],
        last: f64,
        so_far: f64,
        sign: f64,
    },
    /// Seen edge on, the ring is a line: by how far the cursor moved along
    /// the ring where it was grabbed, `per_radian` pixels a radian.
    Along {
        from: [f64; 2],
        dir: [f64; 2],
        per_radian: f64,
    },
}

/// Below how square to the view a ring's axis stands its turn goes along
/// the ring instead of round its centre.
const EDGE_ON: f64 = 0.3;

impl Turn {
    fn start(gizmo: &Gizmo, k: usize, pointer: &ScenePointer) -> Option<Self> {
        let axis = gizmo.ring_axis(k);
        let (point, _, _) = gizmo.ring_point_near(k, pointer)?;
        let facing = dot(axis, pointer.direction);
        if facing.abs() >= EDGE_ON {
            let pivot = (pointer.project)(gizmo.center)?;
            return Some(Turn::Angle {
                pivot,
                last: screen_angle(pointer.cursor, pivot),
                so_far: 0.0,
                sign: if facing > 0.0 { -1.0 } else { 1.0 },
            });
        }
        // Which way a turn moves the ring's point nearest the cursor on the
        // screen.
        let step = 0.01;
        let ahead = turned(point, gizmo.center, axis, step);
        let (a, b) = ((pointer.project)(point)?, (pointer.project)(ahead)?);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = d[0].hypot(d[1]);
        (len > 1e-6).then(|| Turn::Along {
            from: pointer.cursor,
            dir: [d[0] / len, d[1] / len],
            per_radian: len / step,
        })
    }

    /// The turn so far, radians, with the cursor at `cursor`.
    fn update(&mut self, cursor: [f64; 2]) -> f64 {
        match self {
            Turn::Angle {
                pivot,
                last,
                so_far,
                sign,
            } => {
                let now = screen_angle(cursor, *pivot);
                let mut step = now - *last;
                if step > std::f64::consts::PI {
                    step -= std::f64::consts::TAU;
                } else if step < -std::f64::consts::PI {
                    step += std::f64::consts::TAU;
                }
                *last = now;
                *so_far += step;
                *so_far * *sign
            }
            Turn::Along {
                from,
                dir,
                per_radian,
            } => ((cursor[0] - from[0]) * dir[0] + (cursor[1] - from[1]) * dir[1]) / *per_radian,
        }
    }
}

/// The cursor's angle round `pivot` on the screen, counterclockwise.
fn screen_angle(cursor: [f64; 2], pivot: [f64; 2]) -> f64 {
    (pivot[1] - cursor[1]).atan2(cursor[0] - pivot[0])
}

/// A model made ready to cut, for one mesh.
struct Ready {
    mesh: Arc<IndexedMesh>,
    model: CutModel,
    /// Whether the model had to be repaired first.
    repaired: bool,
}

/// What the worker hands back.
enum Worked {
    Ready(Result<Ready, String>),
    /// The cut, with how many joints it got and how many were left out.
    Trial(Result<(Cut, [usize; 2]), CutError>),
    /// A cut along a stroke (#440, #441).
    Stroke(Result<Cut, CutError>),
}

/// A stroke cut worked out, or being worked out (#440, #441).
struct StrokeTrial {
    target: Target,
    job: u64,
    /// A loop, which cuts off what it encloses.
    closed: bool,
    cut: Option<Arc<Cut>>,
    failed: bool,
}

/// A stroke in the world: its frame across the view and its points in it.
struct StrokeFrame {
    origin: [f64; 3],
    u: [f64; 3],
    v: [f64; 3],
    along: [f64; 3],
    points: Vec<[f64; 2]>,
    closed: bool,
}

/// The stroke drawn by the rays `rays` (eye and direction), across the
/// view through `center`: the mean direction is the view, each ray meets
/// the plane across it through `center`. A loop when it ends near where it
/// began.
fn stroke_frame(rays: &[([f64; 3], [f64; 3])], center: [f64; 3]) -> Option<StrokeFrame> {
    let sum = rays
        .iter()
        .fold([0.0; 3], |a, (_, d)| add(a, normalize(*d)));
    let along = normalize(sum);
    if !along.iter().all(|c| c.is_finite()) || dot(along, along) < 0.5 {
        return None;
    }
    let (u, v) = sc_cut::Stroke::frame(along);
    let points: Vec<[f64; 2]> = rays
        .iter()
        .filter_map(|(o, d)| {
            let k = dot(along, *d);
            if k.abs() < 1e-9 {
                return None;
            }
            let t = dot(along, sub(center, *o)) / k;
            let p = sub(add(*o, scale(*d, t)), center);
            Some([dot(p, u), dot(p, v)])
        })
        .collect();
    if points.len() < 2 {
        return None;
    }
    let length: f64 = points
        .windows(2)
        .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
        .sum();
    let (a, b) = (points[0], points[points.len() - 1]);
    let closed = points.len() >= 4 && (a[0] - b[0]).hypot(a[1] - b[1]) <= (0.12 * length).max(1.0);
    Some(StrokeFrame {
        origin: center,
        u,
        v,
        along,
        points,
        closed,
    })
}

/// Where up to 48 of `rays` (in the mesh's frame) first meet `mesh`: the
/// points of a loop on the surface.
fn surface_points(mesh: &IndexedMesh, rays: &[([f64; 3], [f64; 3])]) -> Vec<[f64; 3]> {
    let step = (rays.len() / 48).max(1);
    let vertices = mesh.vertices();
    rays.iter()
        .step_by(step)
        .filter_map(|(o, d)| {
            let mut best: Option<f64> = None;
            for f in mesh.faces() {
                let [a, b, c] = f
                    .vertices
                    .map(|v| vertices[v.get() as usize].as_array().map(f64::from));
                let (e1, e2) = (sub(b, a), sub(c, a));
                let h = cross(*d, e2);
                let det = dot(e1, h);
                if det.abs() < 1e-14 {
                    continue;
                }
                let s = sub(*o, a);
                let bu = dot(s, h) / det;
                if !(0.0..=1.0).contains(&bu) {
                    continue;
                }
                let q = cross(s, e1);
                let bv = dot(*d, q) / det;
                if bv < 0.0 || bu + bv > 1.0 {
                    continue;
                }
                let t = dot(e2, q) / det;
                if t > 1e-9 && best.is_none_or(|b| t < b) {
                    best = Some(t);
                }
            }
            best.map(|t| add(*o, scale(*d, t)))
        })
        .collect()
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

type Slot = Arc<Mutex<Option<(u64, Worked)>>>;

/// The section on show, and what was chosen of it.
struct Shown {
    target: Target,
    settings: CutSettings,
    section: Arc<Section>,
    fits: Vec<Fit>,
    chosen: Vec<bool>,
    /// Each piece's fill, in the world.
    fills: Vec<(Vec<[f32; 3]>, Vec<u32>)>,
    /// The cut worked out for exactly this, once it is.
    trial: Option<Arc<Cut>>,
    /// The worker job working it out, or 0.
    trial_job: u64,
    /// The joints the trial got and those left out (#109, #442).
    joints: [usize; 2],
}

pub(crate) struct CutRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    ctrl: Rc<ViewportController>,
    open: Cell<bool>,
    settings: Cell<CutSettings>,
    /// The joints across the cut (#109, #442).
    joint: Cell<JointSettings>,
    /// Drawing a stroke over the model in place of the plane (#440, #441).
    stroke_mode: Cell<bool>,
    /// The rays of the stroke drawn, and the cursor at the last one.
    stroke: RefCell<Vec<([f64; 3], [f64; 3])>>,
    stroke_cursor: Cell<[f64; 2]>,
    drawing: Cell<bool>,
    stroke_trial: RefCell<Option<StrokeTrial>>,
    /// The suggested heights for the model on show, world mm, best first,
    /// and the one on show (#443).
    suggestions: RefCell<Option<(Target, Vec<f64>)>>,
    suggested: Cell<usize>,
    target: RefCell<Option<Target>>,
    ready: RefCell<Option<Arc<Ready>>>,
    /// The mesh being made ready.
    preparing: RefCell<Option<Arc<IndexedMesh>>>,
    shown: RefCell<Option<Shown>>,
    /// Pieces taken or left by a click, for the section on show.
    toggled: RefCell<HashMap<usize, bool>>,
    next_id: Cell<u64>,
    /// The job making a model ready, whose result is waited for.
    waiting: Cell<u64>,
    /// Apply was asked for before the trial cut was in.
    apply_when_in: Cell<bool>,
    slot: Slot,
    settle: slint::Timer,
    /// Clears the status bar's word on a cut.
    notice: slint::Timer,
    /// The handle under the cursor.
    hot: Cell<Option<Handle>>,
    /// The handle being dragged.
    grab: RefCell<Option<Grab>>,
    _scene_events: SceneSubscription,
}

impl CutRun {
    /// Wires the Cut tool; the caller keeps the driver alive for the
    /// window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new_cyclic(|me: &Weak<Self>| {
            let weak = me.clone();
            let scene_events = document.subscribe(move |event| {
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::SelectionChanged
                        | SceneEvent::UndoStackChanged
                        | SceneEvent::Restructured
                ) {
                    let weak = weak.clone();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(run) = weak.upgrade() {
                            run.on_scene_changed();
                        }
                    });
                }
            });
            Self {
                ui: ui.as_weak(),
                document,
                dialogs,
                ctrl,
                open: Cell::new(false),
                settings: Cell::new(CutSettings {
                    height: 0.0,
                    tilt: [0.0; 2],
                    bounded: false,
                    corners: rect_corners([0.0; 2], [10.0; 2]),
                }),
                joint: Cell::new(JointSettings::default()),
                stroke_mode: Cell::new(false),
                stroke: RefCell::new(Vec::new()),
                stroke_cursor: Cell::new([0.0; 2]),
                drawing: Cell::new(false),
                stroke_trial: RefCell::new(None),
                suggestions: RefCell::new(None),
                suggested: Cell::new(0),
                target: RefCell::new(None),
                ready: RefCell::new(None),
                preparing: RefCell::new(None),
                shown: RefCell::new(None),
                toggled: RefCell::new(HashMap::new()),
                next_id: Cell::new(1),
                waiting: Cell::new(0),
                apply_when_in: Cell::new(false),
                slot: Arc::new(Mutex::new(None)),
                settle: slint::Timer::default(),
                notice: slint::Timer::default(),
                hot: Cell::new(None),
                grab: RefCell::new(None),
                _scene_events: scene_events,
            }
        });
        macro_rules! wire {
            ($on:ident, |$run:ident $(, $arg:ident)*| $body:expr) => {{
                let $run = run.clone();
                ui.$on(move |$($arg),*| $body);
            }};
        }
        wire!(on_cut_toggle_requested, |r| r.toggle());
        wire!(on_cut_close_requested, |r| r.close());
        wire!(on_cut_option_changed, |r, code, value| r
            .on_option(code, value));
        wire!(on_cut_apply, |r| r.apply());
        wire!(on_cut_worked, |r| r.worked());
        run.ctrl
            .set_cut_handles(Rc::new(Handles(Rc::downgrade(&run))));
        run
    }

    pub(crate) fn toggle(&self) {
        if self.open.get() {
            self.close();
        } else {
            self.open_tool();
        }
    }

    fn open_tool(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(target) = Target::selected(&self.document) else {
            return;
        };
        self.open.set(true);
        ui.set_cut_open(true);
        self.take_target(target, true);
    }

    /// Closes the tool; nothing in the scene changes.
    pub(crate) fn close(&self) {
        if !self.open.replace(false) {
            return;
        }
        self.stroke.borrow_mut().clear();
        self.drawing.set(false);
        self.stroke_trial.borrow_mut().take();
        self.settle.stop();
        self.waiting.set(0);
        self.apply_when_in.set(false);
        self.shown.borrow_mut().take();
        self.grab.borrow_mut().take();
        self.hot.set(None);
        self.ctrl.set_preview_overlay(None);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_open(false);
            ui.set_cut_working(false);
            ui.set_cut_status("".into());
            ui.set_cut_warning("".into());
            ui.set_cut_can_cut(false);
        }
    }

    fn editable(&self) -> bool {
        self.ui.upgrade().is_some_and(|ui| {
            !ui.get_loading()
                && !ui.get_slicing_in_progress()
                && !ui.get_repairing_in_progress()
                && !ui.get_arranging()
        }) && self.ctrl.allows_layout_tools()
    }

    /// Works on `target`: a new model puts the plane through its middle.
    fn take_target(&self, target: Target, fresh: bool) {
        let new_model = self
            .target
            .borrow()
            .as_ref()
            .is_none_or(|t| t.leaf != target.leaf || !Arc::ptr_eq(&t.mesh, &target.mesh));
        if fresh || new_model {
            let mut s = self.settings.get();
            s.height = (target.lo[2] + target.hi[2]) / 2.0;
            s.tilt = [0.0; 2];
            s.corners = rect_corners(
                [0.0; 2],
                [
                    (target.hi[0] - target.lo[0]) / 2.0 + 1.0,
                    (target.hi[1] - target.lo[1]) / 2.0 + 1.0,
                ],
            );
            self.settings.set(s);
        }
        *self.target.borrow_mut() = Some(target.clone());
        self.push_settings();
        let ready = self
            .ready
            .borrow()
            .as_ref()
            .filter(|r| Arc::ptr_eq(&r.mesh, &target.mesh))
            .cloned();
        if ready.is_some() {
            self.refresh();
        } else {
            self.prepare(target.mesh.clone());
        }
    }

    fn push_settings(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let s = self.settings.get();
        if let Some(t) = self.target.borrow().as_ref() {
            ui.set_cut_height_min(t.lo[2] as f32);
            ui.set_cut_height_max(t.hi[2] as f32);
        }
        ui.set_cut_height(s.height as f32);
        ui.set_cut_tilt_x(s.tilt[0] as f32);
        ui.set_cut_tilt_y(s.tilt[1] as f32);
        ui.set_cut_bounded(s.bounded);
        let [w, d] = s.size();
        let [ou, ov] = s.offset();
        ui.set_cut_width(w as f32);
        ui.set_cut_depth(d as f32);
        ui.set_cut_offset_u(ou as f32);
        ui.set_cut_offset_v(ov as f32);
        let j = self.joint.get();
        let [size, depth, clearance] = j.shown();
        ui.set_cut_joint_kind(i32::from(j.kind));
        ui.set_cut_joint_square(j.square);
        ui.set_cut_joint_size(size as f32);
        ui.set_cut_joint_depth(depth as f32);
        ui.set_cut_joint_clearance(clearance as f32);
        ui.set_cut_joint_preset(j.preset());
    }

    /// Suggest (#443): the plane, level, to the narrowest place of the
    /// model's cross-section; again, to the next one.
    fn suggest(&self) {
        let Some((_, target)) = self.on_show() else {
            return;
        };
        if self.stroke_mode.get() {
            self.set_stroke_mode(false);
        }
        let known = self
            .suggestions
            .borrow()
            .as_ref()
            .filter(|(t, _)| t.same(&target))
            .map(|(_, list)| list.clone());
        let (list, next) = match known {
            Some(list) if !list.is_empty() => {
                let next = (self.suggested.get() + 1) % list.len();
                (list, next)
            }
            _ => {
                // The model in the world, so heights and areas are the
                // scene's.
                let vertices = target
                    .mesh
                    .vertices()
                    .iter()
                    .map(|v| {
                        let p = target.world.apply(v.as_array().map(f64::from));
                        sc_mesh_core::Vertex::new(p.map(|c| c as f32))
                    })
                    .collect();
                let world = IndexedMesh::new(None, vertices, target.mesh.faces().to_vec());
                let list: Vec<f64> = sc_cut::narrowings(&world, [0.0, 0.0, 1.0])
                    .into_iter()
                    .map(|n| n.at)
                    .collect();
                *self.suggestions.borrow_mut() = Some((target.clone(), list.clone()));
                (list, 0)
            }
        };
        let Some(&height) = list.get(next) else {
            self.say(
                self.status_line(),
                ctr!(
                    "CutRun",
                    "No narrowing found to suggest: the model is about as wide all the way up."
                ),
                self.ui.upgrade().is_some_and(|ui| ui.get_cut_can_cut()),
            );
            return;
        };
        self.suggested.set(next);
        let mut s = self.settings.get();
        s.height = height;
        s.tilt = [0.0; 2];
        self.settings.set(s);
        self.push_settings();
        self.refresh();
    }

    /// Which suggestion the plane stands at, as the card says it.
    fn suggestion_note(&self) -> Option<String> {
        let s = self.settings.get();
        let suggestions = self.suggestions.borrow();
        let (_, list) = suggestions.as_ref()?;
        let k = self.suggested.get();
        let at = *list.get(k)?;
        ((s.height - at).abs() < 1e-6 && s.tilt == [0.0; 2]).then(|| {
            ctr!(
                "CutRun",
                "Suggested: a narrowing of the model ({}). Suggest again for the next.",
                format!("{} / {}", k + 1, list.len())
            )
            .to_string()
        })
    }

    /// Draws strokes (`on`) or moves the plane (#440, #441).
    fn set_stroke_mode(&self, on: bool) {
        if on == self.stroke_mode.get() {
            return;
        }
        self.stroke_mode.set(on);
        self.stroke.borrow_mut().clear();
        self.drawing.set(false);
        self.stroke_trial.borrow_mut().take();
        self.grab.borrow_mut().take();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_stroke_mode(on);
        }
        if on {
            self.show_stroke();
            self.describe_stroke();
        } else {
            self.refresh();
        }
    }

    fn stroke_press(&self, pointer: &ScenePointer) -> bool {
        if self.on_show().is_none() {
            return false;
        }
        self.drawing.set(true);
        *self.stroke.borrow_mut() = vec![(pointer.origin, pointer.direction)];
        self.stroke_cursor.set(pointer.cursor);
        self.stroke_trial.borrow_mut().take();
        self.show_stroke();
        self.describe_stroke();
        true
    }

    fn stroke_drag(&self, pointer: &ScenePointer) {
        let last = self.stroke_cursor.get();
        if (pointer.cursor[0] - last[0]).hypot(pointer.cursor[1] - last[1]) < 3.0 {
            return;
        }
        self.stroke_cursor.set(pointer.cursor);
        self.stroke
            .borrow_mut()
            .push((pointer.origin, pointer.direction));
        self.show_stroke();
    }

    fn stroke_release(&self, pointer: Option<&ScenePointer>) {
        self.drawing.set(false);
        let Some(pointer) = pointer else {
            // Given up: the stroke goes.
            self.stroke.borrow_mut().clear();
            self.show_stroke();
            self.describe_stroke();
            return;
        };
        self.stroke
            .borrow_mut()
            .push((pointer.origin, pointer.direction));
        self.show_stroke();
        self.start_stroke_trial();
    }

    /// Works out the cut along the stroke drawn, on a worker.
    fn start_stroke_trial(&self) {
        let Some((_, target)) = self.on_show() else {
            return;
        };
        let Some(ready) = self.ready.borrow().clone() else {
            return;
        };
        let rays = self.stroke.borrow().clone();
        let (Some(world), Some(back)) =
            (stroke_frame(&rays, target.center()), target.world.inverse())
        else {
            self.describe_stroke();
            return;
        };
        let id = self.job_id();
        *self.stroke_trial.borrow_mut() = Some(StrokeTrial {
            target: target.clone(),
            job: id,
            closed: world.closed,
            cut: None,
            failed: false,
        });
        // Into the mesh's own frame: the points stay in world mm along the
        // frame's directions, which the map takes along.
        let zero = back.apply([0.0; 3]);
        let vector = |v: [f64; 3]| sub(back.apply(v), zero);
        let stroke = sc_cut::Stroke {
            origin: back.apply(world.origin),
            u: vector(world.u),
            v: vector(world.v),
            along: vector(world.along),
            points: world.points,
            surface: Vec::new(),
            closed: world.closed,
        };
        let local: Vec<([f64; 3], [f64; 3])> = rays
            .iter()
            .map(|(o, d)| (back.apply(*o), vector(*d)))
            .collect();
        let mesh = ready.model.mesh().clone();
        let slot = self.slot.clone();
        let ui = self.ui.clone();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_working(true);
        }
        std::thread::spawn(move || {
            let cut = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut stroke = stroke;
                if stroke.closed {
                    stroke.surface = surface_points(&mesh, &local);
                }
                sc_cut::stroke_cut(&mesh, &stroke)
            }))
            .unwrap_or_else(|payload| Err(CutError::Failed(panic_message(&payload))));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((id, Worked::Stroke(cut)));
            let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_cut_worked());
        });
        self.describe_stroke();
    }

    /// What the card says in Stroke mode.
    fn describe_stroke(&self) {
        let trial = self.stroke_trial.borrow();
        let (status, warning, can) = match trial.as_ref() {
            None => (
                ctr!(
                    "CutRun",
                    "Draw across the model to cut it along the stroke, or draw a loop round a part to cut it off."
                ),
                Default::default(),
                false,
            ),
            Some(t) => match (&t.cut, t.failed) {
                (Some(cut), _) => (
                    ctr!("CutRun", "It leaves {} pieces.", cut.pieces.len()),
                    Default::default(),
                    cut.pieces.len() >= 2,
                ),
                (None, true) => (
                    Default::default(),
                    if t.closed {
                        ctr!(
                            "CutRun",
                            "The loop cuts nothing off: draw it round a part that stands out of the model."
                        )
                    } else {
                        ctr!(
                            "CutRun",
                            "The stroke does not cut the model in two: draw it from one side of the model to the other, without crossing itself."
                        )
                    },
                    false,
                ),
                (None, false) => (
                    ctr!("CutRun", "Working out the cut…"),
                    Default::default(),
                    false,
                ),
            },
        };
        drop(trial);
        self.say(status, warning, can);
    }

    /// The stroke drawn, as a ribbon across the view through the model's
    /// middle, and the surface it cuts along, through the model (#658).
    fn show_stroke(&self) {
        let Some((_, target)) = self.on_show() else {
            return;
        };
        let rays = self.stroke.borrow().clone();
        let mut pieces: Vec<Arc<OverlayMesh>> = Vec::new();
        if let Some(world) = stroke_frame(&rays, target.center()) {
            let half = (target.reach() * 0.004).max(0.08);
            let at = |p: [f64; 2]| {
                add(
                    world.origin,
                    add(scale(world.u, p[0]), scale(world.v, p[1])),
                )
            };
            let mut positions = Vec::new();
            let mut indices = Vec::new();
            for w in world.points.windows(2) {
                let (a, b) = (at(w[0]), at(w[1]));
                let side = normalize(cross(sub(b, a), world.along));
                if !side.iter().all(|c| c.is_finite()) {
                    continue;
                }
                let base = positions.len() as u32;
                for p in [
                    add(a, scale(side, half)),
                    add(a, scale(side, -half)),
                    add(b, scale(side, -half)),
                    add(b, scale(side, half)),
                ] {
                    // A hair towards the eye, so the model does not hide it.
                    let p = sub(p, scale(world.along, target.reach()));
                    positions.push(p.map(|x| x as f32));
                }
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
            }
            // The cutting surface itself (#658): the stroke swept along the
            // view it was drawn in, right through the model, drawn as the
            // plane of a plane cut is, so it shows from any side once the
            // camera turns.
            let mut wall_positions = Vec::new();
            let mut wall_indices = Vec::new();
            let deep = target.reach();
            for w in world.points.windows(2) {
                let (a, b) = (at(w[0]), at(w[1]));
                let base = wall_positions.len() as u32;
                for p in [
                    sub(a, scale(world.along, deep)),
                    add(a, scale(world.along, deep)),
                    add(b, scale(world.along, deep)),
                    sub(b, scale(world.along, deep)),
                ] {
                    wall_positions.push(p.map(|x| x as f32));
                }
                wall_indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                wall_indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
            }
            if !wall_positions.is_empty() {
                pieces.push(Arc::new(OverlayMesh {
                    positions: wall_positions,
                    indices: wall_indices,
                    tint: OverlayTint::Cavity,
                }));
            }
            if !positions.is_empty() {
                pieces.push(Arc::new(OverlayMesh {
                    positions,
                    indices,
                    tint: if world.closed {
                        OverlayTint::Drain
                    } else {
                        OverlayTint::Valid
                    },
                }));
            }
        }
        let overlay: Overlay = pieces.into();
        self.ctrl.set_preview_overlay(Some(overlay));
    }

    /// A joint setting (#109, #442): 8 kind (0 none, 1 pin, 2 magnet), 9 a
    /// square pin (0/1), 10 size, 11 depth, 12 clearance, 13 a magnet
    /// preset. The cut is worked out again with it.
    fn on_joint_option(&self, code: i32, value: f64) {
        let mut j = self.joint.get();
        let sizes = if j.kind == 2 {
            &mut j.magnet
        } else {
            &mut j.pin
        };
        match code {
            8 => j.kind = (value.round() as u8).min(2),
            9 => j.square = value != 0.0,
            10 => sizes[0] = value.max(0.1),
            11 => sizes[1] = value.max(0.1),
            12 => sizes[2] = value.clamp(0.0, 2.0),
            13 => {
                if let Some(&(d, h)) = MAGNET_PRESETS.get(value.round() as usize) {
                    j.magnet[0] = d;
                    j.magnet[1] = h;
                }
            }
            _ => return,
        }
        if j == self.joint.get() {
            return;
        }
        self.joint.set(j);
        self.push_settings();
        if let Some(shown) = self.shown.borrow_mut().as_mut() {
            shown.trial = None;
            shown.trial_job = 0;
            shown.joints = [0, 0];
        }
        self.describe();
        self.schedule_trial();
    }

    /// 0 height, 1 tilt about x, 2 tilt about y, 3 bounded (0/1), 4 width,
    /// 5 depth, 6 offset along the plane's first direction, 7 the second;
    /// -1 the settle timer (the plane stayed put: the cut is worked out).
    fn on_option(&self, code: i32, value: f32) {
        if code == -1 {
            self.start_trial();
            return;
        }
        if (8..=13).contains(&code) {
            self.on_joint_option(code, f64::from(value));
            return;
        }
        if code == 14 {
            self.set_stroke_mode(value != 0.0);
            return;
        }
        if code == 15 {
            self.suggest();
            return;
        }
        let mut s = self.settings.get();
        let value = f64::from(value);
        match code {
            0 => s.height = value,
            1 => s.tilt[0] = value.clamp(-89.0, 89.0),
            2 => s.tilt[1] = value.clamp(-89.0, 89.0),
            3 => s.bounded = value != 0.0,
            4 => s.corners = s.stretched(0, value.max(0.1)),
            5 => s.corners = s.stretched(1, value.max(0.1)),
            6 | 7 => {
                let k = (code - 6) as usize;
                let by = value - s.offset()[k];
                for c in &mut s.corners {
                    c[k] += by;
                }
            }
            _ => return,
        }
        if s == self.settings.get() {
            return;
        }
        self.settings.set(s);
        self.push_settings();
        self.refresh();
    }

    /// Makes `mesh` ready on a worker: checked closed, repaired when not.
    fn prepare(&self, mesh: Arc<IndexedMesh>) {
        if self
            .preparing
            .borrow()
            .as_ref()
            .is_some_and(|m| Arc::ptr_eq(m, &mesh))
        {
            return;
        }
        *self.preparing.borrow_mut() = Some(mesh.clone());
        self.shown.borrow_mut().take();
        self.ctrl.set_preview_overlay(None);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_working(true);
            ui.set_cut_can_cut(false);
            ui.set_cut_status(ctr!("CutRun", "Getting the model ready…"));
            ui.set_cut_warning("".into());
        }
        let id = self.job_id();
        self.waiting.set(id);
        let slot = self.slot.clone();
        let ui = self.ui.clone();
        std::thread::spawn(move || {
            let worked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match CutModel::new(mesh.clone()) {
                    Ok(model) => Ok(Ready {
                        mesh,
                        model,
                        repaired: false,
                    }),
                    Err(_) => {
                        let (_, fixed) = repaired_copy(&mesh, RepairPasses::default_repair());
                        let fixed =
                            fixed.ok_or_else(|| "the repair found nothing to do".to_owned())?;
                        let model = CutModel::new(Arc::new(fixed))
                            .map_err(|_| "it is not closed, even repaired".to_owned())?;
                        Ok(Ready {
                            mesh,
                            model,
                            repaired: true,
                        })
                    }
                }
            }))
            .unwrap_or_else(|payload| Err(panic_message(&payload)));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((id, Worked::Ready(worked)));
            let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_cut_worked());
        });
    }

    fn job_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// The section for the settings on show, drawn at once; the cut, once
    /// the plane settles.
    fn refresh(&self) {
        if !self.open.get() {
            return;
        }
        if self.stroke_mode.get() {
            self.show_stroke();
            self.describe_stroke();
            return;
        }
        let Some(target) = self.target.borrow().clone() else {
            return;
        };
        let Some(ready) = self.ready.borrow().clone() else {
            return;
        };
        if !Arc::ptr_eq(&ready.mesh, &target.mesh) {
            return;
        }
        let settings = self.settings.get();
        let same_plane = self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|s| s.target.same(&target) && s.settings == settings);
        if !same_plane {
            self.toggled.borrow_mut().clear();
        }
        let Some(world) = placed(&settings, &target) else {
            return;
        };
        let Some(back) = target.world.inverse() else {
            return;
        };
        let local = Plane::through(
            back.apply(world.anchor),
            target.world.normal_back(world.plane.normal()),
        );
        let section = local.and_then(|plane| ready.model.section(plane).ok());
        let Some(section) = section else {
            self.shown.borrow_mut().take();
            self.show_overlay(&target, &world, false);
            self.say(
                ctr!("CutRun", "The plane does not cross the model."),
                Default::default(),
                false,
            );
            return;
        };
        let outline = Outline::new(world.corners.to_vec())
            .unwrap_or_else(|| Outline::rect([0.0; 2], [world.reach + 2.0; 2]));
        let to_frame = |p: [f64; 3]| world.flat(target.world.apply(p));
        let from_frame = |q: [f64; 2]| back.apply(world.world(q));
        let fits = section.fits(&outline, to_frame, from_frame);
        let toggled = self.toggled.borrow();
        let chosen: Vec<bool> = fits
            .iter()
            .enumerate()
            .map(|(k, fit)| match fit {
                Fit::Crossed => false,
                Fit::Inside | Fit::Outside => {
                    toggled.get(&k).copied().unwrap_or(*fit == Fit::Inside)
                }
            })
            .collect();
        drop(toggled);
        // A hair off the plane, toward the camera's side either way, so the
        // plane does not hide them.
        let n = world.plane.normal();
        let fills = (0..fits.len())
            .map(|k| {
                let (points, mut indices) = section.fill(k);
                let base = indices.len();
                indices.extend_from_within(..base);
                for t in indices[base..].as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
                let positions = points
                    .iter()
                    .map(|p| {
                        let w = target.world.apply(p.map(f64::from));
                        [0, 1, 2].map(|i| (w[i] + n[i] * 0.01) as f32)
                    })
                    .collect();
                (positions, indices)
            })
            .collect();
        let section = Arc::new(section);
        *self.shown.borrow_mut() = Some(Shown {
            target: target.clone(),
            settings,
            section: section.clone(),
            fits,
            chosen,
            fills,
            trial: None,
            trial_job: 0,
            joints: [0, 0],
        });
        self.show_overlay(&target, &world, true);
        self.describe();
        self.schedule_trial();
    }

    /// Works the cut out once the plane has stayed put a moment.
    fn schedule_trial(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let ui = ui.as_weak();
        self.settle
            .start(slint::TimerMode::SingleShot, SETTLE, move || {
                if let Some(ui) = ui.upgrade() {
                    ui.invoke_cut_option_changed(-1, 0.0);
                }
            });
    }

    fn start_trial(&self) {
        let id = self.job_id();
        let Some((section, chosen)) = self.shown.borrow_mut().as_mut().and_then(|s| {
            if s.trial.is_some() || !s.chosen.iter().any(|&c| c) {
                return None;
            }
            s.trial_job = id;
            Some((s.section.clone(), s.chosen.clone()))
        }) else {
            return;
        };
        let slot = self.slot.clone();
        let ui = self.ui.clone();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_working(true);
        }
        let joint = self.joint.get().joint();
        std::thread::spawn(move || {
            let cut = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                with_joints(&section, &chosen, joint)
            }))
            .unwrap_or_else(|payload| Err(CutError::Failed(panic_message(&payload))));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((id, Worked::Trial(cut)));
            let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_cut_worked());
        });
    }

    /// Takes a worker's result on the UI thread.
    fn worked(&self) {
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some((id, worked)) = taken else {
            return;
        };
        // Only the job the tool still waits for counts: a model made ready
        // for the model on show, a cut for the section on show.
        let current = match worked {
            Worked::Stroke(_) => self
                .stroke_trial
                .borrow()
                .as_ref()
                .is_some_and(|s| s.job == id),
            Worked::Ready(_) => id == self.waiting.get(),
            Worked::Trial(_) => self
                .shown
                .borrow()
                .as_ref()
                .is_some_and(|s| s.trial_job == id),
        };
        if !current {
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_working(false);
        }
        match worked {
            Worked::Stroke(cut) => {
                if let Some(trial) = self.stroke_trial.borrow_mut().as_mut() {
                    match cut {
                        Ok(cut) => trial.cut = Some(Arc::new(cut)),
                        Err(_) => trial.failed = true,
                    }
                }
                self.describe_stroke();
            }
            Worked::Ready(ready) => {
                self.waiting.set(0);
                self.preparing.borrow_mut().take();
                match ready {
                    Ok(ready) => {
                        *self.ready.borrow_mut() = Some(Arc::new(ready));
                        self.refresh();
                    }
                    Err(why) => self.say(
                        ctr!("CutRun", "This model cannot be cut: {}", why),
                        Default::default(),
                        false,
                    ),
                }
            }
            Worked::Trial(cut) => match cut {
                Ok((cut, joints)) => {
                    if let Some(shown) = self.shown.borrow_mut().as_mut() {
                        shown.trial = Some(Arc::new(cut));
                        shown.joints = joints;
                    }
                    self.redraw();
                    self.describe();
                    if self.apply_when_in.replace(false) {
                        self.apply();
                    }
                }
                Err(e) => {
                    self.apply_when_in.set(false);
                    let why = match e {
                        CutError::NotClosedAfter => ctr!(
                            "CutRun",
                            "The pieces would not come out closed with only these parts of the cut. Take the whole plane, or move it a little."
                        ),
                        e => ctr!("CutRun", "The cut failed: {}", e.to_string()),
                    };
                    self.say(self.status_line(), why, false);
                }
            },
        }
    }

    /// The overlay for what is on show, redrawn.
    fn redraw(&self) {
        if self.stroke_mode.get() {
            self.show_stroke();
            return;
        }
        let Some((world, target)) = self.on_show() else {
            return;
        };
        let with_section = self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|s| s.settings == self.settings.get());
        self.show_overlay(&target, &world, with_section);
    }

    /// The plane on show and the model it cuts, once the model is ready.
    fn on_show(&self) -> Option<(Placed, Target)> {
        if !self.open.get() {
            return None;
        }
        let target = self.target.borrow().clone()?;
        self.ready
            .borrow()
            .as_ref()
            .filter(|r| Arc::ptr_eq(&r.mesh, &target.mesh))?;
        Some((placed(&self.settings.get(), &target)?, target))
    }

    /// New settings from the scene: the card and the section follow.
    fn take_settings(&self, settings: CutSettings) {
        if settings == self.settings.get() {
            return;
        }
        self.settings.set(settings);
        self.push_settings();
        self.refresh();
    }

    /// The pointer moved with no button down: the handle under it lights.
    /// Whether that changed.
    fn hover(&self, pointer: Option<&ScenePointer>) -> bool {
        if self.grab.borrow().is_some() || self.stroke_mode.get() {
            return false;
        }
        let hot = pointer.and_then(|p| self.on_show().and_then(|(at, _)| at.gizmo().pick(p)));
        self.hot.replace(hot) != hot
    }

    /// The gizmo to draw, and a ring's turn in flight.
    fn drawn_gizmo(&self) -> Option<([f64; 3], ToolGizmo, Option<RingTurn>)> {
        if self.stroke_mode.get() {
            return None;
        }
        let grab = self.grab.borrow();
        let (at, _) = self.on_show()?;
        let gizmo = at.gizmo();
        let active = grab.as_ref().map(|g| g.handle);
        let turn = grab.as_ref().and_then(|g| match g.handle {
            Handle::Ring(k) => {
                // Marked on the ring as it was grabbed.
                let (right, up) = g.gizmo.ring_plane(k);
                Some(RingTurn {
                    right,
                    up,
                    start: g.start,
                    sweep: g.swept,
                })
            }
            _ => None,
        });
        // While a ring turns, the gizmo stands as it was grabbed: its marks
        // and its ring stay put under the cursor.
        let gizmo = match grab.as_ref() {
            Some(g) if matches!(g.handle, Handle::Ring(_)) => g.gizmo.clone(),
            _ => gizmo,
        };
        Some((gizmo.center, gizmo.drawn(self.hot.get(), active), turn))
    }

    /// A left press: on a handle (or the plane), the drag is the tool's.
    fn press(&self, pointer: &ScenePointer) -> bool {
        if !self.editable() {
            return false;
        }
        if self.stroke_mode.get() {
            return self.stroke_press(pointer);
        }
        let Some((at, target)) = self.on_show() else {
            return false;
        };
        let gizmo = at.gizmo();
        let Some(handle) = gizmo.pick(pointer) else {
            return false;
        };
        let along = match handle {
            Handle::Arrow => pointer.along_line(gizmo.center, gizmo.n),
            _ => None,
        };
        let turn = match handle {
            Handle::Ring(k) => Turn::start(&gizmo, k, pointer),
            _ => None,
        };
        let start = match handle {
            Handle::Ring(k) => gizmo.ring_point_near(k, pointer).map_or(0.0, |(_, a, _)| a),
            _ => 0.0,
        };
        *self.grab.borrow_mut() = Some(Grab {
            handle,
            settings: self.settings.get(),
            hit: pointer.on_plane(gizmo.center, gizmo.n),
            at,
            gizmo,
            target,
            along,
            turn,
            start,
            swept: 0.0,
        });
        self.hot.set(Some(handle));
        true
    }

    /// The drag goes on: the plane follows the handle.
    fn drag(&self, pointer: &ScenePointer) {
        if self.drawing.get() {
            self.stroke_drag(pointer);
            return;
        }
        let next = {
            let mut grab = self.grab.borrow_mut();
            let Some(g) = grab.as_mut() else {
                return;
            };
            let all_corners = g.settings.corners.map(|c| g.at.world(c));
            match g.handle {
                Handle::Arrow => {
                    let (Some(from), Some(now)) =
                        (g.along, pointer.along_line(g.gizmo.center, g.gizmo.n))
                    else {
                        return;
                    };
                    let by = scale(g.gizmo.n, now - from);
                    posed(
                        &g.settings,
                        &g.target,
                        g.gizmo.n,
                        add(g.gizmo.center, by),
                        all_corners.map(|w| add(w, by)),
                    )
                }
                Handle::Ring(k) => {
                    let Some(turn) = g.turn.as_mut() else {
                        return;
                    };
                    let angle = turn.update(pointer.cursor);
                    g.swept = angle;
                    let (axis, pivot) = (g.gizmo.ring_axis(k), g.gizmo.center);
                    posed(
                        &g.settings,
                        &g.target,
                        turned_dir(g.gizmo.n, axis, angle),
                        pivot,
                        all_corners.map(|w| turned(w, pivot, axis, angle)),
                    )
                }
                Handle::Corner(k) => {
                    let Some(hit) = pointer.on_plane(g.gizmo.center, g.gizmo.n) else {
                        return;
                    };
                    let mut corners = g.at.corners;
                    corners[k] = g.at.flat(hit);
                    // A corner may not cross an edge: the shape stays where
                    // it last was simple.
                    if Outline::new(corners.to_vec()).is_none() {
                        return;
                    }
                    CutSettings {
                        bounded: true,
                        corners,
                        ..g.settings
                    }
                }
                Handle::Plane => {
                    let (Some(from), Some(now)) =
                        (g.hit, pointer.on_plane(g.gizmo.center, g.gizmo.n))
                    else {
                        return;
                    };
                    if !g.settings.bounded {
                        return;
                    }
                    let (a, b) = (g.at.flat(from), g.at.flat(now));
                    let by = [b[0] - a[0], b[1] - a[1]];
                    CutSettings {
                        corners: g.settings.corners.map(|c| [c[0] + by[0], c[1] + by[1]]),
                        ..g.settings
                    }
                }
            }
        };
        self.take_settings(next);
    }

    /// The press ends: kept, or (no pointer) given up, the plane back where
    /// it was. A press on the plane that did not move is a click on it.
    fn release(&self, pointer: Option<&ScenePointer>, moved: bool) {
        if self.drawing.get() {
            self.stroke_release(pointer);
            return;
        }
        let Some(grab) = self.grab.borrow_mut().take() else {
            return;
        };
        match pointer {
            None => self.take_settings(grab.settings),
            Some(p) if !moved && grab.handle == Handle::Plane => {
                self.click(p.origin, p.direction);
            }
            Some(_) => {
                if moved {
                    crate::breadcrumbs::note(&format!("cut: {:?} dragged", grab.handle));
                }
            }
        }
        self.hot
            .set(pointer.and_then(|p| self.on_show().and_then(|(at, _)| at.gizmo().pick(p))));
    }

    /// The plane, the pieces of the section with what is to happen to each,
    /// and the gizmo's handles.
    fn show_overlay(&self, _target: &Target, world: &Placed, with_section: bool) {
        let mut pieces: Vec<Arc<OverlayMesh>> = Vec::new();
        let corners = world.corners.map(|c| world.world(c).map(|x| x as f32));
        let [a, b, c, d, e, f] = quad_triangles(&world.corners);
        pieces.push(Arc::new(OverlayMesh {
            positions: corners.to_vec(),
            indices: vec![a, b, c, d, e, f, a, c, b, d, f, e],
            tint: OverlayTint::Cavity,
        }));
        if with_section && let Some(shown) = self.shown.borrow().as_ref() {
            let joined: &[usize] = shown.trial.as_ref().map_or(&[], |t| &t.joined_by);
            for (k, fit) in shown.fits.iter().enumerate() {
                let tint = if joined.contains(&k) {
                    OverlayTint::Invalid
                } else {
                    match fit {
                        Fit::Crossed => OverlayTint::Drain,
                        _ if shown.chosen[k] => OverlayTint::Valid,
                        _ => OverlayTint::Pending,
                    }
                };
                let (positions, indices) = shown.fills[k].clone();
                pieces.push(Arc::new(OverlayMesh {
                    positions,
                    indices,
                    tint,
                }));
            }
        }
        let overlay: Overlay = pieces.into();
        self.ctrl.set_preview_overlay(Some(overlay));
    }

    fn status_line(&self) -> slint::SharedString {
        let shown = self.shown.borrow();
        let Some(shown) = shown.as_ref() else {
            return Default::default();
        };
        let count = shown.fits.len();
        let taken = shown.chosen.iter().filter(|&&c| c).count();
        let crossed = shown.fits.iter().filter(|&&f| f == Fit::Crossed).count();
        let mut line = if taken == count {
            ctr!(
                "CutRun",
                "The plane crosses 1 part: it is cut.",
                "The plane crosses {n} parts: all are cut.",
                count
            )
            .to_string()
        } else {
            ctr!(
                "CutRun",
                "The plane crosses {} parts: {} cut, {} left whole.",
                count,
                taken,
                count - taken - crossed
            )
            .to_string()
        };
        if let Some(trial) = &shown.trial {
            line.push(' ');
            line.push_str(&match trial.pieces.len() {
                1 => ctr!("CutRun", "It leaves one piece.").to_string(),
                n => ctr!("CutRun", "It leaves {} pieces.", n).to_string(),
            });
            // The joints (#109, #442).
            let [made, left] = shown.joints;
            if made > 0 {
                line.push(' ');
                line.push_str(&ctr!("CutRun", "Joints added: {}.", made));
            }
            if left > 0 {
                line.push(' ');
                line.push_str(&ctr!(
                    "CutRun",
                    "Joints left out: {} (too near the edge of the cut, or too deep for the part).",
                    left
                ));
            }
        }
        // A suggestion on show (#443).
        if let Some(note) = self.suggestion_note() {
            line.push(' ');
            line.push_str(&note);
        }
        line.into()
    }

    /// Says what the cut on show does, and whether it can be done.
    fn describe(&self) {
        let (warning, can) = {
            let shown = self.shown.borrow();
            let Some(shown) = shown.as_ref() else {
                return;
            };
            let crossed = shown.fits.iter().filter(|&&f| f == Fit::Crossed).count();
            let taken = shown.chosen.iter().any(|&c| c);
            let one_piece = shown.trial.as_ref().is_some_and(|t| t.pieces.len() < 2);
            let ready_repaired = self.ready.borrow().as_ref().is_some_and(|r| r.repaired);
            let warning = if crossed > 0 {
                ctr!(
                    "CutRun",
                    "The plane's edge crosses a part (amber): cutting part of it would leave a slit. Move or resize the plane so it lies inside or outside."
                )
            } else if !taken {
                ctr!(
                    "CutRun",
                    "No part of the cut is taken. Click a part on the plane to take it."
                )
            } else if one_piece {
                ctr!(
                    "CutRun",
                    "This cut leaves one piece: the parts in red still join the two sides. Take them too, or make the plane reach them."
                )
            } else if ready_repaired {
                ctr!(
                    "CutRun",
                    "The model is not closed: it is repaired before it is cut."
                )
            } else {
                Default::default()
            };
            (warning, crossed == 0 && taken && !one_piece)
        };
        self.say(self.status_line(), warning, can);
    }

    fn say(&self, status: slint::SharedString, warning: slint::SharedString, can: bool) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_status(status);
            ui.set_cut_warning(warning);
            ui.set_cut_can_cut(can);
        }
    }

    /// A click in the viewport while the tool is open: on a part of the
    /// cut, takes it or leaves it. `false` lets the click select as usual.
    fn click(&self, origin: [f64; 3], direction: [f64; 3]) -> bool {
        if !self.open.get() {
            return false;
        }
        let picked = {
            let shown = self.shown.borrow();
            let Some(shown) = shown.as_ref() else {
                return false;
            };
            let Some(world) = placed(&shown.settings, &shown.target) else {
                return false;
            };
            let n = world.plane.normal();
            let along = dot(n, direction);
            if along.abs() < 1e-9 {
                return false;
            }
            let t = -world.plane.distance(origin) / along;
            if t < 0.0 {
                return false;
            }
            let hit = [0, 1, 2].map(|k| origin[k] + direction[k] * t);
            let Some(back) = shown.target.world.inverse() else {
                return false;
            };
            let Some(k) = shown.section.piece_at(back.apply(hit)) else {
                return false;
            };
            if shown.fits[k] == Fit::Crossed {
                return true;
            }
            (k, !shown.chosen[k])
        };
        let (k, take) = picked;
        self.toggled.borrow_mut().insert(k, take);
        if let Some(shown) = self.shown.borrow_mut().as_mut() {
            shown.chosen[k] = take;
            shown.trial = None;
            shown.trial_job = 0;
        }
        self.redraw();
        self.describe();
        self.schedule_trial();
        true
    }

    /// Cut: the model goes, its pieces come where it stood.
    pub(crate) fn apply(&self) {
        if !self.open.get() || !self.editable() {
            return;
        }
        // A stroke's cut (#440, #441), or the plane's.
        let picked = if self.stroke_mode.get() {
            match self.stroke_trial.borrow().as_ref() {
                Some(StrokeTrial {
                    target,
                    cut: Some(cut),
                    ..
                }) => Some((target.clone(), Some(cut.clone()))),
                _ => return,
            }
        } else {
            self.shown
                .borrow()
                .as_ref()
                .map(|s| (s.target.clone(), s.trial.clone()))
        };
        let Some((target, trial)) = picked else {
            return;
        };
        let Some(trial) = trial else {
            // Not worked out yet: it is, and is taken when it comes in.
            self.apply_when_in.set(true);
            self.settle.stop();
            self.start_trial();
            return;
        };
        if trial.pieces.len() < 2 {
            return;
        }
        let current = Target::selected(&self.document);
        if !current.is_some_and(|c| c.same(&target)) {
            self.refresh();
            return;
        }
        let leaf = target.leaf;
        let sessions = self.document.support_sessions();
        let Some((name, meta, supports, imported_gone)) = self
            .document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                // Imported supports (#651): each body goes with the piece it
                // holds up; those holding a piece other than the one most of
                // them hold (the one that keeps standing on them) go, as the
                // model's own supports do, instead of floating where it was.
                let imported = crate::imported_supports::imported_of(spatial, leaf, None);
                let tops = crate::imported_supports::body_tops(spatial, &imported);
                let points: Vec<[f64; 3]> = tops.iter().map(|(_, top)| *top).collect();
                let held = held_pieces(&trial.pieces, &target.world, &points);
                let mut counts = vec![0usize; trial.pieces.len()];
                for piece in held.iter().flatten() {
                    counts[*piece] += 1;
                }
                // On a tie, the lowest piece: the one nearest the plate.
                let low: Vec<f64> = trial
                    .pieces
                    .iter()
                    .map(|piece| {
                        piece
                            .vertices()
                            .iter()
                            .map(|v| target.world.apply(v.coords().map(f64::from))[2])
                            .fold(f64::INFINITY, f64::min)
                    })
                    .collect();
                let main = (0..counts.len())
                    .max_by(|&a, &b| counts[a].cmp(&counts[b]).then(low[b].total_cmp(&low[a])));
                let gone: Vec<crate::imported_supports::ImportedBody> = tops
                    .into_iter()
                    .zip(&held)
                    .filter(|(_, held)| held.is_some() && **held != main)
                    .map(|((body, _), _)| body)
                    .collect();
                Some((
                    leaf_name(graph, leaf),
                    graph.node(leaf)?.meta.clone(),
                    sets_of_model(spatial, &sessions, leaf),
                    gone,
                ))
            })
            .flatten()
        else {
            return;
        };
        let name = name.unwrap_or_else(|| ctr!("CutRun", "Object").to_string());
        let count = trial.pieces.len();
        let parts: Vec<(IndexedMesh, Option<SceneMetadata>)> = trial
            .pieces
            .iter()
            .enumerate()
            .map(|(k, mesh)| {
                let meta = SceneMetadata {
                    name: Some(ctr!("CutRun", "{} – part {}", name, k + 1).to_string()),
                    comment: meta.as_ref().and_then(|m| m.comment.clone()),
                };
                (mesh.clone(), Some(meta))
            })
            .collect();
        let slots = self.document.allocate_leaf_slots(count);
        let Some(edit) = self
            .document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                let split = split_edit(graph, leaf, parts, &slots)?;
                let mut edits = vec![split];
                if let Some(remove) = removal_edit(graph, &supports) {
                    edits.push(remove);
                }
                let gone: Vec<&crate::imported_supports::ImportedBody> =
                    imported_gone.iter().collect();
                if let Some(remove) = crate::imported_supports::removal_of_bodies(spatial, &gone) {
                    edits.push(remove);
                }
                Some(SceneEdit::Compound(edits))
            })
            .flatten()
        else {
            return;
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => {
                // Pieces of a model with Breadslicer supports get supports
                // again, in this Undo step, as a tilt does (#659, #153).
                if supports.is_empty() {
                    self.document.push_edit(edit);
                } else {
                    let pieces: Vec<_> = slots.iter().map(|slot| slot.0).collect();
                    self.document.push_edit_wanting(edit, &pieces);
                }
                self.document
                    .set_selection(Selection::One(LeafId(slots[0].0)));
                crate::breadcrumbs::note(&format!(
                    "cut into {count} pieces{}",
                    if trial.by_fallback { " (fallback)" } else { "" }
                ));
                self.close();
                if let Some(ui) = self.ui.upgrade() {
                    let mut notice = ctr!("CutRun", "Cut into {} pieces.", count).to_string();
                    if !supports.is_empty() {
                        notice.push(' ');
                        notice.push_str(&ctr!(
                            "CutRun",
                            "Supports are being made again for the pieces."
                        ));
                    }
                    if !imported_gone.is_empty() {
                        notice.push(' ');
                        notice.push_str(&ctr!(
                            "CutRun",
                            "{n} imported support held a piece cut off and was removed.",
                            "{n} imported supports held pieces cut off and were removed.",
                            imported_gone.len()
                        ));
                    }
                    ui.set_status_notice(notice.into());
                    let weak = ui.as_weak();
                    self.notice.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_secs(6),
                        move || {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_status_notice(Default::default());
                            }
                        },
                    );
                }
            }
            Err(EditNotApplied::NoSceneLoaded) => {}
            Err(EditNotApplied::Rejected(error)) => {
                let details = error.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("CutRun", "Cut"),
                    context: None,
                    message: ctr!("CutRun", "The cut could not be put in the scene."),
                    details: details.clone().into(),
                });
            }
        }
    }

    fn on_scene_changed(&self) {
        if !self.open.get() {
            return;
        }
        match Target::selected(&self.document) {
            None => self.close(),
            Some(target) => {
                let same = self
                    .target
                    .borrow()
                    .as_ref()
                    .is_some_and(|t| t.same(&target));
                if !same {
                    self.take_target(target, false);
                }
            }
        }
    }
}

/// What the viewport hands the tool: the pointer over the scene.
struct Handles(Weak<CutRun>);

impl CutHandles for Handles {
    fn active(&self) -> bool {
        self.0.upgrade().is_some_and(|run| run.open.get())
    }

    fn gizmo(&self) -> Option<([f64; 3], ToolGizmo, Option<RingTurn>)> {
        self.0.upgrade().and_then(|run| run.drawn_gizmo())
    }

    fn hover(&self, pointer: Option<&ScenePointer>) -> bool {
        self.0.upgrade().is_some_and(|run| run.hover(pointer))
    }

    fn press(&self, pointer: &ScenePointer) -> bool {
        self.0.upgrade().is_some_and(|run| run.press(pointer))
    }

    fn drag(&self, pointer: &ScenePointer) {
        if let Some(run) = self.0.upgrade() {
            run.drag(pointer);
        }
    }

    fn release(&self, pointer: Option<&ScenePointer>, moved: bool) {
        if let Some(run) = self.0.upgrade() {
            run.release(pointer, moved);
        }
    }

    fn click(&self, pointer: &ScenePointer) -> bool {
        self.0
            .upgrade()
            .is_some_and(|run| run.click(pointer.origin, pointer.direction))
    }
}

/// How far from a piece's surface, mm, the top of an imported support may be
/// and still hold it.
const HOLDS_WITHIN_MM: f64 = 1.5;

/// For each point of `tops` (world mm), the piece of `pieces` (in the mesh's
/// frame, `world` to the world) whose surface is nearest it, within
/// [`HOLDS_WITHIN_MM`]; `None` near none (#651).
fn held_pieces(pieces: &[IndexedMesh], world: &Affine, tops: &[[f64; 3]]) -> Vec<Option<usize>> {
    use glam::Vec3;
    let reach = HOLDS_WITHIN_MM;
    let cell = |c: f64| (c / reach).floor() as i64;
    let at = |p: [f64; 3]| p.map(cell);
    // The cells each top looks in, and which tops look in each.
    let mut wanted: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (t, &top) in tops.iter().enumerate() {
        let c = at(top);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    wanted
                        .entry([c[0] + dx, c[1] + dy, c[2] + dz])
                        .or_default()
                        .push(t);
                }
            }
        }
    }
    let mut best: Vec<Option<(f64, usize)>> = vec![None; tops.len()];
    if wanted.is_empty() {
        return Vec::new();
    }
    for (k, piece) in pieces.iter().enumerate() {
        let corners: Vec<[f64; 3]> = piece
            .vertices()
            .iter()
            .map(|v| world.apply(v.coords().map(f64::from)))
            .collect();
        for face in piece.faces() {
            let [a, b, c] = face.vertices.map(|v| corners[v.get() as usize]);
            let lo = [0, 1, 2].map(|i| cell(a[i].min(b[i]).min(c[i])));
            let hi = [0, 1, 2].map(|i| cell(a[i].max(b[i]).max(c[i])));
            let span = (0..3).map(|i| (hi[i] - lo[i] + 1) as u64).product::<u64>();
            let mut near: Vec<usize> = Vec::new();
            if span as usize > wanted.len() {
                for (key, ts) in &wanted {
                    if (0..3).all(|i| lo[i] <= key[i] && key[i] <= hi[i]) {
                        near.extend(ts);
                    }
                }
            } else {
                for x in lo[0]..=hi[0] {
                    for y in lo[1]..=hi[1] {
                        for z in lo[2]..=hi[2] {
                            if let Some(ts) = wanted.get(&[x, y, z]) {
                                near.extend(ts);
                            }
                        }
                    }
                }
            }
            if near.is_empty() {
                continue;
            }
            near.sort_unstable();
            near.dedup();
            let tri = [a, b, c].map(|p| Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32));
            for t in near {
                let q = tops[t];
                let q = Vec3::new(q[0] as f32, q[1] as f32, q[2] as f32);
                let d = f64::from(crate::island_paint::closest_on_triangle(q, tri).distance(q));
                if d <= reach && best[t].is_none_or(|(b, _)| d < b) {
                    best[t] = Some((d, k));
                }
            }
        }
    }
    best.into_iter().map(|b| b.map(|(_, k)| k)).collect()
}

#[cfg(test)]
#[path = "tests_cut_run.rs"]
mod tests;

/// The cut of `section` at the pieces `chosen` takes, with `joint` at the
/// middle of each of them (#109, #442), and how many joints it got and how
/// many were left out. A joint that the booleans could not make leaves the
/// cut as it is and is counted out.
fn with_joints(
    section: &Section,
    chosen: &[bool],
    joint: Option<Joint>,
) -> Result<(Cut, [usize; 2]), CutError> {
    let mut cut = section.cut(chosen)?;
    let Some(joint) = joint.filter(|_| cut.pieces.len() >= 2) else {
        return Ok((cut, [0, 0]));
    };
    let at = joint_points(section, chosen);
    match add_joints(&cut.pieces, section.plane(), &at, joint) {
        Ok(joined) => {
            cut.pieces = joined.pieces;
            Ok((cut, [joined.made, joined.skipped]))
        }
        Err(_) => Ok((cut, [0, at.len()])),
    }
}
