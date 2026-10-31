/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Closed test shapes, outward faces, in millimetres.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};

#[derive(Default)]
pub(crate) struct Shape {
    v: Vec<[f64; 3]>,
    f: Vec<[u32; 3]>,
}

impl Shape {
    pub(crate) fn mesh(&self) -> IndexedMesh {
        IndexedMesh::new(
            None,
            self.v
                .iter()
                .map(|p| Vertex::new(p.map(|x| x as f32)))
                .collect(),
            self.f
                .iter()
                .map(|t| IndexedTriangle::from_raw_indices(*t))
                .collect(),
        )
    }

    fn vertex(&mut self, p: [f64; 3]) -> u32 {
        self.v.push(p);
        (self.v.len() - 1) as u32
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.f.push([a, b, c]);
        self.f.push([a, c, d]);
    }

    /// An axis-aligned box, each side split in `n` × `n` quads.
    pub(crate) fn boxed(mut self, lo: [f64; 3], hi: [f64; 3], n: usize) -> Self {
        // Each side: an origin corner and two edge vectors whose cross
        // product points out.
        let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        let sides: [([f64; 3], [f64; 3], [f64; 3]); 6] = [
            (lo, [0.0, d[1], 0.0], [d[0], 0.0, 0.0]),
            ([lo[0], lo[1], hi[2]], [d[0], 0.0, 0.0], [0.0, d[1], 0.0]),
            (lo, [d[0], 0.0, 0.0], [0.0, 0.0, d[2]]),
            ([lo[0], hi[1], lo[2]], [0.0, 0.0, d[2]], [d[0], 0.0, 0.0]),
            (lo, [0.0, 0.0, d[2]], [0.0, d[1], 0.0]),
            ([hi[0], lo[1], lo[2]], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]),
        ];
        // Shared vertices along the box edges keep it closed: weld by
        // position.
        let mut index = std::collections::HashMap::<[i64; 3], u32>::new();
        for (o, a, b) in sides {
            let mut grid = vec![0u32; (n + 1) * (n + 1)];
            for i in 0..=n {
                for j in 0..=n {
                    let (s, t) = (i as f64 / n as f64, j as f64 / n as f64);
                    let p = [0, 1, 2].map(|k| o[k] + s * a[k] + t * b[k]);
                    let key = p.map(|x| (x * 1e6).round() as i64);
                    let id = *index.entry(key).or_insert_with(|| {
                        self.v.push(p);
                        (self.v.len() - 1) as u32
                    });
                    grid[i * (n + 1) + j] = id;
                }
            }
            for i in 0..n {
                for j in 0..n {
                    let g = |i: usize, j: usize| grid[i * (n + 1) + j];
                    self.quad(g(i, j), g(i + 1, j), g(i + 1, j + 1), g(i, j + 1));
                }
            }
        }
        self
    }

    /// A sphere of radius `r` around `c`.
    pub(crate) fn sphere(mut self, c: [f64; 3], r: f64, segments: usize, rings: usize) -> Self {
        let top = self.vertex([c[0], c[1], c[2] + r]);
        let mut rows = Vec::new();
        for i in 1..rings {
            let th = PI * i as f64 / rings as f64;
            let row: Vec<u32> = (0..segments)
                .map(|j| {
                    let ph = TAU * j as f64 / segments as f64;
                    self.vertex([
                        c[0] + r * th.sin() * ph.cos(),
                        c[1] + r * th.sin() * ph.sin(),
                        c[2] + r * th.cos(),
                    ])
                })
                .collect();
            rows.push(row);
        }
        let bottom = self.vertex([c[0], c[1], c[2] - r]);
        for j in 0..segments {
            let k = (j + 1) % segments;
            self.f.push([top, rows[0][j], rows[0][k]]);
            for w in rows.windows(2) {
                self.quad(w[0][j], w[1][j], w[1][k], w[0][k]);
            }
            let last = &rows[rows.len() - 1];
            self.f.push([bottom, last[k], last[j]]);
        }
        self
    }

    /// A solid of revolution about Z: `profile` runs from the bottom axis
    /// point out and up to the top axis point, as `(radius, z)`.
    pub(crate) fn lathe(mut self, c: [f64; 2], profile: &[(f64, f64)], segments: usize) -> Self {
        let n = profile.len();
        let bottom = self.vertex([c[0], c[1], profile[0].1]);
        let rings: Vec<Vec<u32>> = profile[1..n - 1]
            .iter()
            .map(|&(r, z)| {
                (0..segments)
                    .map(|j| {
                        let ph = TAU * j as f64 / segments as f64;
                        self.vertex([c[0] + r * ph.cos(), c[1] + r * ph.sin(), z])
                    })
                    .collect()
            })
            .collect();
        let top = self.vertex([c[0], c[1], profile[n - 1].1]);
        for j in 0..segments {
            let k = (j + 1) % segments;
            self.f.push([bottom, rings[0][k], rings[0][j]]);
            for w in rings.windows(2) {
                self.quad(w[0][j], w[0][k], w[1][k], w[1][j]);
            }
            let last = &rings[rings.len() - 1];
            self.f.push([top, last[j], last[k]]);
        }
        self
    }

    /// A hemispherical bowl, outer radius `r_out`, inner `r_in`, rim at
    /// z = 0 and open towards +Z.
    pub(crate) fn bowl(mut self, r_out: f64, r_in: f64, segments: usize, rings: usize) -> Self {
        // Rows from the rim (θ = 0) down to just above the bottom pole.
        let rows = |s: &mut Self, r: f64| -> (Vec<Vec<u32>>, u32) {
            let mut out = Vec::new();
            for i in 0..rings {
                let th = FRAC_PI_2 * i as f64 / rings as f64;
                out.push(
                    (0..segments)
                        .map(|j| {
                            let ph = TAU * j as f64 / segments as f64;
                            s.vertex([
                                r * th.cos() * ph.cos(),
                                r * th.cos() * ph.sin(),
                                -r * th.sin(),
                            ])
                        })
                        .collect::<Vec<u32>>(),
                );
            }
            let pole = s.vertex([0.0, 0.0, -r]);
            (out, pole)
        };
        let (outer, op) = rows(&mut self, r_out);
        let (inner, ip) = rows(&mut self, r_in);
        for j in 0..segments {
            let k = (j + 1) % segments;
            for w in outer.windows(2) {
                self.quad(w[0][j], w[1][j], w[1][k], w[0][k]);
            }
            for w in inner.windows(2) {
                self.quad(w[0][j], w[0][k], w[1][k], w[1][j]);
            }
            let (lo, li) = (&outer[rings - 1], &inner[rings - 1]);
            self.f.push([op, lo[k], lo[j]]);
            self.f.push([ip, li[j], li[k]]);
            // The rim, facing up.
            self.quad(outer[0][j], outer[0][k], inner[0][k], inner[0][j]);
        }
        self
    }
}

/// A miniature, in one piece: a round base 25 mm across and 3 mm thick,
/// with a figure 8 mm across standing 25 mm on it, its head wider.
pub(crate) fn mini() -> IndexedMesh {
    let profile = [
        (0.0, 0.0),
        (12.5, 0.0),
        (12.5, 3.0),
        (4.0, 3.0),
        (4.0, 14.0),
        (3.0, 20.0),
        (5.0, 24.0),
        (4.0, 28.0),
        (0.0, 28.0),
    ];
    Shape::default().lathe([0.0, 0.0], &profile, 48).mesh()
}

/// Three blocks of different sizes, with no symmetry.
pub(crate) fn lopsided() -> IndexedMesh {
    Shape::default()
        .boxed([0.0, 0.0, 0.0], [30.0, 20.0, 8.0], 8)
        .boxed([0.0, 0.0, 8.0], [8.0, 8.0, 28.0], 6)
        .boxed([30.0, 14.0, 0.0], [42.0, 20.0, 5.0], 4)
        .mesh()
}
