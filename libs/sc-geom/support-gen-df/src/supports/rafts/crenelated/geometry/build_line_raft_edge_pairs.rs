//! DragonFruit `geometry/buildLineRaftEdgePairs.ts` and
//! `delaunayTriangulate2d.ts`: the line raft's beam graph — hull edges, the
//! short Delaunay edges, and a spanning-tree backbone that keeps it connected.

use std::collections::{HashMap, HashSet};

use super::convex_hull_2d::convex_hull_2d;
use crate::three::Vector2;
use crate::three::math_utils::{js_max, js_min};

/// Bowyer–Watson over a super-triangle; triangles touching it are dropped.
pub fn delaunay_triangulate_2d(points: &[Vector2]) -> Vec<[usize; 3]> {
    if points.len() < 3 {
        return Vec::new();
    }
    let mut pts: Vec<Vector2> = points.to_vec();
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in &pts {
        min_x = js_min(min_x, p.x);
        min_y = js_min(min_y, p.y);
        max_x = js_max(max_x, p.x);
        max_y = js_max(max_y, p.y);
    }
    let dmax = js_max(max_x - min_x, max_y - min_y);
    let (midx, midy) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    let n = points.len();
    let (i0, i1, i2) = (n, n + 1, n + 2);
    pts.push(Vector2::new(midx - 20.0 * dmax, midy - dmax));
    pts.push(Vector2::new(midx, midy + 20.0 * dmax));
    pts.push(Vector2::new(midx + 20.0 * dmax, midy - dmax));

    let mut triangles: Vec<[usize; 3]> = vec![[i0, i1, i2]];
    for pi in 0..n {
        let p = pts[pi];
        let bad: Vec<bool> = triangles
            .iter()
            .map(|t| circumcircle_contains(pts[t[0]], pts[t[1]], pts[t[2]], p))
            .collect();
        // Edges once among the bad triangles, in first-seen order.
        let mut order: Vec<(usize, usize)> = Vec::new();
        let mut count: HashMap<(usize, usize), (usize, usize, usize)> = HashMap::new();
        for (t, _) in triangles.iter().zip(&bad).filter(|(_, b)| **b) {
            for (ea, eb) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let key = (ea.min(eb), ea.max(eb));
                match count.get_mut(&key) {
                    Some(e) => e.2 += 1,
                    None => {
                        count.insert(key, (ea, eb, 1));
                        order.push(key);
                    }
                }
            }
        }
        let mut k = 0;
        triangles.retain(|_| {
            k += 1;
            !bad[k - 1]
        });
        for key in order {
            let (a, b, c) = count[&key];
            if c == 1 {
                triangles.push([a, b, pi]);
            }
        }
    }
    triangles.retain(|t| t[0] < n && t[1] < n && t[2] < n);
    triangles
}

fn circumcircle_contains(a: Vector2, b: Vector2, c: Vector2, p: Vector2) -> bool {
    let (ax, ay) = (a.x - p.x, a.y - p.y);
    let (bx, by) = (b.x - p.x, b.y - p.y);
    let (cx, cy) = (c.x - p.x, c.y - p.y);
    let det = (ax * ax + ay * ay) * (bx * cy - by * cx) - (bx * bx + by * by) * (ax * cy - ay * cx)
        + (cx * cx + cy * cy) * (ax * by - ay * bx);
    let orient = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if orient > 0.0 {
        det > 1e-10
    } else {
        det < -1e-10
    }
}

fn edge_key(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
}

fn edge_len(a: Vector2, b: Vector2) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    (dx * dx + dy * dy).sqrt()
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        UnionFind {
            parent: (0..size).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut cursor = x;
        while self.parent[cursor] != cursor {
            let next = self.parent[cursor];
            self.parent[cursor] = root;
            cursor = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        self.parent[rb] = ra;
        true
    }

    fn component_count(&mut self) -> usize {
        let roots: HashSet<usize> = (0..self.parent.len()).map(|i| self.find(i)).collect();
        roots.len()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BuildLineRaftEdgePairsOptions {
    pub has_border_ring: bool,
    pub keep_factor: Option<f64>,
    pub abs_max_len: Option<f64>,
    pub enforce_connected: Option<bool>,
    pub include_spanning_tree_backbone: Option<bool>,
}

#[derive(Clone, Copy)]
struct Candidate {
    a: usize,
    b: usize,
    len: f64,
}

/// `(left, right) => left.len - right.len`, a consistent order: any stable
/// sort agrees with V8's.
fn sort_by_len(c: &mut [Candidate]) {
    c.sort_by(|l, r| {
        l.len
            .partial_cmp(&r.len)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

pub fn build_line_raft_edge_pairs(
    nodes2d: &[Vector2],
    options: BuildLineRaftEdgePairsOptions,
) -> Vec<(usize, usize)> {
    if nodes2d.len() < 2 {
        return Vec::new();
    }
    let keep_factor = options.keep_factor.unwrap_or(8.0);
    let abs_max_len = options.abs_max_len.unwrap_or(220.0);
    let enforce_connected = options.enforce_connected.unwrap_or(true);
    let include_backbone = options.include_spanning_tree_backbone.unwrap_or(true);

    let hull = convex_hull_2d(nodes2d);
    let hull_indices: Vec<usize> = hull
        .iter()
        .map(|hp| {
            let mut best = 0;
            let mut best_d2 = f64::INFINITY;
            for (i, p) in nodes2d.iter().enumerate() {
                let (dx, dy) = (p.x - hp.x, p.y - hp.y);
                let d2 = dx * dx + dy * dy;
                if d2 < best_d2 {
                    best_d2 = d2;
                    best = i;
                }
            }
            best
        })
        .collect();
    let mut hull_edges = Vec::new();
    if hull_indices.len() >= 2 {
        for i in 0..hull_indices.len() {
            let (a, b) = (hull_indices[i], hull_indices[(i + 1) % hull_indices.len()]);
            if a != b {
                hull_edges.push((a, b));
            }
        }
    }

    let tris = delaunay_triangulate_2d(nodes2d);

    let n = nodes2d.len();
    let mut nn = vec![f64::INFINITY; n];
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            nn[i] = js_min(nn[i], edge_len(nodes2d[i], nodes2d[j]));
        }
        if !nn[i].is_finite() {
            nn[i] = 0.0;
        }
    }

    let mut edges: HashSet<(usize, usize)> = HashSet::new();
    let mut edge_pairs: Vec<(usize, usize)> = Vec::new();
    let mut candidate_order: Vec<(usize, usize)> = Vec::new();
    let mut candidates: HashMap<(usize, usize), Candidate> = HashMap::new();
    let mut record = |a: usize, b: usize, order: &mut Vec<(usize, usize)>| {
        if a == b {
            return;
        }
        let key = edge_key(a, b);
        if candidates.contains_key(&key) {
            return;
        }
        candidates.insert(
            key,
            Candidate {
                a,
                b,
                len: edge_len(nodes2d[a], nodes2d[b]),
            },
        );
        order.push(key);
    };

    for &(a, b) in &hull_edges {
        record(a, b, &mut candidate_order);
        if edges.insert(edge_key(a, b)) {
            edge_pairs.push((a, b));
        }
    }
    for t in &tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let key = edge_key(a, b);
            record(a, b, &mut candidate_order);
            if edges.contains(&key) {
                continue;
            }
            let len = edge_len(nodes2d[a], nodes2d[b]);
            let local_max = keep_factor * js_min(nn[a], nn[b]);
            if len > abs_max_len {
                continue;
            }
            if nn[a] > 0.0 && nn[b] > 0.0 && len > local_max {
                continue;
            }
            edges.insert(key);
            edge_pairs.push((a, b));
        }
    }

    if !enforce_connected || n < 3 {
        return edge_pairs;
    }

    let bridge = |uf: &mut UnionFind,
                  edges: &mut HashSet<(usize, usize)>,
                  edge_pairs: &mut Vec<(usize, usize)>,
                  skip: &dyn Fn(&(usize, usize)) -> bool| {
        let mut bridges = Vec::new();
        for i in 0..n {
            for j in i + 1..n {
                if skip(&edge_key(i, j)) {
                    continue;
                }
                bridges.push(Candidate {
                    a: i,
                    b: j,
                    len: edge_len(nodes2d[i], nodes2d[j]),
                });
            }
        }
        sort_by_len(&mut bridges);
        for c in bridges {
            if uf.component_count() <= 1 {
                break;
            }
            if !uf.union(c.a, c.b) {
                continue;
            }
            if edges.insert(edge_key(c.a, c.b)) {
                edge_pairs.push((c.a, c.b));
            }
        }
    };

    if include_backbone {
        let mut mst = UnionFind::new(n);
        let mut list: Vec<Candidate> = candidate_order.iter().map(|k| candidates[k]).collect();
        sort_by_len(&mut list);
        for c in list {
            if !mst.union(c.a, c.b) {
                continue;
            }
            if edges.insert(edge_key(c.a, c.b)) {
                edge_pairs.push((c.a, c.b));
            }
        }
        if mst.component_count() > 1 {
            bridge(&mut mst, &mut edges, &mut edge_pairs, &|k| {
                candidates.contains_key(k)
            });
        }
    } else {
        let mut uf = UnionFind::new(n);
        for &(a, b) in &edge_pairs {
            uf.union(a, b);
        }
        if uf.component_count() > 1 {
            let known = edges.clone();
            bridge(&mut uf, &mut edges, &mut edge_pairs, &|k| known.contains(k));
        }
    }
    edge_pairs
}
