/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Islands: the points of a model that print first in their region and so
//! need a support tip of their own.
//!
//! An island is a vertex lower than its whole one-ring, counted on the full
//! mesh: its topology does not change with the pose, so neither does the
//! count, and re-orienting an oriented model leaves it where it is (on a
//! simplified mesh the minima depend on the pose and the search drifts).
//! Minima closer than [`ISLAND_CLUSTER_MM`] to a lower counted one are the
//! same island ([`count_clustered`]), the rule the support engine shares.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::vec::{V3, add, cross, dot, scale, sub, unit};

/// Two minima closer than this, in millimetres, are one island: one tip
/// holds both. The support engine research measured 26–71 % of its islands
/// in such clusters; orientation and supports count them the same way.
pub const ISLAND_CLUSTER_MM: f64 = 1.5;

/// Vertex neighbours, compressed: `adj[start[v]..start[v + 1]]` holds, for
/// every face around `v`, the vertex after `v` and the one before it.
#[derive(Debug, Default)]
pub(crate) struct Ring {
    pub(crate) start: Vec<u32>,
    pub(crate) adj: Vec<[u32; 2]>,
}

impl Ring {
    pub(crate) fn new(vertex_count: usize, tri: &[[u32; 3]]) -> Self {
        let mut start = vec![0u32; vertex_count + 1];
        for t in tri {
            for &v in t {
                start[v as usize + 1] += 1;
            }
        }
        for v in 0..vertex_count {
            start[v + 1] += start[v];
        }
        let mut fill = start.clone();
        let mut adj = vec![[0u32; 2]; tri.len() * 3];
        for t in tri {
            for k in 0..3 {
                let v = t[k] as usize;
                adj[fill[v] as usize] = [t[(k + 1) % 3], t[(k + 2) % 3]];
                fill[v] += 1;
            }
        }
        Ring { start, adj }
    }

    #[inline]
    pub(crate) fn of(&self, v: usize) -> &[[u32; 2]] {
        &self.adj[self.start[v] as usize..self.start[v + 1] as usize]
    }

    /// Whether the mesh is closed and consistently oriented: every edge
    /// `a → b` of a face has exactly one face with `b → a`.
    pub(crate) fn is_closed(&self) -> bool {
        let n = self.start.len() - 1;
        (0..n).into_par_iter().with_min_len(4096).all(|v| {
            let ring = self.of(v);
            if ring.is_empty() {
                return true;
            }
            let mut next: Vec<u32> = ring.iter().map(|r| r[0]).collect();
            let mut prev: Vec<u32> = ring.iter().map(|r| r[1]).collect();
            next.sort_unstable();
            prev.sort_unstable();
            next == prev && next.windows(2).all(|w| w[0] != w[1])
        })
    }
}

/// A cap on the unit sphere: its axis and the cosine of its half-angle.
type Cap = ([f32; 3], f32);

/// Each vertex's neighbours, once each: `adj[start[v]..start[v + 1]]`, and
/// the vertices that can be a minimum at all, each with a cap around the
/// directions in which it can.
#[derive(Debug, Default)]
pub(crate) struct Neighbours {
    start: Vec<u32>,
    adj: Vec<u32>,
    /// The vertices that are a minimum in some direction, by index.
    candidates: Vec<u32>,
    /// Per candidate: the cap's axis and the cosine of its half-angle. A
    /// direction outside the cap cannot make the vertex a minimum.
    caps: Vec<Cap>,
}

impl Neighbours {
    pub(crate) fn new(ring: &Ring) -> Self {
        let n = ring.start.len() - 1;
        let blocks: Vec<(Vec<u32>, Vec<u32>)> = (0..n)
            .collect::<Vec<usize>>()
            .par_chunks(1 << 14)
            .map(|vs| {
                let mut lens = Vec::with_capacity(vs.len());
                let mut adj = Vec::new();
                let mut scratch: Vec<u32> = Vec::new();
                for &v in vs {
                    scratch.clear();
                    scratch.extend(ring.of(v).iter().flatten());
                    scratch.sort_unstable();
                    scratch.dedup();
                    lens.push(scratch.len() as u32);
                    adj.extend_from_slice(&scratch);
                }
                (lens, adj)
            })
            .collect();
        let mut start = Vec::with_capacity(n + 1);
        start.push(0u32);
        let mut adj = Vec::new();
        for (lens, a) in blocks {
            for l in lens {
                let last = *start.last().unwrap_or(&0);
                start.push(last + l);
            }
            adj.extend(a);
        }
        Neighbours {
            start,
            adj,
            candidates: Vec::new(),
            caps: Vec::new(),
        }
    }

    /// Finds which vertices can be a minimum, and in which directions.
    pub(crate) fn with_caps(mut self, pos: &[V3]) -> Self {
        let blocks: Vec<Vec<(u32, Cap)>> = (0..pos.len().div_ceil(VERTEX_BLOCK))
            .into_par_iter()
            .map(|b| {
                let mut out = Vec::new();
                let mut edges: Vec<V3> = Vec::new();
                for v in b * VERTEX_BLOCK..((b + 1) * VERTEX_BLOCK).min(pos.len()) {
                    let ring = self.of(v);
                    if ring.is_empty() {
                        continue;
                    }
                    edges.clear();
                    edges.extend(ring.iter().map(|&j| sub(pos[j as usize], pos[v])));
                    if let Some(cap) = minimum_cap(&edges) {
                        out.push((v as u32, cap));
                    }
                }
                out
            })
            .collect();
        for (v, cap) in blocks.into_iter().flatten() {
            self.candidates.push(v);
            self.caps.push(cap);
        }
        self
    }

    #[inline]
    fn of(&self, v: usize) -> &[u32] {
        &self.adj[self.start[v] as usize..self.start[v + 1] as usize]
    }
}

/// Directions scored per pass over the mesh: the vertices and their
/// neighbours are read once for all of them.
const DIRECTIONS_PER_PASS: usize = 32;
/// Vertices per parallel block.
const VERTEX_BLOCK: usize = 1 << 13;

/// A cap holding every direction `u` with `e·u > 0` for all `edges`, the
/// directions that make the vertex a minimum; `None` when there is none (a
/// saddle). The cone of those directions is spanned by its extreme rays,
/// each the cross product of two edges, so the cap is built from the cross
/// products that lie on the right side of every edge. A cap of cosine −2
/// prunes nothing (too few edges, or a cone too wide to bound).
fn minimum_cap(edges: &[V3]) -> Option<Cap> {
    const ANY: Cap = ([0.0; 3], -2.0);
    if edges.len() < 3 {
        return Some(ANY);
    }
    let lengths: Vec<f64> = edges.iter().map(|e| dot(*e, *e).sqrt()).collect();
    let mut rays: Vec<V3> = Vec::new();
    let mut crossed = false;
    for i in 0..edges.len() {
        for j in i + 1..edges.len() {
            let Some(r) = unit(cross(edges[i], edges[j])) else {
                continue;
            };
            crossed = true;
            for r in [r, scale(r, -1.0)] {
                if edges
                    .iter()
                    .zip(&lengths)
                    .all(|(e, l)| dot(*e, r) >= -1e-9 * l)
                {
                    rays.push(r);
                }
            }
        }
    }
    if !crossed {
        return Some(ANY);
    }
    if rays.is_empty() {
        return None;
    }
    let Some(axis) = unit(rays.iter().fold([0.0; 3], |s, r| add(s, *r))) else {
        return Some(ANY);
    };
    let cos = rays.iter().map(|r| dot(axis, *r)).fold(1.0, f64::min);
    if cos < 0.02 {
        return Some(ANY);
    }
    // A little slack for the single-precision test.
    Some((axis.map(|x| x as f32), (cos - 1e-4) as f32))
}

/// The local minima of the mesh for each of `dirs`: the vertices lower than
/// all their neighbours, by index.
pub(crate) fn local_minima_many(pos: &[V3], nbrs: &Neighbours, dirs: &[V3]) -> Vec<Vec<u32>> {
    let mut out: Vec<Vec<u32>> = vec![Vec::new(); dirs.len()];
    let n = nbrs.candidates.len();
    for (pass, group) in dirs.chunks(DIRECTIONS_PER_PASS).enumerate() {
        let group32: Vec<[f32; 3]> = group.iter().map(|u| u.map(|x| x as f32)).collect();
        let blocks: Vec<Vec<Vec<u32>>> = (0..n.div_ceil(VERTEX_BLOCK))
            .into_par_iter()
            .map(|b| {
                let mut found: Vec<Vec<u32>> = vec![Vec::new(); group.len()];
                let mut edges: Vec<V3> = Vec::new();
                for c in b * VERTEX_BLOCK..((b + 1) * VERTEX_BLOCK).min(n) {
                    let (axis, cos) = nbrs.caps[c];
                    let v = nbrs.candidates[c] as usize;
                    edges.clear();
                    for (k, u) in group32.iter().enumerate() {
                        if axis[0] * u[0] + axis[1] * u[1] + axis[2] * u[2] < cos {
                            continue;
                        }
                        if edges.is_empty() {
                            let p = pos[v];
                            edges.extend(nbrs.of(v).iter().map(|&j| sub(pos[j as usize], p)));
                        }
                        if edges.iter().all(|e| dot(*e, group[k]) > 0.0) {
                            found[k].push(v as u32);
                        }
                    }
                }
                found
            })
            .collect();
        for block in blocks {
            for (k, f) in block.into_iter().enumerate() {
                out[pass * DIRECTIONS_PER_PASS + k].extend(f);
            }
        }
    }
    out
}

/// The minima that count as islands, from lowest up: a minimum closer than
/// `merge_mm` to a lower counted one is part of its island. `points` are the
/// minima's positions, already sorted from lowest up.
///
/// This is the clustering rule the support engine (#64) is meant to share.
pub fn count_clustered(points: &[[f64; 3]], merge_mm: f64) -> Vec<usize> {
    let cell = |p: V3| p.map(|x| (x / merge_mm).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<V3>> = HashMap::new();
    let mut kept = Vec::new();
    let r2 = merge_mm * merge_mm;
    for (i, &p) in points.iter().enumerate() {
        let c = cell(p);
        let mut near = false;
        'look: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(ps) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz])
                        && ps.iter().any(|q| {
                            let d = sub(*q, p);
                            dot(d, d) < r2
                        })
                    {
                        near = true;
                        break 'look;
                    }
                }
            }
        }
        if !near {
            kept.push(i);
            grid.entry(c).or_default().push(p);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_minima_count_once_and_far_ones_apart() {
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.2],
            [5.0, 0.0, 0.3],
            [5.0, 1.4, 0.4],
            [5.0, 2.8, 0.5],
        ];
        // The last one is 2.8 mm from the kept one at y = 0, so it counts.
        assert_eq!(count_clustered(&pts, ISLAND_CLUSTER_MM), vec![0, 2, 4]);
    }
}
