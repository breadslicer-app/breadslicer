/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Cuts along a stroke drawn over the model in the view (#440), and lasso
//! cuts (#441).
//!
//! The stroke is drawn on screen: its points lie in a frame across the view
//! direction, and the cut is the stroke swept along that direction through
//! the model.
//!
//! - **An open stroke** (#440) cuts the model in two along it: each end runs
//!   on straight past the model, and the side on the stroke's left closes it
//!   into a region. A straight stroke is a plane cut; a curved one follows
//!   the curve.
//! - **A closed stroke**, a lasso (#441), cuts off what it encloses, down to
//!   the plane that best fits the lasso's points on the surface: a shoulder
//!   pad, a hand, a head. The cut there is flat.
//!
//! The region is a prism (the stroke extruded along the view), and for a
//! lasso that prism cut by the plane; the pieces are the model's part
//! inside it and the part outside, made with Manifold's booleans, so each
//! comes out closed with its cap.

use manifold_rust::linalg::{Mat3x4, Vec2, Vec3};
use manifold_rust::manifold::Manifold;
use manifold_rust::types::{Error, MeshGL};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

use crate::split::Cut;
use crate::{CutError, cross, dot, is_closed, is_simple, polygon_area};

/// A stroke over the model, in a frame across the direction it sweeps
/// along.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    /// The frame's origin, its two directions across the sweep (unit, at
    /// right angles) and the sweep's direction, away from the eye; `u × v`
    /// is `along`.
    pub origin: [f64; 3],
    pub u: [f64; 3],
    pub v: [f64; 3],
    pub along: [f64; 3],
    /// The stroke's points in the frame.
    pub points: Vec<[f64; 2]>,
    /// For a lasso (#441): the points where it lies on the surface, for the
    /// plane its cut stops at. Empty for an open stroke.
    pub surface: Vec<[f64; 3]>,
    /// A lasso (#441), or an open stroke (#440).
    pub closed: bool,
}

impl Stroke {
    /// Two directions across `along`, unit and at right angles, with
    /// `u × v = along`.
    pub fn frame(along: [f64; 3]) -> ([f64; 3], [f64; 3]) {
        let a = normalize(along);
        let helper = if a[2].abs() < 0.9 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let u = normalize(cross(helper, a));
        let v = cross(a, u);
        (u, v)
    }

    /// `p` in the frame.
    pub fn flat(&self, p: [f64; 3]) -> [f64; 2] {
        let d = sub(p, self.origin);
        [dot(d, self.u), dot(d, self.v)]
    }
}

/// Cuts `mesh` along `stroke`: the part inside the stroke's region first,
/// then the rest.
pub fn stroke_cut(mesh: &IndexedMesh, stroke: &Stroke) -> Result<Cut, CutError> {
    let points = tidy(&stroke.points);
    if points.len() < 2 || (stroke.closed && points.len() < 3) {
        return Err(CutError::Misses);
    }
    // The model's reach in the frame. The frame may be skewed (a stroke
    // taken into a scaled model's own axes): its coordinates come from the
    // inverse of its axes.
    let inverse = invert([stroke.u, stroke.v, stroke.along]).ok_or(CutError::Misses)?;
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for v in mesh.vertices() {
        let p = v.as_array().map(f64::from);
        let d = sub(p, stroke.origin);
        let q = [0, 1, 2].map(|k| dot(inverse[k], d));
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    if !lo[0].is_finite() {
        return Err(CutError::Misses);
    }
    let size = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max).max(1.0);
    let reach = size * 4.0
        + points
            .iter()
            .map(|p| p[0].abs().max(p[1].abs()))
            .fold(0.0, f64::max);
    let polygon = if stroke.closed {
        points.clone()
    } else {
        open_region(&points, reach)
    };
    if !is_simple(&polygon) {
        return Err(CutError::Failed("the stroke crosses itself".into()));
    }
    let mut polygon = polygon;
    if polygon_area(&polygon) < 0.0 {
        polygon.reverse();
    }
    // The prism, from in front of the model to past its back.
    let margin = size * 0.5 + 1.0;
    let (near, far) = (lo[2] - margin, hi[2] + margin);
    let section = vec![
        polygon
            .iter()
            .map(|p| Vec2::new(p[0], p[1]))
            .collect::<Vec<Vec2>>(),
    ];
    let prism = Manifold::extrude(&section, far - near, 0, 0.0, Vec2::new(1.0, 1.0));
    if prism.status() != Error::NoError || prism.is_empty() {
        return Err(CutError::Failed("the stroke makes no region".into()));
    }
    let start = add(stroke.origin, scale(stroke.along, near));
    let v3 = |a: [f64; 3]| Vec3::new(a[0], a[1], a[2]);
    let mut region = prism.transform(&Mat3x4 {
        x: v3(stroke.u),
        y: v3(stroke.v),
        z: v3(stroke.along),
        w: v3(start),
    });
    if stroke.closed {
        // The lasso's region stops at the plane through its points.
        // When its points on the surface lie along a line (a loop drawn
        // mostly off the model), no plane fits them: the plane across the
        // view through them stands in.
        let (centre, normal) = match best_plane(&stroke.surface) {
            Some(fit) => fit,
            None if !stroke.surface.is_empty() => {
                let n = stroke.surface.len() as f64;
                let c = stroke
                    .surface
                    .iter()
                    .fold([0.0; 3], |a, p| add(a, *p))
                    .map(|x| x / n);
                (c, scale(normalize(stroke.along), -1.0))
            }
            None => return Err(CutError::Misses),
        };
        // Facing the eye: what lies in front of the plane is cut off.
        let normal = if dot(normal, stroke.along) > 0.0 {
            scale(normal, -1.0)
        } else {
            normal
        };
        let (pu, pv) = Stroke::frame(normal);
        let big = reach * 4.0;
        let half = Manifold::cube(Vec3::new(2.0 * big, 2.0 * big, big), false)
            .translate(Vec3::new(-big, -big, 0.0))
            .transform(&Mat3x4 {
                x: v3(pu),
                y: v3(pv),
                z: v3(normal),
                w: v3(centre),
            });
        region = region.intersection(&half);
    }
    let model_id = Manifold::reserve_ids(1);
    let solid = to_manifold(mesh, model_id)?;
    let inside = solid.intersection(&region);
    let outside = solid.difference(&region);
    let cap = faces_not_from(&inside, model_id);
    let mut pieces = Vec::new();
    for part in [inside, outside] {
        if part.status() != Error::NoError {
            return Err(CutError::Failed(format!("{:?}", part.status())));
        }
        if part.is_empty() || part.volume() < 1e-9 {
            return Err(CutError::Misses);
        }
        pieces.push(from_manifold(&part)?);
    }
    Ok(Cut {
        pieces,
        joined_by: Vec::new(),
        by_fallback: false,
        cap,
    })
}

/// The triangles of `solid` that do not come from the mesh with
/// `original`: where a boolean's other operand closed it.
fn faces_not_from(solid: &Manifold, original: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let gl = solid.get_mesh_gl(-1);
    let stride = gl.num_prop as usize;
    let tris = gl.tri_verts.len() / 3;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut moved: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for (run, &id) in gl.run_original_id.iter().enumerate() {
        if id == original {
            continue;
        }
        let start = gl.run_index.get(run).map_or(0, |&i| i as usize / 3);
        let end = gl
            .run_index
            .get(run + 1)
            .map_or(tris, |&i| i as usize / 3)
            .min(tris);
        for t in start..end {
            for &v in &gl.tri_verts[3 * t..3 * t + 3] {
                let at = *moved.entry(v).or_insert_with(|| {
                    let p = &gl.vert_properties[v as usize * stride..];
                    positions.push([p[0], p[1], p[2]]);
                    positions.len() as u32 - 1
                });
                indices.push(at);
            }
        }
    }
    (positions, indices)
}

/// An open stroke closed into a region: its ends run on straight past the
/// model by `reach`, and the stroke's left side closes it.
fn open_region(points: &[[f64; 2]], reach: f64) -> Vec<[f64; 2]> {
    let n = points.len();
    let dir = |a: [f64; 2], b: [f64; 2]| {
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-12);
        [d[0] / l, d[1] / l]
    };
    let head = dir(points[1], points[0]);
    let tail = dir(points[n - 2], points[n - 1]);
    let along = dir(points[0], points[n - 1]);
    let left = [-along[1], along[0]];
    let start = [
        points[0][0] + head[0] * reach,
        points[0][1] + head[1] * reach,
    ];
    let end = [
        points[n - 1][0] + tail[0] * reach,
        points[n - 1][1] + tail[1] * reach,
    ];
    let mut out = vec![start];
    out.extend_from_slice(points);
    out.push(end);
    out.push([
        end[0] + left[0] * reach * 2.0,
        end[1] + left[1] * reach * 2.0,
    ]);
    out.push([
        start[0] + left[0] * reach * 2.0,
        start[1] + left[1] * reach * 2.0,
    ]);
    out
}

/// The stroke without points closer than a hundredth of a millimetre to
/// the one before, and for a lasso without the last when it closes on the
/// first.
fn tidy(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::new();
    for &p in points {
        if !p.iter().all(|c| c.is_finite()) {
            continue;
        }
        if out
            .last()
            .is_none_or(|q| (p[0] - q[0]).hypot(p[1] - q[1]) > 0.01)
        {
            out.push(p);
        }
    }
    if out.len() > 2 {
        let (a, b) = (out[0], out[out.len() - 1]);
        if (a[0] - b[0]).hypot(a[1] - b[1]) <= 0.01 {
            out.pop();
        }
    }
    out
}

/// The plane that best fits `points`: their middle and the direction they
/// spread least along (the smallest eigenvector of their scatter). `None`
/// for fewer than three points, or points along a line.
fn best_plane(points: &[[f64; 3]]) -> Option<([f64; 3], [f64; 3])> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let c = points
        .iter()
        .fold([0.0; 3], |a, p| add(a, *p))
        .map(|x| x / n);
    let mut m = [[0.0; 3]; 3];
    for p in points {
        let d = sub(*p, c);
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += d[i] * d[j];
            }
        }
    }
    // The normal: the cross product of the two widest spreads, found by
    // power iteration on the scatter and on what is left of it.
    let power = |m: &[[f64; 3]; 3], seed: [f64; 3]| {
        let mut x = seed;
        for _ in 0..64 {
            let y = [0, 1, 2].map(|i| dot(m[i], x));
            let l = dot(y, y).sqrt();
            if l < 1e-18 {
                return None;
            }
            x = y.map(|v| v / l);
        }
        Some(x)
    };
    let e1 = power(&m, [0.577, 0.577, 0.577])?;
    let l1 = dot(e1, [0, 1, 2].map(|i| dot(m[i], e1)));
    let mut rest = m;
    for i in 0..3 {
        for j in 0..3 {
            rest[i][j] -= l1 * e1[i] * e1[j];
        }
    }
    let seed = normalize(cross(e1, [0.31, 0.57, 0.76]));
    let e2 = power(&rest, seed)?;
    // Spread along a line only: no plane to speak of.
    let l2 = dot(e2, [0, 1, 2].map(|i| dot(rest[i], e2)));
    if l2 < 1e-3 * l1 {
        return None;
    }
    let normal = cross(e1, e2);
    let l = dot(normal, normal).sqrt();
    (l > 1e-9).then(|| (c, normal.map(|v| v / l)))
}

fn to_manifold(mesh: &IndexedMesh, original: u32) -> Result<Manifold, CutError> {
    let gl = MeshGL {
        num_prop: 3,
        run_original_id: vec![original],
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
        return Err(CutError::NotClosed);
    }
    Ok(solid)
}

fn from_manifold(solid: &Manifold) -> Result<IndexedMesh, CutError> {
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
        return Err(CutError::NotClosedAfter);
    }
    Ok(mesh)
}

/// The rows of the inverse of the matrix whose columns are `axes`, or
/// `None` when they are flat.
fn invert(axes: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let [a, b, c] = axes;
    let det = dot(a, cross(b, c));
    if det.abs() < 1e-18 {
        return None;
    }
    Some([cross(b, c), cross(c, a), cross(a, b)].map(|r| r.map(|x| x / det)))
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
    if l < 1e-18 { a } else { a.map(|c| c / l) }
}
