/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! T6 and D3 (#185): supports that land on the model.
//!
//! A support grows straight down from an overhang to the plate. When the
//! model lies under it, it lands on the model instead: the support then
//! marks two surfaces, the overhang and the face it stands on (T6). When
//! that face is painted detail, the mark lands on the detail (D3), however
//! well the detail faces up: a brim or a hat above a face is enough.
//!
//! Each overhang face sends rays straight down (one from its middle, four
//! from the middles of its quarters when it holds more than a tip's
//! area), each standing for its share of the face's tips. The triangles are
//! projected on the plate and binned in a grid, so a ray only meets the
//! triangles of its cell: one pass over the mesh per pose.

use rayon::prelude::*;

use crate::histogram::overhang_weight;
use crate::vec::{V3, basis, cross, dot, sub, unit};

/// What lands on the model in one pose, in tips.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Landings {
    /// Tips whose support lands on the model (T6).
    pub(crate) on_model: f64,
    /// Of those, the tips whose support lands on painted detail (D3).
    pub(crate) on_paint: f64,
}

/// A triangle projected on the plate.
struct Flat {
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    /// Heights of the corners along the up direction.
    h: [f64; 3],
}

impl Flat {
    /// The height of the triangle over `q`, when `q` is inside it.
    fn height_at(&self, q: [f64; 2]) -> Option<f64> {
        let (a, b, c) = (self.a, self.b, self.c);
        let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if d.abs() < 1e-18 {
            return None;
        }
        let l0 = ((b[1] - c[1]) * (q[0] - c[0]) + (c[0] - b[0]) * (q[1] - c[1])) / d;
        let l1 = ((c[1] - a[1]) * (q[0] - c[0]) + (a[0] - c[0]) * (q[1] - c[1])) / d;
        let l2 = 1.0 - l0 - l1;
        let eps = -1e-9;
        (l0 >= eps && l1 >= eps && l2 >= eps)
            .then(|| l0 * self.h[0] + l1 * self.h[1] + l2 * self.h[2])
    }
}

/// The supports landing on the model with `u` on top: the overhangs past
/// `c0` (the cosine of the overhang angle against straight down), each
/// `area_per_tip` mm² a tip. `painted` marks the painted triangles of
/// `tri`, when the model has paint. Faces resting on the plate (within
/// `plate_band` of `z0`, with `resting`) send nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn landings(
    pos: &[V3],
    tri: &[[u32; 3]],
    u: V3,
    c0: f64,
    area_per_tip: f64,
    painted: Option<&[bool]>,
    resting: bool,
    z0: f64,
    plate_band: f64,
) -> Landings {
    if tri.is_empty() || area_per_tip <= 0.0 {
        return Landings::default();
    }
    let (e1, e2) = basis(u);
    let flat: Vec<Flat> = tri
        .par_iter()
        .with_min_len(1 << 12)
        .map(|t| {
            let p = t.map(|i| pos[i as usize]);
            Flat {
                a: [dot(p[0], e1), dot(p[0], e2)],
                b: [dot(p[1], e1), dot(p[1], e2)],
                c: [dot(p[2], e1), dot(p[2], e2)],
                h: p.map(|q| dot(q, u)),
            }
        })
        .collect();
    let up_facing: Vec<bool> = tri
        .par_iter()
        .with_min_len(1 << 12)
        .map(|t| {
            let p = t.map(|i| pos[i as usize]);
            dot(cross(sub(p[1], p[0]), sub(p[2], p[0])), u) > 0.0
        })
        .collect();

    // The grid: about two triangles per cell on average.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for f in &flat {
        for p in [f.a, f.b, f.c] {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let span = [(hi[0] - lo[0]).max(1e-6), (hi[1] - lo[1]).max(1e-6)];
    let side = ((span[0] * span[1]) / (flat.len() as f64 / 2.0))
        .sqrt()
        .max(1e-6);
    let dims = [
        ((span[0] / side).ceil() as usize).clamp(1, 2048),
        ((span[1] / side).ceil() as usize).clamp(1, 2048),
    ];
    let cell_of = |q: [f64; 2]| -> [usize; 2] {
        [0, 1].map(|k| (((q[k] - lo[k]) / span[k] * dims[k] as f64) as usize).min(dims[k] - 1))
    };
    // Compressed rows: how many in each cell, then the triangles.
    let mut count = vec![0u32; dims[0] * dims[1] + 1];
    let range = |f: &Flat| {
        let (mut a, mut b) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in [f.a, f.b, f.c] {
            for k in 0..2 {
                a[k] = a[k].min(p[k]);
                b[k] = b[k].max(p[k]);
            }
        }
        (cell_of(a), cell_of(b))
    };
    for f in &flat {
        let (a, b) = range(f);
        for y in a[1]..=b[1] {
            for x in a[0]..=b[0] {
                count[y * dims[0] + x + 1] += 1;
            }
        }
    }
    for i in 1..count.len() {
        count[i] += count[i - 1];
    }
    let mut fill = count.clone();
    let mut items = vec![0u32; count[count.len() - 1] as usize];
    for (t, f) in flat.iter().enumerate() {
        let (a, b) = range(f);
        for y in a[1]..=b[1] {
            for x in a[0]..=b[0] {
                let cell = y * dims[0] + x;
                items[fill[cell] as usize] = t as u32;
                fill[cell] += 1;
            }
        }
    }

    // The first face straight under `q`, below `height`: whether it is one
    // facing up, and which.
    let below = |q: [f64; 2], height: f64, from: usize| -> Option<usize> {
        let [x, y] = cell_of(q);
        let cell = y * dims[0] + x;
        let mut best: Option<(f64, usize)> = None;
        for &t in &items[count[cell] as usize..count[cell + 1] as usize] {
            let t = t as usize;
            if t == from {
                continue;
            }
            if let Some(h) = flat[t].height_at(q)
                && h < height - 1e-6
                && best.is_none_or(|(b, _)| h > b)
            {
                best = Some((h, t));
            }
        }
        best.and_then(|(_, t)| up_facing[t].then_some(t))
    };

    let parts: Vec<Landings> = tri
        .par_chunks(1 << 12)
        .enumerate()
        .map(|(block, chunk)| {
            let mut out = Landings::default();
            for (k, t) in chunk.iter().enumerate() {
                let i = block * (1 << 12) + k;
                let p = t.map(|v| pos[v as usize]);
                let n2 = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                let Some(n) = unit(n2) else { continue };
                let w = overhang_weight(-dot(n, u), c0);
                if w <= 0.0 {
                    continue;
                }
                if resting && p.iter().all(|q| dot(*q, u) - z0 <= plate_band) {
                    continue;
                }
                let tips = dot(n2, n) / 2.0 * w / area_per_tip;
                let f = &flat[i];
                let mid = |x: [f64; 2], y: [f64; 2], z: [f64; 2]| {
                    [(x[0] + y[0] + z[0]) / 3.0, (x[1] + y[1] + z[1]) / 3.0]
                };
                let hmid = |x: f64, y: f64, z: f64| (x + y + z) / 3.0;
                let samples: Vec<([f64; 2], f64)> = if tips > 1.0 {
                    let half =
                        |x: [f64; 2], y: [f64; 2]| [(x[0] + y[0]) / 2.0, (x[1] + y[1]) / 2.0];
                    let (ab, bc, ca) = (half(f.a, f.b), half(f.b, f.c), half(f.c, f.a));
                    let (hab, hbc, hca) = (
                        (f.h[0] + f.h[1]) / 2.0,
                        (f.h[1] + f.h[2]) / 2.0,
                        (f.h[2] + f.h[0]) / 2.0,
                    );
                    vec![
                        (mid(f.a, ab, ca), hmid(f.h[0], hab, hca)),
                        (mid(ab, f.b, bc), hmid(hab, f.h[1], hbc)),
                        (mid(ca, bc, f.c), hmid(hca, hbc, f.h[2])),
                        (mid(ab, bc, ca), hmid(hab, hbc, hca)),
                    ]
                } else {
                    vec![(mid(f.a, f.b, f.c), hmid(f.h[0], f.h[1], f.h[2]))]
                };
                let share = tips / samples.len() as f64;
                for (q, h) in samples {
                    if let Some(hit) = below(q, h, i) {
                        out.on_model += share;
                        if painted.is_some_and(|p| p.get(hit).copied().unwrap_or(false)) {
                            out.on_paint += share;
                        }
                    }
                }
            }
            out
        })
        .collect();
    parts
        .into_iter()
        .fold(Landings::default(), |a, b| Landings {
            on_model: a.on_model + b.on_model,
            on_paint: a.on_paint + b.on_paint,
        })
}
