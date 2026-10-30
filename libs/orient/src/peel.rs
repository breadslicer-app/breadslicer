/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The cross-section of every layer, and the force it takes to peel it off
//! the film.
//!
//! Each triangle's cut segment at a layer, oriented by the triangle's normal
//! (`(b − a)·(u × n) > 0`), adds its shoelace term `½ (a × b)·u`: outer
//! contours add, holes subtract, and no contour has to be stitched. The
//! perimeter is the sum of the segment lengths.

use rayon::prelude::*;

use crate::vec::{V3, basis, cross, dot, sub};

/// How hard a layer pulls on the film as it peels: `σ·A + k·P`.
///
/// The structural analysis (#11) and the review use the same per-layer force
/// model, so "too much force" means the same thing everywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeelModel {
    /// Adhesion per area, N/mm². 15 kPa is a common FEP film; the range
    /// measured for one is 13–19 kPa.
    pub sigma_n_per_mm2: f64,
    /// Edge drag per perimeter length, N/mm.
    pub perimeter_n_per_mm: f64,
    /// Suction under a cup open towards the film, N/mm²: up to a vacuum.
    pub cup_n_per_mm2: f64,
    /// The layer spacing the profile is sampled at, mm. Peel peaks and area
    /// jumps are features of millimetres, not of one 50 µm layer.
    pub sample_mm: f64,
}

impl Default for PeelModel {
    fn default() -> Self {
        PeelModel {
            sigma_n_per_mm2: 0.015,
            perimeter_n_per_mm: 0.0005,
            cup_n_per_mm2: 0.1,
            sample_mm: 0.5,
        }
    }
}

/// Every sampled layer's cross-section area and perimeter, from the lowest
/// point up. Layer `k` is cut at `z0 + (k + ½)·step`.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerProfile {
    /// Height of the model's lowest point along the up direction, mm.
    pub z0: f64,
    /// Layer spacing, mm.
    pub step: f64,
    /// Cross-section area per layer, mm².
    pub area: Vec<f64>,
    /// Cross-section perimeter per layer, mm.
    pub perimeter: Vec<f64>,
}

/// Triangles per parallel block: each block fills its own profile, and the
/// profiles are added in block order, so the sums do not depend on the
/// thread count.
const BLOCK: usize = 1 << 15;

pub(crate) fn layer_profile(
    pos: &[V3],
    tri: &[[u32; 3]],
    up: V3,
    z0: f64,
    z1: f64,
    step: f64,
) -> LayerProfile {
    let layers = (((z1 - z0) / step).ceil() as usize).max(1);
    let (e1, e2) = basis(up);
    let empty = || (vec![0.0; layers], vec![0.0; layers]);
    let parts: Vec<(Vec<f64>, Vec<f64>)> = tri
        .par_chunks(BLOCK)
        .map(|block| {
            let (mut area, mut perim) = empty();
            for t in block {
                cut_triangle(pos, *t, up, e1, e2, z0, step, &mut area, &mut perim);
            }
            (area, perim)
        })
        .collect();
    let (mut area, mut perimeter) = empty();
    for (a, p) in parts {
        for k in 0..layers {
            area[k] += a[k];
            perimeter[k] += p[k];
        }
    }
    LayerProfile {
        z0,
        step,
        area,
        perimeter,
    }
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn cut_triangle(
    pos: &[V3],
    t: [u32; 3],
    up: V3,
    e1: V3,
    e2: V3,
    z0: f64,
    step: f64,
    area: &mut [f64],
    perim: &mut [f64],
) {
    let p = t.map(|i| pos[i as usize]);
    let z = p.map(|q| dot(q, up));
    let lo = z[0].min(z[1]).min(z[2]);
    let hi = z[0].max(z[1]).max(z[2]);
    let k0 = ((lo - z0) / step - 0.5).ceil().max(0.0) as usize;
    let k1 = ((hi - z0) / step - 0.5).floor();
    if k1 < k0 as f64 {
        return;
    }
    let k1 = (k1 as usize).min(area.len() - 1);
    let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
    // Segments run along `up × n`, which keeps the solid on their left.
    let side = cross(up, n);
    let sx = [dot(side, e1), dot(side, e2)];
    let xy = p.map(|q| [dot(q, e1), dot(q, e2)]);
    let mut ord = [0usize, 1, 2];
    ord.sort_by(|&a, &b| z[a].total_cmp(&z[b]));
    let [a, b, c] = ord;
    let lerp = |i: usize, j: usize, h: f64| {
        let s = (h - z[i]) / (z[j] - z[i]);
        [
            xy[i][0] + s * (xy[j][0] - xy[i][0]),
            xy[i][1] + s * (xy[j][1] - xy[i][1]),
        ]
    };
    for k in k0..=k1 {
        let h = z0 + (k as f64 + 0.5) * step;
        if h < lo || h > hi || z[c] == z[a] {
            continue;
        }
        let p1 = lerp(a, c, h);
        let p2 = if h < z[b] {
            lerp(a, b, h)
        } else {
            lerp(b, c, h)
        };
        let d = [p2[0] - p1[0], p2[1] - p1[1]];
        let (p1, p2) = if d[0] * sx[0] + d[1] * sx[1] >= 0.0 {
            (p1, p2)
        } else {
            (p2, p1)
        };
        area[k] += 0.5 * (p1[0] * p2[1] - p1[1] * p2[0]);
        perim[k] += (d[0] * d[0] + d[1] * d[1]).sqrt();
    }
}

/// The peel summary of one pose.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PeelSummary {
    /// The largest per-layer force, N, cups included.
    pub peak_force_n: f64,
    /// Height of that layer above the model's lowest point, mm.
    pub peak_at_mm: f64,
    /// The largest cross-section, mm².
    pub peak_area_mm2: f64,
    /// Layers whose area grows more than 35 % and 4 mm² over the one below:
    /// the sudden loads that tear thin supports.
    pub area_jumps: usize,
    /// The force those jumps add beyond that allowance, summed, N: the
    /// measure the cost weighs, since it changes smoothly with the pose
    /// where the count steps.
    pub jump_force_n: f64,
}

/// Area growth that counts as a jump: more than this factor ...
pub(crate) const JUMP_FACTOR: f64 = 1.35;
/// ... and more than this many mm².
pub(crate) const JUMP_MIN_MM2: f64 = 4.0;

/// Peak force and jumps of a profile; `cup_area(z)` adds the suction area of
/// a cup at height `z` above the lowest point.
pub(crate) fn summarize(
    profile: Option<&LayerProfile>,
    model: &PeelModel,
    cups: &[f64],
    cup_step: f64,
) -> PeelSummary {
    let cup_at = |z: f64| {
        if cups.is_empty() || cup_step <= 0.0 {
            0.0
        } else {
            let k = (z / cup_step).floor().max(0.0) as usize;
            cups.get(k).copied().unwrap_or(0.0)
        }
    };
    let mut s = PeelSummary::default();
    match profile {
        Some(p) => {
            for k in 0..p.area.len() {
                let z = (k as f64 + 0.5) * p.step;
                let a = p.area[k].max(0.0);
                let f = model.sigma_n_per_mm2 * a
                    + model.perimeter_n_per_mm * p.perimeter[k]
                    + model.cup_n_per_mm2 * cup_at(z);
                if f > s.peak_force_n {
                    s.peak_force_n = f;
                    s.peak_at_mm = z;
                }
                s.peak_area_mm2 = s.peak_area_mm2.max(a);
                if k > 0 {
                    let below = p.area[k - 1].max(0.0);
                    if a > JUMP_FACTOR * below && a - below > JUMP_MIN_MM2 {
                        s.area_jumps += 1;
                    }
                    let excess = (a - JUMP_FACTOR * below).min(a - below - JUMP_MIN_MM2);
                    s.jump_force_n += model.sigma_n_per_mm2 * excess.max(0.0);
                }
            }
        }
        None => {
            // An open mesh has no cross-section; only the cups pull.
            for (k, &c) in cups.iter().enumerate() {
                let f = model.cup_n_per_mm2 * c;
                if f > s.peak_force_n {
                    s.peak_force_n = f;
                    s.peak_at_mm = (k as f64 + 0.5) * cup_step;
                }
            }
        }
    }
    s
}
