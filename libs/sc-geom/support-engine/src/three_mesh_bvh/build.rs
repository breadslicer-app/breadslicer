/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Building the tree: each node splits its triangles where a binned
//! surface-area heuristic says a ray or a ball pays least to look into the
//! two halves, along the longest axis of their centres.

use super::{Node, NodeKind};

/// How deep the tree may go and how many triangles a leaf may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshBvhOptions {
    pub max_depth: u32,
    pub max_leaf_tris: usize,
}

impl Default for MeshBvhOptions {
    fn default() -> Self {
        MeshBvhOptions {
            max_depth: 48,
            max_leaf_tris: 8,
        }
    }
}

/// Bins the heuristic sorts centres into.
const BINS: usize = 16;

/// What looking into a box costs against testing one triangle.
const TRAVERSAL_COST: f64 = 1.0;

type Bounds = [f32; 6];

const EMPTY: Bounds = [
    f32::INFINITY,
    f32::INFINITY,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NEG_INFINITY,
    f32::NEG_INFINITY,
];

#[inline]
fn grow(into: &mut Bounds, by: &Bounds) {
    for k in 0..3 {
        into[k] = into[k].min(by[k]);
        into[k + 3] = into[k + 3].max(by[k + 3]);
    }
}

#[inline]
fn area(b: &Bounds) -> f64 {
    if b[0] > b[3] {
        return 0.0;
    }
    let [dx, dy, dz] = [0, 1, 2].map(|k| f64::from(b[k + 3] - b[k]));
    2.0 * (dx * dy + dy * dz + dz * dx)
}

#[inline]
fn centre(b: &Bounds, axis: usize) -> f64 {
    (f64::from(b[axis]) + f64::from(b[axis + 3])) * 0.5
}

/// The triangles in tree order and the nodes, the root first.
pub(super) fn build(triangles: &[Bounds], options: MeshBvhOptions) -> (Vec<u32>, Vec<Node>) {
    let mut order: Vec<u32> = (0..triangles.len() as u32).collect();
    let mut nodes = Vec::with_capacity(triangles.len().div_ceil(options.max_leaf_tris) * 2);
    if !triangles.is_empty() {
        split(
            triangles,
            &mut order,
            0,
            triangles.len(),
            0,
            options,
            &mut nodes,
        );
    }
    (order, nodes)
}

/// Adds the node of `order[start..end]` and its subtree to `nodes`.
fn split(
    triangles: &[Bounds],
    order: &mut [u32],
    start: usize,
    end: usize,
    depth: u32,
    options: MeshBvhOptions,
    nodes: &mut Vec<Node>,
) {
    let mut bounds = EMPTY;
    let mut centres = EMPTY;
    for &t in &order[start..end] {
        let b = &triangles[t as usize];
        grow(&mut bounds, b);
        let c = [0, 1, 2].map(|k| centre(b, k) as f32);
        grow(&mut centres, &[c[0], c[1], c[2], c[0], c[1], c[2]]);
    }
    let me = nodes.len();
    let count = end - start;
    let leaf = Node {
        bounds,
        kind: NodeKind::Leaf {
            offset: start as u32,
            count: count as u32,
        },
    };
    nodes.push(leaf);
    if count <= options.max_leaf_tris || depth >= options.max_depth {
        return;
    }
    let Some((axis, cut)) = best_cut(triangles, &order[start..end], &bounds, &centres) else {
        return;
    };
    // Partition: centres below the cut first.
    let slice = &mut order[start..end];
    let mut left = 0;
    for i in 0..slice.len() {
        if centre(&triangles[slice[i] as usize], axis) < cut {
            slice.swap(i, left);
            left += 1;
        }
    }
    if left == 0 || left == slice.len() {
        // All centres on one side (many equal centres): split the middle.
        left = slice.len() / 2;
    }
    let mid = start + left;
    split(triangles, order, start, mid, depth + 1, options, nodes);
    let right = nodes.len();
    split(triangles, order, mid, end, depth + 1, options, nodes);
    nodes[me].kind = NodeKind::Inner {
        right,
        split_axis: axis as u32,
    };
}

/// The axis and the centre coordinate to split at, or `None` when keeping the
/// triangles in one leaf costs less.
fn best_cut(
    triangles: &[Bounds],
    order: &[u32],
    bounds: &Bounds,
    centres: &Bounds,
) -> Option<(usize, f64)> {
    let leaf_cost = order.len() as f64;
    let parent_area = area(bounds).max(f64::MIN_POSITIVE);
    let mut best: Option<(f64, usize, f64)> = None;
    for axis in 0..3 {
        let (lo, hi) = (f64::from(centres[axis]), f64::from(centres[axis + 3]));
        let extent = hi - lo;
        if !(extent > 0.0) {
            continue;
        }
        let mut bins = [(EMPTY, 0usize); BINS];
        for &t in order {
            let b = &triangles[t as usize];
            let k = (((centre(b, axis) - lo) / extent) * BINS as f64) as usize;
            let bin = &mut bins[k.min(BINS - 1)];
            grow(&mut bin.0, b);
            bin.1 += 1;
        }
        // Sweep from the right, then from the left.
        let mut right_area = [0.0; BINS];
        let mut right_count = [0usize; BINS];
        let (mut acc, mut n) = (EMPTY, 0);
        for k in (1..BINS).rev() {
            grow(&mut acc, &bins[k].0);
            n += bins[k].1;
            right_area[k] = area(&acc);
            right_count[k] = n;
        }
        let (mut acc, mut n) = (EMPTY, 0);
        for k in 1..BINS {
            grow(&mut acc, &bins[k - 1].0);
            n += bins[k - 1].1;
            if n == 0 || right_count[k] == 0 {
                continue;
            }
            let cost = TRAVERSAL_COST
                + (area(&acc) * n as f64 + right_area[k] * right_count[k] as f64) / parent_area;
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, axis, lo + extent * k as f64 / BINS as f64));
            }
        }
    }
    best.filter(|&(cost, _, _)| cost < leaf_cost)
        .map(|(_, axis, cut)| (axis, cut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_triangle_lands_in_one_leaf() {
        let triangles: Vec<Bounds> = (0..1000)
            .map(|i| {
                let x = (i % 10) as f32;
                let y = ((i / 10) % 10) as f32;
                let z = (i / 100) as f32;
                [x, y, z, x + 0.5, y + 0.5, z + 0.5]
            })
            .collect();
        let (order, nodes) = build(&triangles, MeshBvhOptions::default());
        let mut seen = vec![0; triangles.len()];
        for node in &nodes {
            if let NodeKind::Leaf { offset, count } = node.kind {
                assert!(count as usize <= 8);
                for &t in &order[offset as usize..(offset + count) as usize] {
                    seen[t as usize] += 1;
                }
            }
        }
        assert!(seen.iter().all(|&s| s == 1));
    }
}
