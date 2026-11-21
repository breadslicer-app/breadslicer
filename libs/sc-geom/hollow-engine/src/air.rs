//! Which air around a mesh is a cavity and which is outside (#447).
//!
//! A cavity drained through a small hole is, by topology, outside: the hole
//! joins them. What tells it from the gap between a leg and the body is
//! size: a ball of [`BALL_MM`] radius rolls in from outside into a wide gap
//! but not through a drain, and fits inside a cavity, while a thin slit
//! (a cape a few millimetres off the body) holds no such ball at all. So a
//! cavity is the air covered by balls of that radius that fit inside and
//! that no ball from outside reaches; a slit is neither, and is never
//! drilled into. Measured on a voxel grid.

use std::collections::VecDeque;

use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::mesh::{IndexedMesh, Vec3};

/// The radius of the ball that tells a cavity from the outside: wider than
/// the drains a model is usually drained by (a 3 mm hole lets no 4 mm ball
/// through), thicker than a slit between a figure's parts.
pub(crate) const BALL_MM: f32 = 2.0;
/// The finest voxel the map uses.
const FINEST_MM: f32 = 0.4;
/// At most this many voxels: a larger model gets coarser ones.
const MAX_CELLS: f64 = 8.0e6;

/// The air around a mesh, voxel by voxel: whether each is in a cavity.
#[derive(Debug)]
pub(crate) struct AirMap {
    origin: Vec3,
    cell: f32,
    dims: [usize; 3],
    /// Per voxel: 0 solid, 1 outside, 2 cavity.
    class: Vec<u8>,
}

const SOLID: u8 = 0;
const OUTSIDE: u8 = 1;
const CAVITY: u8 = 2;

impl AirMap {
    pub(crate) fn new(mesh: &IndexedMesh, bvh: &Bvh) -> Self {
        let bbox = mesh.bbox();
        if mesh.triangles.is_empty() || !bbox.min.finite() || !bbox.max.finite() {
            return Self {
                origin: Vec3::ZERO,
                cell: 1.0,
                dims: [0; 3],
                class: Vec::new(),
            };
        }
        let extent = bbox.max.sub(bbox.min);
        let box_volume = |pad: f32| {
            f64::from(extent.x + 2.0 * pad)
                * f64::from(extent.y + 2.0 * pad)
                * f64::from(extent.z + 2.0 * pad)
        };
        let mut cell = FINEST_MM;
        loop {
            let pad = BALL_MM + 3.0 * cell;
            let cells = box_volume(pad) / f64::from(cell).powi(3);
            if cells <= MAX_CELLS {
                break;
            }
            cell *= (cells / MAX_CELLS).cbrt() as f32 * 1.01;
        }
        let pad = BALL_MM + 3.0 * cell;
        let origin = bbox.min.sub(Vec3::new(pad, pad, pad));
        let dims =
            [extent.x, extent.y, extent.z].map(|e| ((e + 2.0 * pad) / cell).ceil() as usize + 1);
        let [nx, ny, nz] = dims;
        let index = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;

        // Inside or not, row by row along x: one ray per row, by parity.
        let mut class = vec![OUTSIDE; nx * ny * nz];
        class
            .par_chunks_mut(nx)
            .enumerate()
            .for_each(|(row, cells)| {
                let (j, k) = (row % ny, row / ny);
                // A hair off the grid, so rows do not run along edges.
                let y = origin.y + (j as f32 + 0.5) * cell + 1.3e-4;
                let z = origin.z + (k as f32 + 0.5) * cell + 0.7e-4;
                let start = Vec3::new(origin.x - 1.0, y, z);
                let mut hits = bvh.ray_hit_distances(mesh, start, Vec3::new(1.0, 0.0, 0.0));
                hits.dedup_by(|a, b| (*a - *b).abs() < 1e-5);
                let mut next = 0;
                for (i, c) in cells.iter_mut().enumerate() {
                    let t = 1.0 + (i as f32 + 0.5) * cell;
                    while next < hits.len() && hits[next] < t {
                        next += 1;
                    }
                    if next % 2 == 1 {
                        *c = SOLID;
                    }
                }
            });

        // Distance to the solid, then where a ball of BALL_MM fits.
        let from_solid = chamfer(dims, cell, |n| class[n] == SOLID);
        let roomy: Vec<bool> = from_solid.iter().map(|&d| d > BALL_MM).collect();
        // The roomy air reached from the edge of the grid.
        let mut reached = vec![false; class.len()];
        let mut queue = VecDeque::new();
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let edge =
                        i == 0 || j == 0 || k == 0 || i + 1 == nx || j + 1 == ny || k + 1 == nz;
                    let n = index(i, j, k);
                    if edge && roomy[n] {
                        reached[n] = true;
                        queue.push_back((i, j, k));
                    }
                }
            }
        }
        while let Some((i, j, k)) = queue.pop_front() {
            let neighbours = [
                (i.wrapping_sub(1), j, k),
                (i + 1, j, k),
                (i, j.wrapping_sub(1), k),
                (i, j + 1, k),
                (i, j, k.wrapping_sub(1)),
                (i, j, k + 1),
            ];
            for (a, b, c) in neighbours {
                if a < nx && b < ny && c < nz {
                    let n = index(a, b, c);
                    if roomy[n] && !reached[n] {
                        reached[n] = true;
                        queue.push_back((a, b, c));
                    }
                }
            }
        }
        // A cavity: within a ball's radius of where a ball fits that none
        // from outside reaches.
        let from_inside = chamfer(dims, cell, |n| roomy[n] && !reached[n]);
        for (n, c) in class.iter_mut().enumerate() {
            if *c != SOLID && from_inside[n] <= BALL_MM + 0.5 * cell {
                *c = CAVITY;
            }
        }
        Self {
            origin,
            cell,
            dims,
            class,
        }
    }

    /// Whether `p` is in a cavity's air; `None` in the solid or off the map.
    pub(crate) fn cavity_at(&self, p: Vec3) -> Option<bool> {
        let at = p.sub(self.origin).scale(1.0 / self.cell);
        let [nx, ny, nz] = self.dims;
        let (i, j, k) = (at.x.floor(), at.y.floor(), at.z.floor());
        if i < 0.0 || j < 0.0 || k < 0.0 {
            return None;
        }
        let (i, j, k) = (i as usize, j as usize, k as usize);
        if i >= nx || j >= ny || k >= nz {
            return None;
        }
        match self.class[(k * ny + j) * nx + i] {
            CAVITY => Some(true),
            OUTSIDE => Some(false),
            _ => None,
        }
    }

    /// Whether the air along `a`..`b` is a cavity's: of the samples along it
    /// that fall in air, more in a cavity than outside.
    pub(crate) fn cavity_along(&self, a: Vec3, b: Vec3) -> bool {
        let (mut cavity, mut outside) = (0, 0);
        for s in 1..8 {
            let p = a.add(b.sub(a).scale(s as f32 / 8.0));
            match self.cavity_at(p) {
                Some(true) => cavity += 1,
                Some(false) => outside += 1,
                None => {}
            }
        }
        cavity > outside
    }
}

/// The distance (mm) from each voxel to the nearest voxel `seed` says, by
/// two chamfer passes over the 26 neighbours.
fn chamfer(dims: [usize; 3], cell: f32, seed: impl Fn(usize) -> bool) -> Vec<f32> {
    let [nx, ny, nz] = dims;
    let mut d: Vec<f32> = (0..nx * ny * nz)
        .map(|n| if seed(n) { 0.0 } else { f32::INFINITY })
        .collect();
    // The neighbours before a voxel in scan order, and their distance.
    let mut before: Vec<(isize, isize, isize, f32)> = Vec::new();
    for dk in -1isize..=1 {
        for dj in -1isize..=1 {
            for di in -1isize..=1 {
                if (dk, dj, di) < (0, 0, 0) {
                    let len = ((di * di + dj * dj + dk * dk) as f32).sqrt() * cell;
                    before.push((di, dj, dk, len));
                }
            }
        }
    }
    let at = |i: isize, j: isize, k: isize| -> Option<usize> {
        (i >= 0 && j >= 0 && k >= 0 && (i as usize) < nx && (j as usize) < ny && (k as usize) < nz)
            .then(|| (k as usize * ny + j as usize) * nx + i as usize)
    };
    for k in 0..nz as isize {
        for j in 0..ny as isize {
            for i in 0..nx as isize {
                let n = at(i, j, k).unwrap_or(0);
                let mut best = d[n];
                for &(di, dj, dk, len) in &before {
                    if let Some(m) = at(i + di, j + dj, k + dk) {
                        best = best.min(d[m] + len);
                    }
                }
                d[n] = best;
            }
        }
    }
    for k in (0..nz as isize).rev() {
        for j in (0..ny as isize).rev() {
            for i in (0..nx as isize).rev() {
                let n = at(i, j, k).unwrap_or(0);
                let mut best = d[n];
                for &(di, dj, dk, len) in &before {
                    if let Some(m) = at(i - di, j - dj, k - dk) {
                        best = best.min(d[m] + len);
                    }
                }
                d[n] = best;
            }
        }
    }
    d
}
