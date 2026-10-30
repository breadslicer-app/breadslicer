/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Cups: the hollows that open towards the film and pull a vacuum at every
//! peel.
//!
//! A hollow that would hold water with the model standing in its pose (a
//! bowl open towards +Z) is closed towards the plate and open towards the
//! film: while its walls print, the resin under the film cannot flow in, and
//! the film sucks on it. The model is voxelized once, in its own frame; for
//! each pose a priority flood finds the water level every air voxel would
//! hold (the lowest level at which it drains to the outside). Air below its
//! level is cup.
//!
//! This is the coarse cup term of the core search; the Full mode (#185)
//! refines it.

use crate::vec::{V3, dot};

/// Cells along the model's longest side.
const CELLS: f64 = 40.0;

/// The model's voxels: `true` for air outside the model.
#[derive(Debug, Default)]
pub(crate) struct Voxels {
    dims: [usize; 3],
    origin: V3,
    pub(crate) size: f64,
    outside: Vec<bool>,
}

impl Voxels {
    pub(crate) fn new(pos: &[V3], tri: &[[u32; 3]], lo: V3, hi: V3) -> Self {
        let longest = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
        if !(longest.is_finite() && longest > 0.0) || tri.is_empty() {
            return Voxels::default();
        }
        let size = longest / CELLS;
        // One cell of air all around, so the border is outside.
        let origin = [lo[0] - size, lo[1] - size, lo[2] - size];
        let dims = [0, 1, 2].map(|k| ((hi[k] - lo[k]) / size).ceil() as usize + 3);
        let count = dims[0] * dims[1] * dims[2];
        let mut wall = vec![false; count];
        let index = |p: V3| {
            let c = [0, 1, 2].map(|k| (((p[k] - origin[k]) / size) as usize).min(dims[k] - 1));
            (c[2] * dims[1] + c[1]) * dims[0] + c[0]
        };
        // Mark every cell a triangle passes through, by sampling it at a
        // third of a cell: walls stay watertight for a 6-connected flood.
        for t in tri {
            let [a, b, c] = t.map(|i| pos[i as usize]);
            let edge = |p: V3, q: V3| {
                let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
                dot(d, d).sqrt()
            };
            let longest_edge = edge(a, b).max(edge(b, c)).max(edge(c, a));
            let n = ((longest_edge / (size / 3.0)).ceil() as usize).max(1);
            for i in 0..=n {
                for j in 0..=(n - i) {
                    let (s, r) = (i as f64 / n as f64, j as f64 / n as f64);
                    let w = 1.0 - s - r;
                    let p = [0, 1, 2].map(|k| w * a[k] + s * b[k] + r * c[k]);
                    wall[index(p)] = true;
                }
            }
        }
        // Air reachable from the border is outside; the rest is the model.
        let mut outside = vec![false; count];
        let mut stack = vec![0usize];
        outside[0] = true;
        let strides = [1, dims[0], dims[0] * dims[1]];
        while let Some(i) = stack.pop() {
            let c = [i % dims[0], (i / dims[0]) % dims[1], i / strides[2]];
            for k in 0..3 {
                if c[k] > 0 {
                    let j = i - strides[k];
                    if !wall[j] && !outside[j] {
                        outside[j] = true;
                        stack.push(j);
                    }
                }
                if c[k] + 1 < dims[k] {
                    let j = i + strides[k];
                    if !wall[j] && !outside[j] {
                        outside[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        Voxels {
            dims,
            origin,
            size,
            outside,
        }
    }

    /// The cup area per height band of one cell, from `z0` up, in mm².
    /// With `plate`, the plate at `z0` closes the bottom: air under it is
    /// not a way out, and a hollow sealed against it is a cup.
    pub(crate) fn cups(&self, up: V3, z0: f64, plate: bool) -> Vec<f64> {
        if self.outside.is_empty() {
            return Vec::new();
        }
        let d = self.dims;
        let count = d[0] * d[1] * d[2];
        let strides = [1, d[0], d[0] * d[1]];
        // Cell heights, axis by axis.
        let axis = |k: usize| -> Vec<f64> {
            (0..d[k])
                .map(|c| (self.origin[k] + (c as f64 + 0.5) * self.size) * up[k])
                .collect()
        };
        let (hx, hy, hz) = (axis(0), axis(1), axis(2));
        let mut h = Vec::with_capacity(count);
        for z in &hz {
            for y in &hy {
                h.extend(hx.iter().map(|x| x + y + z));
            }
        }
        let open = |i: usize| self.outside[i] && !(plate && h[i] < z0);
        // Water level per cell; infinite until the flood reaches it. The
        // flood runs on a bucket queue an eighth of a cell fine: levels come
        // out within that of the exact priority flood, well inside the depth
        // a cup must have.
        let mut level = vec![f64::INFINITY; count];
        let lowest = h.iter().copied().fold(f64::INFINITY, f64::min);
        let highest = h.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let quantum = self.size / 8.0;
        let bucket = |x: f64| ((x - lowest) / quantum) as usize;
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); bucket(highest) + 2];
        for i in 0..count {
            let c = [i % d[0], (i / d[0]) % d[1], i / strides[2]];
            let border = (0..3).any(|k| c[k] == 0 || c[k] + 1 == d[k]);
            if border && open(i) {
                level[i] = h[i];
                buckets[bucket(h[i])].push(i as u32);
            }
        }
        for b in 0..buckets.len() {
            while let Some(i) = buckets[b].pop() {
                let i = i as usize;
                let li = level[i];
                let c = [i % d[0], (i / d[0]) % d[1], i / strides[2]];
                for k in 0..3 {
                    for j in [
                        (c[k] > 0).then(|| i - strides[k]),
                        (c[k] + 1 < d[k]).then(|| i + strides[k]),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if open(j) && level[j].is_infinite() {
                            level[j] = h[j].max(li);
                            buckets[bucket(level[j]).max(b)].push(j as u32);
                        }
                    }
                }
            }
        }
        let cell_area = self.size * self.size;
        let mut bands: Vec<f64> = Vec::new();
        for i in 0..count {
            if !open(i) {
                continue;
            }
            let depth = level[i] - h[i];
            if depth > 0.25 * self.size {
                let band = ((h[i] - z0) / self.size).floor().max(0.0) as usize;
                if bands.len() <= band {
                    bands.resize(band + 1, 0.0);
                }
                bands[band] += cell_area;
            }
        }
        bands
    }
}
