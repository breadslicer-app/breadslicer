/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Convex hulls: the 3D hull of the model (its height in any direction, its
//! large flat faces) and the 2D hull of its shadow (whether it fits the
//! plate, how slender it stands).

use rayon::prelude::*;

use crate::vec::{V3, cross, dot, norm, scale, sub, unit};

/// The convex hull of a point set.
#[derive(Debug, Clone, Default)]
pub(crate) struct Hull {
    /// The hull's vertices, in the order the input listed them.
    pub(crate) points: Vec<V3>,
    /// The hull's triangles: outward unit normal and area.
    pub(crate) faces: Vec<HullFace>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct HullFace {
    pub(crate) normal: V3,
    pub(crate) area: f64,
}

impl Hull {
    /// Lowest and highest `p·up` over the hull, that is over the whole model.
    pub(crate) fn extent(&self, up: V3) -> (f64, f64) {
        self.points
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                let z = dot(*p, up);
                (lo.min(z), hi.max(z))
            })
    }
}

/// Points per block when the hull is built in parallel: each block's hull is
/// built on its own thread, then the hull of the blocks' hull vertices.
const BLOCK: usize = 1 << 16;

/// The convex hull of `points`. Deterministic: the blocks are fixed, so the
/// result does not depend on the thread count.
pub(crate) fn convex_hull(points: &[V3]) -> Hull {
    if points.len() <= BLOCK {
        return quickhull(points);
    }
    let parts: Vec<Vec<V3>> = points
        .par_chunks(BLOCK)
        .map(|block| quickhull(block).points)
        .collect();
    quickhull(&parts.concat())
}

struct Face {
    v: [usize; 3],
    n: V3,
    d: f64,
    /// `adj[k]`: the face across the edge `v[k] → v[k + 1]`.
    adj: [usize; 3],
    outside: Vec<usize>,
    alive: bool,
}

fn make_face(p: &[V3], v: [usize; 3]) -> Face {
    let n = cross(sub(p[v[1]], p[v[0]]), sub(p[v[2]], p[v[0]]));
    let n = unit(n).unwrap_or([0.0, 0.0, 1.0]);
    Face {
        v,
        n,
        d: dot(n, p[v[0]]),
        adj: [usize::MAX; 3],
        outside: Vec::new(),
        alive: true,
    }
}

/// Quickhull. A point within `eps` of a face counts as on it; for a set with
/// no volume (flat or a line) the points themselves are returned, without
/// faces.
fn quickhull(p: &[V3]) -> Hull {
    let degenerate = || Hull {
        points: p.to_vec(),
        faces: Vec::new(),
    };
    if p.len() < 4 {
        return degenerate();
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let (mut ilo, mut ihi) = ([0usize; 3], [0usize; 3]);
    for (i, q) in p.iter().enumerate() {
        for k in 0..3 {
            if q[k] < lo[k] {
                lo[k] = q[k];
                ilo[k] = i;
            }
            if q[k] > hi[k] {
                hi[k] = q[k];
                ihi[k] = i;
            }
        }
    }
    let span = sub(hi, lo);
    let size = span[0].max(span[1]).max(span[2]);
    if !size.is_finite() || size <= 0.0 {
        return degenerate();
    }
    let reach = lo
        .iter()
        .chain(&hi)
        .fold(0.0_f64, |m, x| m.max(x.abs()))
        .max(size);
    let eps = reach * 1e-11;
    // The initial tetrahedron: the widest axis pair, the farthest point from
    // their line, then the farthest from their plane.
    let axis = (0..3)
        .max_by(|&a, &b| span[a].total_cmp(&span[b]))
        .unwrap_or(0);
    let (i0, i1) = (ilo[axis], ihi[axis]);
    let dir = match unit(sub(p[i1], p[i0])) {
        Some(d) => d,
        None => return degenerate(),
    };
    let line_dist = |q: V3| {
        let w = sub(q, p[i0]);
        norm(sub(w, scale(dir, dot(w, dir))))
    };
    let i2 = (0..p.len())
        .max_by(|&a, &b| line_dist(p[a]).total_cmp(&line_dist(p[b])))
        .unwrap_or(0);
    if line_dist(p[i2]) <= eps {
        return degenerate();
    }
    let pn = match unit(cross(sub(p[i1], p[i0]), sub(p[i2], p[i0]))) {
        Some(n) => n,
        None => return degenerate(),
    };
    let plane = |q: V3| dot(pn, sub(q, p[i0]));
    let i3 = (0..p.len())
        .max_by(|&a, &b| plane(p[a]).abs().total_cmp(&plane(p[b]).abs()))
        .unwrap_or(0);
    if plane(p[i3]).abs() <= eps {
        return degenerate();
    }
    // Orient the base so the fourth point is behind it.
    let base = if plane(p[i3]) > 0.0 {
        [i0, i2, i1]
    } else {
        [i0, i1, i2]
    };
    let mut faces = vec![
        make_face(p, base),
        make_face(p, [base[1], base[0], i3]),
        make_face(p, [base[2], base[1], i3]),
        make_face(p, [base[0], base[2], i3]),
    ];
    for f in 0..4 {
        for k in 0..3 {
            let (a, b) = (faces[f].v[k], faces[f].v[(k + 1) % 3]);
            let g = (0..4)
                .find(|&g| {
                    g != f && (0..3).any(|j| faces[g].v[j] == b && faces[g].v[(j + 1) % 3] == a)
                })
                .unwrap_or(f);
            faces[f].adj[k] = g;
        }
    }
    let simplex = [i0, i1, i2, i3];
    for (i, &q) in p.iter().enumerate() {
        if simplex.contains(&i) {
            continue;
        }
        if let Some(f) = (0..4).find(|&f| dot(faces[f].n, q) - faces[f].d > eps) {
            faces[f].outside.push(i);
        }
    }

    let mut stack: Vec<usize> = (0..4).collect();
    let mut stamp: Vec<u32> = vec![0; 4];
    let mut tag = 0u32;
    let mut visible: Vec<usize> = Vec::new();
    let mut horizon: Vec<(usize, usize, usize)> = Vec::new();
    let mut bfs: Vec<usize> = Vec::new();
    while let Some(f) = stack.pop() {
        if !faces[f].alive || faces[f].outside.is_empty() {
            continue;
        }
        // The farthest outside point; the first one on ties.
        let (mut eye_at, mut eye_dist) = (0, f64::NEG_INFINITY);
        for (k, &i) in faces[f].outside.iter().enumerate() {
            let dist = dot(faces[f].n, p[i]) - faces[f].d;
            if dist > eye_dist {
                (eye_at, eye_dist) = (k, dist);
            }
        }
        let eye = faces[f].outside[eye_at];
        let e = p[eye];
        // The faces the eye sees, from `f` across neighbours, and the
        // horizon: the edges between a seen face and an unseen one.
        tag += 1;
        stamp.resize(faces.len(), 0);
        visible.clear();
        horizon.clear();
        bfs.clear();
        bfs.push(f);
        stamp[f] = tag;
        // Faces confirmed unseen this round carry `tag + 1` meaning "seen
        // and rejected"; a face is visited once.
        let rejected = tag.wrapping_add(1_000_000_000);
        while let Some(g) = bfs.pop() {
            visible.push(g);
            for k in 0..3 {
                let nb = faces[g].adj[k];
                if stamp[nb] == tag {
                    continue;
                }
                if stamp[nb] != rejected && dot(faces[nb].n, e) - faces[nb].d > eps {
                    stamp[nb] = tag;
                    bfs.push(nb);
                } else {
                    stamp[nb] = rejected;
                    horizon.push((faces[g].v[k], faces[g].v[(k + 1) % 3], nb));
                }
            }
        }
        // The horizon must be one simple loop; if rounding broke it, drop
        // this point (it lies within rounding of the hull) and go on.
        let simple = {
            let mut starts: Vec<usize> = horizon.iter().map(|h| h.0).collect();
            let mut ends: Vec<usize> = horizon.iter().map(|h| h.1).collect();
            starts.sort_unstable();
            ends.sort_unstable();
            let unique = starts.windows(2).all(|w| w[0] != w[1]);
            unique && starts == ends && horizon.len() >= 3
        };
        if !simple {
            faces[f].outside.swap_remove(eye_at);
            stack.push(f);
            continue;
        }
        let first_new = faces.len();
        for &(a, b, nb) in &horizon {
            let mut nf = make_face(p, [a, b, eye]);
            nf.adj[0] = nb;
            let id = faces.len();
            if let Some(k) = (0..3).find(|&k| faces[nb].v[k] == b && faces[nb].v[(k + 1) % 3] == a)
            {
                faces[nb].adj[k] = id;
            }
            faces.push(nf);
        }
        let new_count = horizon.len();
        for j in 0..new_count {
            let (a, b, _) = horizon[j];
            // Across `b → eye` lies the new face starting at `b`; across
            // `eye → a`, the new face ending at `a`.
            let after = (0..new_count).find(|&m| horizon[m].0 == b).unwrap_or(j);
            let before = (0..new_count).find(|&m| horizon[m].1 == a).unwrap_or(j);
            faces[first_new + j].adj[1] = first_new + after;
            faces[first_new + j].adj[2] = first_new + before;
        }
        let mut orphans: Vec<usize> = Vec::new();
        for &g in &visible {
            faces[g].alive = false;
            orphans.append(&mut faces[g].outside);
        }
        for i in orphans {
            if i == eye {
                continue;
            }
            if let Some(nf) =
                (first_new..faces.len()).find(|&nf| dot(faces[nf].n, p[i]) - faces[nf].d > eps)
            {
                faces[nf].outside.push(i);
            }
        }
        stack.extend(first_new..faces.len());
    }

    let mut used = vec![false; p.len()];
    let mut out_faces = Vec::new();
    for f in faces.iter().filter(|f| f.alive) {
        let [a, b, c] = f.v;
        used[a] = true;
        used[b] = true;
        used[c] = true;
        let area = norm(cross(sub(p[b], p[a]), sub(p[c], p[a]))) / 2.0;
        out_faces.push(HullFace { normal: f.n, area });
    }
    Hull {
        points: (0..p.len()).filter(|&i| used[i]).map(|i| p[i]).collect(),
        faces: out_faces,
    }
}

/// The 2D convex hull of `pts`, counter-clockwise, without repeated points
/// (Andrew's monotone chain).
pub(crate) fn hull_2d(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let turn = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut h: Vec<[f64; 2]> = Vec::with_capacity(pts.len() + 1);
    for &q in &pts {
        while h.len() >= 2 && turn(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
            h.pop();
        }
        h.push(q);
    }
    let lower = h.len() + 1;
    for &q in pts.iter().rev().skip(1) {
        while h.len() >= lower && turn(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
            h.pop();
        }
        h.push(q);
    }
    h.pop();
    h
}

/// The area of a counter-clockwise polygon.
#[cfg(test)]
pub(crate) fn polygon_area(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a[0] * b[1] - a[1] * b[0]
        })
        .sum::<f64>()
        / 2.0
}

/// The smallest width of a convex polygon: the least distance between two
/// parallel lines that hold it.
pub(crate) fn min_width(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len == 0.0 {
                return f64::INFINITY;
            }
            poly.iter()
                .map(|q| ((q[0] - a[0]) * dy - (q[1] - a[1]) * dx).abs() / len)
                .fold(0.0, f64::max)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Whether the polygon, turned by `deg` about the vertical (counter-clockwise
/// seen from above), fits a `width` × `depth` rectangle.
pub(crate) fn fits_turned(poly: &[[f64; 2]], width: f64, depth: f64, deg: f64) -> bool {
    let tolerance = 1e-6;
    let [w, d] = turned_extent(poly, deg);
    w <= width + tolerance && d <= depth + tolerance
}

/// The width and depth of the polygon's bounding rectangle once turned by
/// `deg`.
fn turned_extent(poly: &[[f64; 2]], deg: f64) -> [f64; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for q in poly {
        let r = [c * q[0] - s * q[1], s * q[0] + c * q[1]];
        for k in 0..2 {
            lo[k] = lo[k].min(r[k]);
            hi[k] = hi[k].max(r[k]);
        }
    }
    [hi[0] - lo[0], hi[1] - lo[1]]
}

/// The smallest turn about the vertical, in degrees within ±90°, after which
/// the polygon fits a `width` × `depth` rectangle; `None` when no turn does.
/// Turns are tried in half-degree steps, nearest to no turn first.
pub(crate) fn fit_turn_deg(poly: &[[f64; 2]], width: f64, depth: f64) -> Option<f64> {
    if fits_turned(poly, width, depth, 0.0) {
        return Some(0.0);
    }
    (1..=180).find_map(|step| {
        let deg = step as f64 * 0.5;
        [deg, -deg]
            .into_iter()
            .find(|&d| fits_turned(poly, width, depth, d))
    })
}

/// The turn about the vertical, in degrees within (−90°, 90°], that lays a
/// convex polygon square: its smallest bounding rectangle (one side along a
/// polygon edge) ends up along the axes, the long side along X, or along Y
/// when `long_along_y`. The smallest turn wins among equals.
pub(crate) fn square_turn_deg(poly: &[[f64; 2]], long_along_y: bool) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let normal = |deg: f64| {
        let d = deg.rem_euclid(180.0);
        if d > 90.0 { d - 180.0 } else { d }
    };
    let mut best: Option<(f64, f64)> = None;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        if dx == 0.0 && dy == 0.0 {
            continue;
        }
        let along = -dy.atan2(dx).to_degrees();
        let [w, d] = turned_extent(poly, along);
        let quarter = if (w < d) != long_along_y { 90.0 } else { 0.0 };
        let turn = normal(along + quarter);
        let area = w * d;
        let better = match best {
            None => true,
            Some((best_area, best_turn)) => {
                area < best_area * (1.0 - 1e-9)
                    || (area <= best_area * (1.0 + 1e-9) && turn.abs() < best_turn.abs())
            }
        };
        if better {
            best = Some((area, turn));
        }
    }
    best.map_or(0.0, |(_, turn)| if turn.abs() < 1e-9 { 0.0 } else { turn })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 11) as f64) / ((1u64 << 53) as f64)
    }

    #[test]
    fn cube_corners_are_the_hull_of_a_filled_cube() {
        let mut seed = 7;
        let mut pts: Vec<V3> = (0..5000)
            .map(|_| [lcg(&mut seed), lcg(&mut seed), lcg(&mut seed)])
            .collect();
        for x in [0.0, 1.0] {
            for y in [0.0, 1.0] {
                for z in [0.0, 1.0] {
                    pts.push([x, y, z]);
                }
            }
        }
        let h = quickhull(&pts);
        assert_eq!(h.points.len(), 8);
        let area: f64 = h.faces.iter().map(|f| f.area).sum();
        assert!((area - 6.0).abs() < 1e-9, "{area}");
    }

    #[test]
    fn sphere_points_all_on_the_hull_and_blocks_agree() {
        let mut seed = 11;
        let pts: Vec<V3> = (0..200_000)
            .map(|_| {
                let v = [
                    lcg(&mut seed) - 0.5,
                    lcg(&mut seed) - 0.5,
                    lcg(&mut seed) - 0.5,
                ];
                unit(v).map(|u| scale(u, 10.0)).unwrap_or([10.0, 0.0, 0.0])
            })
            .collect();
        let h = convex_hull(&pts);
        // Points on a sphere are all extreme.
        assert!(h.points.len() > 190_000, "{}", h.points.len());
        for u in [[1.0, 0.0, 0.0], [0.0, 0.6, 0.8], [-0.48, 0.6, -0.64]] {
            let (lo, hi) = h.extent(u);
            let (elo, ehi) = pts
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                    (a.min(dot(*p, u)), b.max(dot(*p, u)))
                });
            assert_eq!((lo, hi), (elo, ehi));
        }
        let area: f64 = h.faces.iter().map(|f| f.area).sum();
        let sphere = 4.0 * std::f64::consts::PI * 100.0;
        assert!((area - sphere).abs() / sphere < 1e-3, "{area}");
    }

    #[test]
    fn flat_input_is_returned_as_is() {
        let pts: Vec<V3> = (0..10).map(|i| [i as f64, (i * i) as f64, 0.0]).collect();
        let h = quickhull(&pts);
        assert_eq!(h.points.len(), 10);
        assert!(h.faces.is_empty());
    }

    #[test]
    fn hull_2d_area_width_and_fit() {
        let square = hull_2d(vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [0.0, 2.0],
            [1.0, 1.0],
        ]);
        assert_eq!(square.len(), 4);
        assert!((polygon_area(&square) - 4.0).abs() < 1e-12);
        assert!((min_width(&square) - 2.0).abs() < 1e-12);
        // A 110 × 5 rod fits a 100 × 60 plate only when turned.
        let rod = hull_2d(vec![[0.0, 0.0], [110.0, 0.0], [110.0, 5.0], [0.0, 5.0]]);
        let turn = fit_turn_deg(&rod, 100.0, 60.0).expect("fits diagonally");
        assert!(turn.abs() > 10.0 && turn.abs() < 40.0, "{turn}");
        assert_eq!(fit_turn_deg(&rod, 50.0, 50.0), None);
    }

    #[test]
    fn a_turned_rectangle_is_laid_square() {
        let rect = |deg: f64| {
            let (s, c) = deg.to_radians().sin_cos();
            hull_2d(
                [[0.0, 0.0], [40.0, 0.0], [40.0, 10.0], [0.0, 10.0]]
                    .iter()
                    .map(|p| [c * p[0] - s * p[1] + 7.0, s * p[0] + c * p[1] - 3.0])
                    .collect(),
            )
        };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        // Turned 30°: 30° back lays it along X, 60° more along Y.
        assert!(close(square_turn_deg(&rect(30.0), false), -30.0));
        assert!(close(square_turn_deg(&rect(30.0), true), 60.0));
        assert!(close(square_turn_deg(&rect(-20.0), false), 20.0));
        // Already square: no turn.
        assert_eq!(square_turn_deg(&rect(0.0), false), 0.0);
        // Along Y already, wanted along X: a quarter turn.
        assert!(close(square_turn_deg(&rect(90.0), false).abs(), 90.0));
        let [w, d] = turned_extent(&rect(30.0), square_turn_deg(&rect(30.0), false));
        assert!(close(w, 40.0) && close(d, 10.0), "{w} × {d}");
    }
}
