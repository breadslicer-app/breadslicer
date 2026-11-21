//! Each layer of a posed model as a mask of pixels.
//!
//! A layer is cut at its top (a hair above it), the cut's segments joined
//! into outlines where they share an end, and each outline filled on its
//! own by the even-odd rule at the pixel centres; a layer is the union of
//! its filled outlines. Pixel rows run from the model's back (`+y`) to its
//! front.

use rayon::prelude::*;

use crate::mask::Mask;

/// How close two numbers must be to count as the same (mm).
const EPS: f64 = 1e-6;

/// Where the grid of a scan lies: its first column's left edge, its first
/// row's top edge (world `y`), its pixel, and its size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub origin_x: f64,
    pub top_y: f64,
    pub px_mm: f64,
    pub width: i32,
    pub height: i32,
}

/// The masks of layers `0..count`, layer `l` cut at `base_z + (l + 1) ·
/// layer_mm` (and a hair above).
pub fn layer_masks(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    grid: Grid,
    base_z: f64,
    layer_mm: f64,
    count: usize,
) -> Vec<Mask> {
    // The triangles by their lowest point, to find the ones a cut crosses.
    let mut spans: Vec<(f64, f64, u32)> = triangles
        .iter()
        .enumerate()
        .map(|(t, tri)| {
            let z = tri.map(|i| f64::from(positions[i as usize][2]));
            (z[0].min(z[1]).min(z[2]), z[0].max(z[1]).max(z[2]), t as u32)
        })
        .collect();
    spans.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let tallest = spans.iter().map(|s| s.1 - s.0).fold(0.0f64, f64::max);
    (0..count)
        .into_par_iter()
        .map(|l| {
            let z = base_z + (l as f64 + 1.0) * layer_mm + 1e-6 + 1e-5;
            let from = spans.partition_point(|s| s.0 < z - tallest - 20.0 * EPS);
            let to = spans.partition_point(|s| s.0 <= z + 20.0 * EPS);
            let segments: Vec<[[f64; 2]; 2]> = spans[from..to]
                .iter()
                .filter(|s| s.1 >= z - 20.0 * EPS)
                .filter_map(|s| cut(positions, triangles[s.2 as usize], z))
                .collect();
            fill(&segments, grid)
        })
        .collect()
}

/// Where the plane `z` cuts a triangle, as a segment in the mask's plane
/// (`y` turned to point to the front), when it cuts two of its edges.
fn cut(positions: &[[f32; 3]], tri: [u32; 3], z: f64) -> Option<[[f64; 2]; 2]> {
    let p = tri.map(|i| positions[i as usize].map(f64::from));
    let above = p.iter().all(|q| q[2] >= z + 10.0 * EPS);
    let below = p.iter().all(|q| q[2] <= z - 10.0 * EPS);
    if above || below {
        return None;
    }
    let mut points = [[0.0; 2]; 3];
    let mut n = 0;
    for k in 0..3 {
        let (a, b) = (p[k], p[(k + 1) % 3]);
        let dz = b[2] - a[2];
        if dz.abs() < EPS {
            continue;
        }
        let t = (z - a[2]) / dz;
        if !(-EPS..=1.0 + EPS).contains(&t) {
            continue;
        }
        points[n] = [a[0] + t * (b[0] - a[0]), -(a[1] + t * (b[1] - a[1]))];
        n += 1;
    }
    (n == 2).then_some([points[0], points[1]])
}

/// The ends' key: segments meeting within 1e-5 mm are one outline.
fn key(p: [f64; 2]) -> (i64, i64) {
    ((p[0] * 1e5).round() as i64, (p[1] * 1e5).round() as i64)
}

/// The outlines' union: segments grouped by shared ends, each group filled
/// even-odd at the pixel centres.
fn fill(segments: &[[[f64; 2]; 2]], grid: Grid) -> Mask {
    let mut mask = Mask::empty(grid.width, grid.height);
    if segments.is_empty() {
        return mask;
    }
    // Group the segments through their ends.
    let mut ends: Vec<((i64, i64), u32)> = segments
        .iter()
        .enumerate()
        .flat_map(|(i, s)| [(key(s[0]), i as u32), (key(s[1]), i as u32)])
        .collect();
    ends.sort_unstable();
    let mut parent: Vec<u32> = (0..segments.len() as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            let up = parent[parent[i as usize] as usize];
            parent[i as usize] = up;
            i = up;
        }
        i
    }
    for run in ends.chunk_by(|a, b| a.0 == b.0) {
        for pair in run.windows(2) {
            let (a, b) = (root(&mut parent, pair[0].1), root(&mut parent, pair[1].1));
            if a != b {
                parent[a.max(b) as usize] = a.min(b);
            }
        }
    }
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut group_of = vec![u32::MAX; segments.len()];
    for i in 0..segments.len() as u32 {
        let r = root(&mut parent, i) as usize;
        if group_of[r] == u32::MAX {
            group_of[r] = groups.len() as u32;
            groups.push(Vec::new());
        }
        groups[group_of[r] as usize].push(i);
    }

    // Row `y`'s centre lies at mask coordinate `top + (y + 0.5) · px` (the
    // mask's `y` is the world's turned).
    let row_origin = -grid.top_y + grid.px_mm * 0.5;
    let mut spans: Vec<Vec<(i32, i32)>> = vec![Vec::new(); grid.height as usize];
    let mut crossings: Vec<Vec<f64>> = vec![Vec::new(); grid.height as usize];
    for group in groups {
        if group.len() < 2 {
            continue;
        }
        for c in crossings.iter_mut() {
            c.clear();
        }
        let mut touched: Vec<usize> = Vec::new();
        for &s in &group {
            let [a, b] = segments[s as usize];
            let to_px = |q: [f64; 2]| {
                [
                    (q[0] - grid.origin_x) / grid.px_mm,
                    (q[1] - row_origin) / grid.px_mm,
                ]
            };
            let (a, b) = (to_px(a), to_px(b));
            if (a[1] - b[1]).abs() < 1e-6 {
                continue;
            }
            let (lo, hi) = if a[1] < b[1] { (a, b) } else { (b, a) };
            let slope = (hi[0] - lo[0]) / (hi[1] - lo[1]);
            // Rows whose centre the segment spans, its top included.
            let first = lo[1].ceil().max(0.0) as i64;
            let mut row = first;
            while (row as f64) < hi[1] && row < i64::from(grid.height) {
                let r = row as usize;
                if crossings[r].is_empty() {
                    touched.push(r);
                }
                crossings[r].push(lo[0] + slope * (row as f64 - lo[1]));
                row += 1;
            }
        }
        for r in touched {
            let xs = &mut crossings[r];
            xs.sort_by(f64::total_cmp);
            for pair in xs.as_chunks::<2>().0 {
                // Pixels whose centre lies in [a, b).
                let a = ((pair[0] - 0.5).ceil() as i64).max(0);
                let b = ((pair[1] - 0.5).ceil() as i64).min(i64::from(grid.width));
                if a < b {
                    spans[r].push((a as i32, b as i32));
                }
            }
        }
    }
    for (row, mut s) in mask.rows.iter_mut().zip(spans) {
        s.sort_unstable();
        for (a, b) in s {
            match row.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => row.push((a, b)),
            }
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 × 2 box from z 0 to 1, as triangles.
    fn block() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let c = |i: usize| {
            [
                if i & 1 == 0 { 0.0 } else { 4.0 },
                if i & 2 == 0 { 0.0 } else { 2.0 },
                if i & 4 == 0 { 0.0 } else { 1.0 },
            ]
        };
        let positions = (0..8).map(c).collect();
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let triangles = quads
            .iter()
            .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
            .collect();
        (positions, triangles)
    }

    #[test]
    fn a_box_fills_its_pixels_on_every_layer_it_crosses() {
        let (positions, triangles) = block();
        let grid = Grid {
            origin_x: 0.0,
            top_y: 2.0,
            px_mm: 0.5,
            width: 8,
            height: 4,
        };
        let masks = layer_masks(&positions, &triangles, grid, 0.0, 0.25, 4);
        for (l, m) in masks.iter().enumerate().take(3) {
            assert!(
                m.rows.iter().all(|r| r == &vec![(0, 8)]),
                "layer {l}: {m:?}"
            );
        }
        // The last cut is just over the top: nothing.
        assert!(masks[3].rows.iter().all(|r| r.is_empty()));
    }
}
