/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Joints across a cut: assembly keys (#109) and magnet slots (#442).
//!
//! A cut leaves two faces that meet again when the parts are glued. A
//! **pin** on the part below the plane and a **socket** a clearance wider
//! on the part above align and lock them; a **magnet slot** is a pocket on
//! each side, facing each other, each the size of a magnet and a clearance.
//! Each joint stands at a point of the cut ([`joint_points`]: the middle of
//! each piece of the cross-section the cut took), along the plane's
//! normal.
//!
//! A joint is only made where it fits: its hole keeps [`MIN_WALL_MM`] of
//! the part around it and under its floor, measured on the parts
//! themselves; one that does not fit is left out and counted. The parts
//! are joined and hollowed with Manifold's booleans, so they come out
//! closed.

use manifold_rust::linalg::{Mat3x4, Vec3};
use manifold_rust::manifold::Manifold;
use manifold_rust::types::{Error, MeshGL};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

use crate::section::Section;
use crate::{CutError, Plane, cross, dot, is_closed};

/// The least of the part a hole leaves around it and under its floor, mm.
pub const MIN_WALL_MM: f64 = 0.8;
/// How far a pin reaches into its own part below the plane, so the two fuse
/// into one solid, mm.
const PIN_ROOT_MM: f64 = 0.5;
/// How far a hole opens past the plane, so it cuts through the cap cleanly,
/// mm.
const OPEN_MM: f64 = 0.2;
/// Sides of a round pin or hole.
const SEGMENTS: i32 = 48;
/// Common magnet sizes, diameter × height, mm (#442).
pub const MAGNET_PRESETS: [(f64, f64); 3] = [(2.0, 1.0), (3.0, 2.0), (5.0, 2.0)];

/// The shape of a pin and its socket (#109).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinShape {
    Round,
    Square,
}

/// What joins the parts across a cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Joint {
    /// A pin on the part below the plane, a socket on the part above (#109).
    Pin {
        shape: PinShape,
        /// Its diameter, or its side when square, mm.
        size_mm: f64,
        /// How far it reaches past the plane, mm.
        depth_mm: f64,
        /// How much wider and deeper the socket is, mm.
        clearance_mm: f64,
    },
    /// A pocket for a magnet on each side (#442).
    Magnet {
        diameter_mm: f64,
        height_mm: f64,
        /// How much wider and deeper each pocket is than the magnet, mm.
        clearance_mm: f64,
    },
}

impl Joint {
    /// The default pin: round, 3 mm across and deep, 0.15 mm clearance.
    pub const fn default_pin() -> Self {
        Joint::Pin {
            shape: PinShape::Round,
            size_mm: 3.0,
            depth_mm: 3.0,
            clearance_mm: 0.15,
        }
    }

    /// The default magnet slot: a 3 × 2 mm magnet, 0.1 mm clearance.
    pub const fn default_magnet() -> Self {
        Joint::Magnet {
            diameter_mm: 3.0,
            height_mm: 2.0,
            clearance_mm: 0.1,
        }
    }

    /// The widest the joint's hole is, mm: what must fit in the section.
    fn hole_radius(&self) -> f64 {
        match *self {
            Joint::Pin {
                shape,
                size_mm,
                clearance_mm,
                ..
            } => {
                let half = size_mm / 2.0 + clearance_mm;
                match shape {
                    PinShape::Round => half,
                    PinShape::Square => half * std::f64::consts::SQRT_2,
                }
            }
            Joint::Magnet {
                diameter_mm,
                clearance_mm,
                ..
            } => diameter_mm / 2.0 + clearance_mm,
        }
    }

    fn valid(&self) -> bool {
        let positive = |v: f64| v.is_finite() && v > 0.0;
        match *self {
            Joint::Pin {
                size_mm,
                depth_mm,
                clearance_mm,
                ..
            } => positive(size_mm) && positive(depth_mm) && clearance_mm >= 0.0,
            Joint::Magnet {
                diameter_mm,
                height_mm,
                clearance_mm,
            } => positive(diameter_mm) && positive(height_mm) && clearance_mm >= 0.0,
        }
    }
}

/// Where the joints of a cut stand: the middle of each piece of the
/// cross-section `chosen` takes, on the plane, in the mesh's frame. The
/// middle of a piece is its area's centroid, or when that falls outside
/// it (a ring, a C) the middle of its widest triangle.
pub fn joint_points(section: &Section, chosen: &[bool]) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for k in 0..section.pieces().len() {
        if !chosen.get(k).copied().unwrap_or(false) {
            continue;
        }
        let (points, indices) = section.fill(k);
        let mut sum = [0.0; 3];
        let mut area = 0.0;
        let mut widest: Option<(f64, [f64; 3])> = None;
        for t in indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| points[i as usize].map(f64::from));
            let n = cross(sub(b, a), sub(c, a));
            let twice = dot(n, n).sqrt();
            let mid = [0, 1, 2].map(|i| (a[i] + b[i] + c[i]) / 3.0);
            for i in 0..3 {
                sum[i] += mid[i] * twice;
            }
            area += twice;
            if widest.is_none_or(|(w, _)| twice > w) {
                widest = Some((twice, mid));
            }
        }
        if area <= 0.0 {
            continue;
        }
        let centroid = sum.map(|v| v / area);
        let at = if section.piece_at(centroid) == Some(k) {
            centroid
        } else if let Some((_, mid)) = widest {
            mid
        } else {
            continue;
        };
        out.push(at);
    }
    out
}

/// The parts of a cut with their joints.
#[derive(Debug)]
pub struct Joined {
    pub pieces: Vec<IndexedMesh>,
    /// Joints made.
    pub made: usize,
    /// Joints left out: too close to the section's edge, or too deep for
    /// the part behind them.
    pub skipped: usize,
}

/// Adds `joint` at each of `at` (points on `plane`, in the frame of the
/// pieces) to the `pieces` of a cut by `plane`.
pub fn add_joints(
    pieces: &[IndexedMesh],
    plane: Plane,
    at: &[[f64; 3]],
    joint: Joint,
) -> Result<Joined, CutError> {
    if !joint.valid() {
        return Err(CutError::Failed("the joint has no size".into()));
    }
    let mut pieces: Vec<IndexedMesh> = pieces.to_vec();
    let n = plane.normal();
    let (u, v) = plane.frame();
    let (mut made, mut skipped) = (0, 0);
    for &point in at {
        // Snap the point onto the plane.
        let d = plane.distance(point);
        let p = [0, 1, 2].map(|i| point[i] - n[i] * d);
        let probe = 0.05;
        let below = pieces
            .iter()
            .position(|m| inside(m, add(p, scale(n, -probe))));
        let above = pieces
            .iter()
            .position(|m| inside(m, add(p, scale(n, probe))));
        let (Some(below), Some(above)) = (below, above) else {
            skipped += 1;
            continue;
        };
        if below == above {
            skipped += 1;
            continue;
        }
        // The hole and a wall around it lie inside the section, both sides.
        let ring = joint.hole_radius() + MIN_WALL_MM;
        let fits = (0..16).all(|k| {
            let (s, c) = (k as f64 * std::f64::consts::TAU / 16.0).sin_cos();
            let q = add(p, add(scale(u, ring * c), scale(v, ring * s)));
            inside(&pieces[below], add(q, scale(n, -probe)))
                && inside(&pieces[above], add(q, scale(n, probe)))
        });
        if !fits {
            skipped += 1;
            continue;
        }
        let (into_above, into_below) = match joint {
            Joint::Pin {
                depth_mm,
                clearance_mm,
                ..
            } => (depth_mm + clearance_mm, 0.0),
            Joint::Magnet {
                height_mm,
                clearance_mm,
                ..
            } => (height_mm + clearance_mm, height_mm + clearance_mm),
        };
        // Each hole keeps a floor under it.
        let deep_enough = |piece: &IndexedMesh, dir: [f64; 3], depth: f64| {
            depth <= 0.0
                || thickness(piece, add(p, scale(dir, probe)), dir)
                    .is_some_and(|t| t + probe >= depth + MIN_WALL_MM)
        };
        if !deep_enough(&pieces[above], n, into_above)
            || !deep_enough(&pieces[below], scale(n, -1.0), into_below)
        {
            skipped += 1;
            continue;
        }
        let frame = Frame { u, v, n };
        let (new_below, new_above) = match joint {
            Joint::Pin {
                shape,
                size_mm,
                depth_mm,
                clearance_mm,
            } => {
                let pin = prism(shape, size_mm / 2.0, &frame, p, -PIN_ROOT_MM, depth_mm);
                let socket = prism(
                    shape,
                    size_mm / 2.0 + clearance_mm,
                    &frame,
                    p,
                    -OPEN_MM,
                    depth_mm + clearance_mm,
                );
                (
                    boolean(&pieces[below], &pin, true)?,
                    boolean(&pieces[above], &socket, false)?,
                )
            }
            Joint::Magnet {
                diameter_mm,
                height_mm,
                clearance_mm,
            } => {
                let r = diameter_mm / 2.0 + clearance_mm;
                let depth = height_mm + clearance_mm;
                let low = prism(PinShape::Round, r, &frame, p, -depth, OPEN_MM);
                let high = prism(PinShape::Round, r, &frame, p, -OPEN_MM, depth);
                (
                    boolean(&pieces[below], &low, false)?,
                    boolean(&pieces[above], &high, false)?,
                )
            }
        };
        pieces[below] = new_below;
        pieces[above] = new_above;
        made += 1;
    }
    Ok(Joined {
        pieces,
        made,
        skipped,
    })
}

/// The plane's frame: two directions along it and its normal.
struct Frame {
    u: [f64; 3],
    v: [f64; 3],
    n: [f64; 3],
}

/// A round or square prism of half-width `half` along the frame's normal,
/// from `from` to `to` past `at`.
fn prism(shape: PinShape, half: f64, frame: &Frame, at: [f64; 3], from: f64, to: f64) -> Manifold {
    let height = to - from;
    let solid = match shape {
        PinShape::Round => Manifold::cylinder(height, half, half, SEGMENTS),
        PinShape::Square => Manifold::cube(Vec3::new(2.0 * half, 2.0 * half, height), false)
            .translate(Vec3::new(-half, -half, 0.0)),
    };
    let start = add(at, scale(frame.n, from));
    let v3 = |a: [f64; 3]| Vec3::new(a[0], a[1], a[2]);
    solid.transform(&Mat3x4 {
        x: v3(frame.u),
        y: v3(frame.v),
        z: v3(frame.n),
        w: v3(start),
    })
}

fn to_manifold(mesh: &IndexedMesh) -> Result<Manifold, CutError> {
    let gl = MeshGL {
        num_prop: 3,
        vert_properties: mesh
            .vertices()
            .iter()
            .flat_map(|v| v.as_array().iter().copied())
            .collect(),
        tri_verts: mesh
            .faces()
            .iter()
            .flat_map(|f| f.vertices.map(|v| v.get()))
            .collect(),
        ..Default::default()
    };
    let mut solid = Manifold::from_mesh_gl(&gl);
    if solid.status() != Error::NoError {
        solid = Manifold::from_mesh_gl_robust(&gl);
    }
    if solid.status() != Error::NoError {
        return Err(CutError::Failed(format!("{:?}", solid.status())));
    }
    Ok(solid)
}

fn from_manifold(solid: &Manifold) -> Result<IndexedMesh, CutError> {
    if solid.status() != Error::NoError {
        return Err(CutError::Failed(format!("{:?}", solid.status())));
    }
    let gl = solid.get_mesh_gl(-1);
    let stride = gl.num_prop as usize;
    let mesh = IndexedMesh::new(
        None,
        gl.vert_properties
            .chunks_exact(stride)
            .map(|p| Vertex::new([p[0], p[1], p[2]]))
            .collect(),
        gl.tri_verts
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&t| IndexedTriangle::from_raw_indices(t))
            .collect(),
    );
    if !is_closed(&mesh) {
        return Err(CutError::Failed(
            "a part with its joint is not closed".into(),
        ));
    }
    Ok(mesh)
}

/// `mesh` with `tool` added (`join`) or taken out.
fn boolean(mesh: &IndexedMesh, tool: &Manifold, join: bool) -> Result<IndexedMesh, CutError> {
    let solid = to_manifold(mesh)?;
    let out = if join {
        solid.union(tool)
    } else {
        solid.difference(tool)
    };
    from_manifold(&out)
}

/// Whether `p` lies inside the closed `mesh`: a ray from it crosses the
/// surface an odd number of times.
fn inside(mesh: &IndexedMesh, p: [f64; 3]) -> bool {
    // Off the axes, so it never runs along an edge.
    let dir = normalize([0.578_9, 0.326_141, 0.748_683]);
    crossings(mesh, p, dir).len() % 2 == 1
}

/// How far a ray from `p` along `dir` (inside `mesh`) runs before it leaves
/// the mesh: the nearest crossing. `None` when it crosses nothing.
fn thickness(mesh: &IndexedMesh, p: [f64; 3], dir: [f64; 3]) -> Option<f64> {
    crossings(mesh, p, dir).into_iter().reduce(f64::min)
}

/// The distances along the ray from `p` along `dir` at which it crosses a
/// triangle of `mesh`.
fn crossings(mesh: &IndexedMesh, p: [f64; 3], dir: [f64; 3]) -> Vec<f64> {
    let vertices = mesh.vertices();
    let mut out = Vec::new();
    for f in mesh.faces() {
        let [a, b, c] = f
            .vertices
            .map(|v| vertices[v.get() as usize].as_array().map(f64::from));
        let e1 = sub(b, a);
        let e2 = sub(c, a);
        let h = cross(dir, e2);
        let det = dot(e1, h);
        if det.abs() < 1e-14 {
            continue;
        }
        let inv = 1.0 / det;
        let s = sub(p, a);
        let bu = inv * dot(s, h);
        if !(0.0..=1.0).contains(&bu) {
            continue;
        }
        let q = cross(s, e1);
        let bv = inv * dot(dir, q);
        if bv < 0.0 || bu + bv > 1.0 {
            continue;
        }
        let t = inv * dot(e2, q);
        if t > 1e-9 {
            out.push(t);
        }
    }
    out
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    a.map(|c| c * s)
}

fn normalize(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    a.map(|c| c / l)
}
