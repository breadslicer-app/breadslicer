/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Cuts a closed triangle mesh with a plane and closes the pieces (#56).
//!
//! [`Section::new`] finds where the plane crosses the mesh: closed loops of
//! points on the mesh's edges, grouped into the pieces of the cross-section
//! ([`SectionPiece`]), each an outline and the holes inside it. A cut takes
//! all of them or only some ([`Section::fits`] tells which lie inside a
//! shape on the plane, [`Outline`]): the faces at a piece left out stay whole, so
//! only the chosen pieces are cut. [`Section::cut`] splits the faces at the
//! chosen pieces, closes each side with a cap that keeps every point of the
//! loops, and hands back the pieces of the model the cut leaves, each closed.
//!
//! Each shell (a connected surface) is capped on its own: shells that
//! overlap have caps that overlap, as their volumes do. When a cut through
//! the whole plane does not come out closed (a shell that runs through
//! itself), it is done again with Manifold's plane split.

mod fallback;
mod section;
mod split;

#[cfg(test)]
mod tests;

use std::fmt;

use sc_mesh_core::IndexedMesh;

pub use section::{CutModel, Fit, Section, SectionPiece};

pub use split::{Cut, volume};

/// A plane: the points `p` with `normal · p = offset`, `normal` of length 1.
/// Its positive side is the one `normal` points to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    normal: [f64; 3],
    offset: f64,
}

impl Plane {
    /// The plane through `point` facing `normal`; `None` when `normal` has no
    /// length or is not finite.
    pub fn through(point: [f64; 3], normal: [f64; 3]) -> Option<Self> {
        let length = dot(normal, normal).sqrt();
        if !length.is_finite() || length < 1e-12 || !point.iter().all(|c| c.is_finite()) {
            return None;
        }
        let normal = normal.map(|c| c / length);
        Some(Self {
            normal,
            offset: dot(normal, point),
        })
    }

    pub fn normal(&self) -> [f64; 3] {
        self.normal
    }

    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// How far `point` lies from the plane, positive on the side `normal`
    /// points to.
    pub fn distance(&self, point: [f64; 3]) -> f64 {
        dot(self.normal, point) - self.offset
    }

    /// Two directions along the plane, of length 1 and at right angles,
    /// with `u × v = normal`: `u` is the x axis laid on the plane (the y axis
    /// when the plane is upright across x), so a level plane has `u` = x and
    /// `v` = y.
    pub fn frame(&self) -> ([f64; 3], [f64; 3]) {
        let n = self.normal;
        let along = |axis: [f64; 3]| {
            let d = dot(axis, n);
            let u = [axis[0] - n[0] * d, axis[1] - n[1] * d, axis[2] - n[2] * d];
            let length = dot(u, u).sqrt();
            (length > 1e-6).then(|| u.map(|c| c / length))
        };
        let u = along([1.0, 0.0, 0.0])
            .or_else(|| along([0.0, 1.0, 0.0]))
            .unwrap_or([1.0, 0.0, 0.0]);
        (u, cross(n, u))
    }
}

/// A shape on a plane that bounds a cut: a simple polygon (its edges meet
/// only at its corners), in the plane's own frame or any frame the caller
/// maps to ([`Section::fits`]). The Cut tool's four corners make one.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    points: Vec<[f64; 2]>,
}

impl Outline {
    /// The polygon through `points` in order, or `None` when it has fewer
    /// than three corners, no area, or edges that cross.
    pub fn new(points: Vec<[f64; 2]>) -> Option<Self> {
        (is_simple(&points) && polygon_area(&points).abs() > 1e-12).then_some(Self { points })
    }

    /// The rectangle around `center` reaching `half` along each direction.
    pub fn rect(center: [f64; 2], half: [f64; 2]) -> Self {
        let [x, y] = center;
        let [w, h] = half;
        Self {
            points: vec![
                [x - w, y - h],
                [x + w, y - h],
                [x + w, y + h],
                [x - w, y + h],
            ],
        }
    }

    pub fn points(&self) -> &[[f64; 2]] {
        &self.points
    }

    /// Whether `p` lies inside (even-odd).
    pub fn contains(&self, p: [f64; 2]) -> bool {
        let ring = &self.points;
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

    /// Whether the segment from `a` to `b` meets one of the edges.
    fn edge_meets(&self, a: [f64; 2], b: [f64; 2]) -> bool {
        let ring = &self.points;
        (0..ring.len()).any(|k| segments_meet(a, b, ring[k], ring[(k + 1) % ring.len()]))
    }
}

/// Twice the signed area of a polygon.
fn polygon_area(points: &[[f64; 2]]) -> f64 {
    (0..points.len())
        .map(|k| {
            let (p, q) = (points[k], points[(k + 1) % points.len()]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum()
}

/// Whether a polygon has three or more corners and no two of its edges meet
/// but neighbours at their shared corner.
pub fn is_simple(points: &[[f64; 2]]) -> bool {
    let n = points.len();
    if n < 3 || points.iter().flatten().any(|c| !c.is_finite()) {
        return false;
    }
    for i in 0..n {
        for j in i + 1..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            let (a, b) = (points[i], points[(i + 1) % n]);
            let (c, d) = (points[j], points[(j + 1) % n]);
            if segments_meet(a, b, c, d) {
                return false;
            }
        }
    }
    true
}

/// Whether segments `ab` and `cd` share a point.
fn segments_meet(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let side = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let within = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        r[0] >= p[0].min(q[0])
            && r[0] <= p[0].max(q[0])
            && r[1] >= p[1].min(q[1])
            && r[1] <= p[1].max(q[1])
    };
    let (d1, d2) = (side(c, d, a), side(c, d, b));
    let (d3, d4) = (side(a, b, c), side(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && within(c, d, a))
        || (d2 == 0.0 && within(c, d, b))
        || (d3 == 0.0 && within(a, b, c))
        || (d4 == 0.0 && within(a, b, d))
}

/// Why a mesh could not be cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CutError {
    /// The mesh is not closed: some edge does not have exactly two faces
    /// running opposite ways. Repair it first.
    NotClosed,
    /// The plane does not cross the mesh.
    Misses,
    /// No piece of the cross-section was chosen.
    NothingChosen,
    /// The pieces did not come out closed, and the cut does not take the
    /// whole plane, so it cannot be done again another way.
    NotClosedAfter,
    /// The fallback failed as well.
    Failed(String),
}

impl fmt::Display for CutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotClosed => f.write_str("the mesh is not closed"),
            Self::Misses => f.write_str("the plane does not cross the model"),
            Self::NothingChosen => f.write_str("no piece of the cross-section is chosen"),
            Self::NotClosedAfter => f.write_str("the pieces would not come out closed"),
            Self::Failed(why) => write!(f, "the cut failed: {why}"),
        }
    }
}

impl std::error::Error for CutError {}

/// Whether every edge of `mesh` has exactly two faces running opposite ways,
/// so it bounds a volume a plane can cut.
pub fn is_closed(mesh: &IndexedMesh) -> bool {
    let edges: Vec<u64> = mesh
        .faces()
        .iter()
        .flat_map(|f| {
            let t = f.vertices.map(|v| v.get());
            (0..3).map(move |k| edge(t[k], t[(k + 1) % 3]))
        })
        .collect();
    paired(edges)
}

/// A directed edge as one number.
fn edge(a: u32, b: u32) -> u64 {
    (u64::from(a) << 32) | u64::from(b)
}

/// Whether each of `edges` comes once and its reverse once.
fn paired(mut edges: Vec<u64>) -> bool {
    use rayon::slice::ParallelSliceMut;

    edges.par_sort_unstable();
    if edges.windows(2).any(|w| w[0] == w[1]) {
        return false;
    }
    edges
        .iter()
        .all(|&e| edges.binary_search(&e.rotate_right(32)).is_ok())
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
