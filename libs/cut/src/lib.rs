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
//! rectangle on the plane): the faces at a piece left out stay whole, so
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

/// A rectangle on a plane: its centre, and how far it reaches from it along
/// the plane's [`Plane::frame`] directions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// The centre, in the plane's frame.
    pub center: [f64; 2],
    /// Half the width along `u` and half the height along `v`.
    pub half: [f64; 2],
}

impl Rect {
    fn contains(&self, p: [f64; 2]) -> bool {
        (p[0] - self.center[0]).abs() <= self.half[0]
            && (p[1] - self.center[1]).abs() <= self.half[1]
    }

    fn corners(&self) -> [[f64; 2]; 4] {
        let [x, y] = self.center;
        let [w, h] = self.half;
        [
            [x - w, y - h],
            [x + w, y - h],
            [x + w, y + h],
            [x - w, y + h],
        ]
    }
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
