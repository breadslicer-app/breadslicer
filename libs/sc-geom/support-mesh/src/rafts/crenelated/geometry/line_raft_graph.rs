//! Which support roots a line raft joins with beams: the hull's sides, the
//! short edges of the roots' Delaunay triangulation, and enough of the
//! shortest edges after that to tie every root to the rest.

use std::collections::{HashMap, HashSet};

use super::raft_body::convex_hull_2d;
use support_geom::geom::Vector2;

/// The Delaunay triangles of `points` (indices), built by inserting the
/// points in order into a triangle far larger than them all and re-fanning
/// the cavity each opens; triangles on the large triangle's corners are
/// dropped. A point on a circumcircle (within 1e-10) is outside it, so
/// cocircular points keep the triangles found first.
pub fn delaunay_triangulate_2d(points: &[Vector2]) -> Vec<[usize; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in points {
        lo = [lo[0].min(p.x), lo[1].min(p.y)];
        hi = [hi[0].max(p.x), hi[1].max(p.y)];
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let mut all = points.to_vec();
    all.extend([
        Vector2::new(mid[0] - 20.0 * span, mid[1] - span),
        Vector2::new(mid[0], mid[1] + 20.0 * span),
        Vector2::new(mid[0] + 20.0 * span, mid[1] - span),
    ]);
    let mut triangles: Vec<[usize; 3]> = vec![[n, n + 1, n + 2]];
    for p in 0..n {
        let (open, kept): (Vec<[usize; 3]>, Vec<[usize; 3]>) = triangles
            .into_iter()
            .partition(|t| in_circle(all[t[0]], all[t[1]], all[t[2]], all[p]));
        // The cavity's rim: the open triangles' edges no other open
        // triangle shares, in the order first met.
        let mut rim: Vec<(usize, usize)> = Vec::new();
        let mut seen: HashMap<(usize, usize), (usize, bool)> = HashMap::new();
        for t in &open {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let key = (a.min(b), a.max(b));
                match seen.get_mut(&key) {
                    Some(entry) => entry.1 = false,
                    None => {
                        seen.insert(key, (rim.len(), true));
                        rim.push((a, b));
                    }
                }
            }
        }
        triangles = kept;
        triangles.extend(
            rim.into_iter()
                .filter(|&(a, b)| seen[&(a.min(b), a.max(b))].1)
                .map(|(a, b)| [a, b, p]),
        );
    }
    triangles.retain(|t| t.iter().all(|&v| v < n));
    triangles
}

/// Whether `p` lies inside the circle through `a`, `b`, `c` (by more than
/// 1e-10 in the determinant, whichever way the triangle turns).
fn in_circle(a: Vector2, b: Vector2, c: Vector2, p: Vector2) -> bool {
    let [a, b, c] = [a, b, c].map(|q| (q.x - p.x, q.y - p.y));
    let lift = |q: (f64, f64)| q.0 * q.0 + q.1 * q.1;
    let det = lift(a) * (b.0 * c.1 - b.1 * c.0) - lift(b) * (a.0 * c.1 - a.1 * c.0)
        + lift(c) * (a.0 * b.1 - a.1 * b.0);
    let turn = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    if turn > 0.0 {
        det > 1e-10
    } else {
        det < -1e-10
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BuildLineRaftEdgePairsOptions {
    pub has_border_ring: bool,
    /// An edge longer than this many times its ends' nearest-neighbour
    /// distance is left out (8 by default)...
    pub keep_factor: Option<f64>,
    /// ...and so is one longer than this (mm, 220 by default).
    pub abs_max_len: Option<f64>,
    /// Tie every root to the rest (by default).
    pub enforce_connected: Option<bool>,
    /// Through the shortest edges seen (a spanning tree), before bridging
    /// what is still apart (by default).
    pub include_spanning_tree_backbone: Option<bool>,
}

/// Disjoint sets, each pointing at the member that joined it first.
struct Sets(Vec<usize>);

impl Sets {
    fn root(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            let up = self.0[self.0[x]];
            self.0[x] = up;
            x = up;
        }
        x
    }

    /// Joins `a`'s set and `b`'s; false when they were one.
    fn join(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra != rb {
            self.0[rb] = ra;
        }
        ra != rb
    }

    fn count(&mut self) -> usize {
        (0..self.0.len()).filter(|&i| self.root(i) == i).count()
    }
}

/// The beams of a line raft over `nodes` (support roots in plan), as pairs
/// of indices, in the order they are chosen.
pub fn build_line_raft_edge_pairs(
    nodes: &[Vector2],
    options: BuildLineRaftEdgePairsOptions,
) -> Vec<(usize, usize)> {
    let n = nodes.len();
    if n < 2 {
        return Vec::new();
    }
    let keep_factor = options.keep_factor.unwrap_or(8.0);
    let longest = options.abs_max_len.unwrap_or(220.0);
    let connect = options.enforce_connected.unwrap_or(true);
    let backbone = options.include_spanning_tree_backbone.unwrap_or(true);
    let length = |a: usize, b: usize| {
        let (dx, dy) = (nodes[b].x - nodes[a].x, nodes[b].y - nodes[a].y);
        (dx * dx + dy * dy).sqrt()
    };
    let key = |a: usize, b: usize| (a.min(b), a.max(b));

    // The hull's corners, each as the first node nearest to it.
    let corners: Vec<usize> = convex_hull_2d(nodes)
        .iter()
        .map(|h| {
            (0..n)
                .map(|i| {
                    let (dx, dy) = (nodes[i].x - h.x, nodes[i].y - h.y);
                    (dx * dx + dy * dy, i)
                })
                .fold(
                    (f64::INFINITY, 0),
                    |best, c| if c.0 < best.0 { c } else { best },
                )
                .1
        })
        .collect();
    let nearest: Vec<f64> = (0..n)
        .map(|i| {
            let d = (0..n)
                .filter(|&j| j != i)
                .map(|j| length(i, j))
                .fold(f64::INFINITY, f64::min);
            if d.is_finite() { d } else { 0.0 }
        })
        .collect();

    let mut chosen: HashSet<(usize, usize)> = HashSet::new();
    let mut out: Vec<(usize, usize)> = Vec::new();
    // Every edge seen (hull and triangulation), once, in order.
    let mut seen: Vec<(usize, usize)> = Vec::new();
    let mut seen_set: HashSet<(usize, usize)> = HashSet::new();
    let mut see = |a: usize, b: usize, seen: &mut Vec<(usize, usize)>| {
        if a != b && seen_set.insert(key(a, b)) {
            seen.push((a, b));
        }
    };
    if corners.len() >= 2 {
        for i in 0..corners.len() {
            let (a, b) = (corners[i], corners[(i + 1) % corners.len()]);
            if a != b {
                see(a, b, &mut seen);
                if chosen.insert(key(a, b)) {
                    out.push((a, b));
                }
            }
        }
    }
    for t in delaunay_triangulate_2d(nodes) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            see(a, b, &mut seen);
            if a == b || chosen.contains(&key(a, b)) {
                continue;
            }
            let l = length(a, b);
            let too_long_here = nearest[a] > 0.0
                && nearest[b] > 0.0
                && l > keep_factor * nearest[a].min(nearest[b]);
            if l > longest || too_long_here {
                continue;
            }
            chosen.insert(key(a, b));
            out.push((a, b));
        }
    }
    if !connect || n < 3 {
        return out;
    }

    let by_length = |edges: &mut Vec<(usize, usize)>| {
        edges.sort_by(|p, q| {
            length(p.0, p.1)
                .partial_cmp(&length(q.0, q.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    };
    // Join what is apart through the shortest of `edges`.
    let tie = |sets: &mut Sets,
               edges: Vec<(usize, usize)>,
               chosen: &mut HashSet<(usize, usize)>,
               out: &mut Vec<(usize, usize)>| {
        for (a, b) in edges {
            if sets.count() <= 1 {
                break;
            }
            if sets.join(a, b) && chosen.insert(key(a, b)) {
                out.push((a, b));
            }
        }
    };
    // Every pair not in `skip`, shortest first.
    let pairs = |skip: &HashSet<(usize, usize)>| {
        let mut all: Vec<(usize, usize)> = (0..n)
            .flat_map(|i| ((i + 1)..n).map(move |j| (i, j)))
            .filter(|p| !skip.contains(p))
            .collect();
        by_length(&mut all);
        all
    };
    let mut sets = Sets((0..n).collect());
    if backbone {
        let mut tree = seen.clone();
        by_length(&mut tree);
        for (a, b) in tree {
            if sets.join(a, b) && chosen.insert(key(a, b)) {
                out.push((a, b));
            }
        }
        if sets.count() > 1 {
            let skip: HashSet<(usize, usize)> = seen.iter().map(|&(a, b)| key(a, b)).collect();
            tie(&mut sets, pairs(&skip), &mut chosen, &mut out);
        }
    } else {
        for &(a, b) in &out {
            sets.join(a, b);
        }
        if sets.count() > 1 {
            let skip = chosen.clone();
            tie(&mut sets, pairs(&skip), &mut chosen, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square's corners and its middle: the sides and the four spokes,
    /// and every root joined.
    #[test]
    fn a_square_with_a_middle_is_its_sides_and_spokes() {
        let nodes = [
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (5.0, 5.0),
        ]
        .map(|(x, y)| Vector2::new(x, y));
        assert_eq!(delaunay_triangulate_2d(&nodes).len(), 4);
        let options = BuildLineRaftEdgePairsOptions {
            has_border_ring: false,
            keep_factor: None,
            abs_max_len: None,
            enforce_connected: None,
            include_spanning_tree_backbone: None,
        };
        let mut edges: Vec<(usize, usize)> = build_line_raft_edge_pairs(&nodes, options)
            .into_iter()
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect();
        edges.sort_unstable();
        assert_eq!(
            edges,
            vec![
                (0, 1),
                (0, 3),
                (0, 4),
                (1, 2),
                (1, 4),
                (2, 3),
                (2, 4),
                (3, 4)
            ]
        );
    }
}
