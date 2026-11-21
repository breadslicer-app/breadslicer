//! Questions asked of the model's surface along vertical lines: its lowest
//! points, whether a point is inside, and where a contact lands on it.

use rayon::prelude::*;

/// The triangles in a grid of cells over the plan, for vertical lines.
pub(crate) struct Plan<'a> {
    positions: &'a [[f32; 3]],
    triangles: &'a [[u32; 3]],
    min: [f32; 2],
    cell: f32,
    size: [usize; 2],
    /// `starts[c]..starts[c + 1]` bounds cell `c`'s triangles in `faces`.
    starts: Vec<u32>,
    faces: Vec<u32>,
}

/// How far outside its plan a triangle is still tried (mm): far above the
/// rounding of the test.
const PLAN_MARGIN_MM: f32 = 0.01;

impl<'a> Plan<'a> {
    pub(crate) fn new(positions: &'a [[f32; 3]], triangles: &'a [[u32; 3]]) -> Self {
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for p in positions {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let n = triangles.len().max(1) as f32;
        let span = [(hi[0] - lo[0]).max(1e-3), (hi[1] - lo[1]).max(1e-3)];
        // About four triangles a cell, if they spread evenly.
        let cell = (span[0] * span[1] / n * 4.0).sqrt().max(1e-3);
        let size = [
            ((span[0] / cell).ceil() as usize).clamp(1, 4096),
            ((span[1] / cell).ceil() as usize).clamp(1, 4096),
        ];
        let cell = (span[0] / size[0] as f32).max(span[1] / size[1] as f32);
        let mut plan = Self {
            positions,
            triangles,
            min: lo,
            cell,
            size,
            starts: Vec::new(),
            faces: Vec::new(),
        };
        let mut pairs: Vec<(u32, u32)> = triangles
            .par_iter()
            .enumerate()
            .flat_map_iter(|(t, tri)| {
                let p = tri.map(|i| positions[i as usize]);
                let r = plan.range(
                    p[0][0].min(p[1][0]).min(p[2][0]) - PLAN_MARGIN_MM,
                    p[0][0].max(p[1][0]).max(p[2][0]) + PLAN_MARGIN_MM,
                    p[0][1].min(p[1][1]).min(p[2][1]) - PLAN_MARGIN_MM,
                    p[0][1].max(p[1][1]).max(p[2][1]) + PLAN_MARGIN_MM,
                );
                let width = plan.size[0];
                r.flat_map(move |(x, y)| std::iter::once(((y * width + x) as u32, t as u32)))
            })
            .collect();
        pairs.par_sort_unstable();
        let cells = size[0] * size[1];
        let mut starts = vec![0u32; cells + 1];
        for &(c, _) in &pairs {
            starts[c as usize + 1] += 1;
        }
        for c in 0..cells {
            starts[c + 1] += starts[c];
        }
        plan.starts = starts;
        plan.faces = pairs.into_iter().map(|p| p.1).collect();
        plan
    }

    fn index(&self, v: f32, k: usize) -> usize {
        (((v - self.min[k]) / self.cell).floor().max(0.0) as usize).min(self.size[k] - 1)
    }

    fn range(
        &self,
        x0: f32,
        x1: f32,
        y0: f32,
        y1: f32,
    ) -> impl Iterator<Item = (usize, usize)> + use<> {
        let (a, b) = (self.index(x0, 0), self.index(x1, 0));
        let (c, d) = (self.index(y0, 1), self.index(y1, 1));
        (c..=d).flat_map(move |y| (a..=b).map(move |x| (x, y)))
    }

    /// Where the vertical line through `(x, y)` meets each triangle it
    /// crosses: `(z, triangle, its unit normal's z)`.
    pub(crate) fn heights(&self, x: f32, y: f32) -> Vec<(f32, u32, f32)> {
        let c = self.index(y, 1) * self.size[0] + self.index(x, 0);
        self.faces[self.starts[c] as usize..self.starts[c + 1] as usize]
            .iter()
            .filter_map(|&t| {
                let [a, b, c] = self.triangles[t as usize].map(|i| self.positions[i as usize]);
                height_through(x, y, a, b, c).map(|(z, nz)| (z, t, nz))
            })
            .collect()
    }

    /// Whether `(x, y, z)` is inside: an odd number of triangles below it on
    /// its vertical line.
    pub(crate) fn inside(&self, x: f32, y: f32, z: f32) -> bool {
        self.heights(x, y).iter().filter(|h| h.0 < z - 1e-5).count() % 2 == 1
    }
}

/// The height of triangle `abc` over `(x, y)`, edges included, and its unit
/// normal's `z`; none when the line misses it or runs along it.
fn height_through(x: f32, y: f32, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<(f32, f32)> {
    let (u, v) = (
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
        [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
    );
    let nz = u[0] * v[1] - u[1] * v[0];
    if nz.abs() < 1e-8 {
        return None;
    }
    let (px, py) = (x - a[0], y - a[1]);
    let s = (px * v[1] - py * v[0]) / nz;
    let t = (u[0] * py - u[1] * px) / nz;
    if s < 0.0 || t < 0.0 || s + t > 1.0 {
        return None;
    }
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], nz];
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    Some((a[2] + s * u[2] + t * v[2], nz / length))
}

/// A vertex lower than every vertex within `rings` edges of it, whose
/// surface around it faces down or which stands outside the model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LowPoint {
    pub vertex: u32,
    pub position: [f32; 3],
    /// The last triangle listing it.
    pub face: u32,
}

pub(crate) fn low_points(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    rings: usize,
    plan: &Plan<'_>,
) -> Vec<LowPoint> {
    let n = positions.len();
    let mut pairs: Vec<(u32, u32)> = triangles
        .par_iter()
        .flat_map_iter(|&[a, b, c]| [(a, b), (a, c), (b, a), (b, c), (c, a), (c, b)])
        .collect();
    pairs.par_sort_unstable();
    pairs.dedup();
    let mut first = vec![0u32; n + 1];
    for &(a, _) in &pairs {
        first[a as usize + 1] += 1;
    }
    for v in 0..n {
        first[v + 1] += first[v];
    }
    let next: Vec<u32> = pairs.into_iter().map(|p| p.1).collect();
    let around = |v: usize| &next[first[v] as usize..first[v + 1] as usize];
    let mut last_face = vec![u32::MAX; n];
    let mut normal = vec![[0.0f32; 3]; n];
    for (f, tri) in triangles.iter().enumerate() {
        let [a, b, c] = tri.map(|i| positions[i as usize]);
        let (u, w) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let cross = [
            u[1] * w[2] - u[2] * w[1],
            u[2] * w[0] - u[0] * w[2],
            u[0] * w[1] - u[1] * w[0],
        ];
        let length = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
        for &v in tri {
            last_face[v as usize] = f as u32;
            if length > 0.0 {
                for k in 0..3 {
                    normal[v as usize][k] += cross[k] / length;
                }
            }
        }
    }
    (0..n)
        .into_par_iter()
        .filter_map(|v| {
            let z = positions[v][2];
            // The rings around `v`, breadth first.
            let mut seen = vec![v as u32];
            let mut ring = vec![v as u32];
            for _ in 0..rings {
                let mut out = Vec::new();
                for &u in &ring {
                    for &w in around(u as usize) {
                        if !seen.contains(&w) {
                            seen.push(w);
                            out.push(w);
                        }
                    }
                }
                if out.is_empty() {
                    break;
                }
                ring = out;
            }
            if seen.len() == 1 || seen[1..].iter().any(|&w| positions[w as usize][2] <= z) {
                return None;
            }
            let m = normal[v];
            let length = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
            let nz = if length > 0.0 { m[2] / length } else { 0.0 };
            let p = positions[v];
            if nz >= -0.05 && plan.inside(p[0] + 1.123e-5, p[1] + 2.456e-5, p[2] - 1e-4) {
                return None;
            }
            Some(LowPoint {
                vertex: v as u32,
                position: p,
                face: last_face[v],
            })
        })
        .collect()
}

/// `(x, y, z)` moved onto the surface within `reach` of it along the
/// vertical: to a surface facing down if one is in reach, else the nearest;
/// where it was when none is.
pub(crate) fn snapped(plan: &Plan<'_>, x: f32, y: f32, z: f32, reach: f32) -> [f32; 3] {
    let best = plan
        .heights(x, y)
        .into_iter()
        .filter(|h| (h.0 - z).abs() <= reach)
        .min_by(|a, b| {
            (a.2 >= 0.0)
                .cmp(&(b.2 >= 0.0))
                .then((a.0 - z).abs().total_cmp(&(b.0 - z).abs()))
                .then(a.1.cmp(&b.1))
        });
    [x, y, best.map_or(z, |h| h.0)]
}
