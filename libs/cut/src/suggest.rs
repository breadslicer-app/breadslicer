/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where to cut a model: the narrowings of its cross-section (#443).
//!
//! A neck, a waist or an ankle is a small cross-section between two larger
//! ones: cut there, the parts meet on a small face that is easy to hide and
//! to glue. The model's cross-section area is measured at every height
//! along a direction (each triangle's cut adds its signed shoelace term, so
//! no contour is stitched), smoothed a little, and each local minimum much
//! smaller than the largest sections below and above it is a narrowing.
//! The narrower against its neighbours, the better the suggestion.
//!
//! This is the simplest of the signals the issue lists; creases from the
//! normals and part detection are left for later.

use sc_mesh_core::IndexedMesh;

use crate::{cross, dot};

/// A narrowing worth cutting at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Narrowing {
    /// Its height along the direction measured, mm, as `dot(p, up)`.
    pub at: f64,
    /// Its cross-section, mm².
    pub area_mm2: f64,
    /// How narrow it is against the smaller of the largest sections below
    /// and above it: 0 a point, 1 none at all.
    pub ratio: f64,
}

/// Layers the cross-section is measured at.
const LAYERS: usize = 240;
/// A minimum this narrow against its neighbours or narrower is suggested.
const NARROW: f64 = 0.6;
/// Minima closer than this share of the height are one: the narrowest
/// stays.
const APART: f64 = 0.06;
/// Suggestions given, best first.
const MOST: usize = 6;

/// The narrowings of `mesh` along `up` (unit), best first.
pub fn narrowings(mesh: &IndexedMesh, up: [f64; 3]) -> Vec<Narrowing> {
    let heights: Vec<f64> = mesh
        .vertices()
        .iter()
        .map(|v| dot(v.as_array().map(f64::from), up))
        .collect();
    let (lo, hi) = heights
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &h| {
            (a.min(h), b.max(h))
        });
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return Vec::new();
    }
    let step = (hi - lo) / LAYERS as f64;
    let mut area = vec![0.0; LAYERS];
    let vertices = mesh.vertices();
    for f in mesh.faces() {
        let idx = f.vertices.map(|v| v.get() as usize);
        let p = idx.map(|i| vertices[i].as_array().map(f64::from));
        let h = idx.map(|i| heights[i]);
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let (fmin, fmax) = (h[0].min(h[1]).min(h[2]), h[0].max(h[1]).max(h[2]));
        let first = (((fmin - lo) / step) - 0.5).ceil().max(0.0) as usize;
        let last = ((((fmax - lo) / step) - 0.5).floor() as isize).min(LAYERS as isize - 1);
        if last < first as isize {
            continue;
        }
        #[allow(clippy::needless_range_loop)]
        for k in first..=last as usize {
            let z = lo + (k as f64 + 0.5) * step;
            // Where the layer crosses the triangle's edges.
            let mut ends: Vec<[f64; 3]> = Vec::with_capacity(2);
            for e in 0..3 {
                let (a, b) = (e, (e + 1) % 3);
                let (ha, hb) = (h[a], h[b]);
                if (ha - z) * (hb - z) < 0.0 {
                    let t = (z - ha) / (hb - ha);
                    ends.push([0, 1, 2].map(|i| p[a][i] + t * (p[b][i] - p[a][i])));
                }
            }
            if ends.len() != 2 {
                continue;
            }
            // Oriented so the inside lies on the segment's left, seen from
            // above: then ½ (a × b)·up adds up to the enclosed area.
            let (mut a, mut b) = (ends[0], ends[1]);
            if dot(sub(b, a), cross(up, n)) < 0.0 {
                std::mem::swap(&mut a, &mut b);
            }
            area[k] += 0.5 * dot(cross(a, b), up);
        }
    }
    // A little smoothing: a narrowing is a feature of millimetres.
    let smooth: Vec<f64> = (0..LAYERS)
        .map(|k| {
            let lo_k = k.saturating_sub(1);
            let hi_k = (k + 1).min(LAYERS - 1);
            (area[lo_k] + 2.0 * area[k] + area[hi_k]) / 4.0
        })
        .map(|a| a.max(0.0))
        .collect();
    // The largest section below and above each layer.
    let mut below = vec![0.0; LAYERS];
    let mut above = vec![0.0; LAYERS];
    let mut m = 0.0f64;
    for k in 0..LAYERS {
        below[k] = m;
        m = m.max(smooth[k]);
    }
    m = 0.0;
    for k in (0..LAYERS).rev() {
        above[k] = m;
        m = m.max(smooth[k]);
    }
    let margin = (LAYERS as f64 * 0.04) as usize;
    let mut found: Vec<Narrowing> = Vec::new();
    for k in margin.max(1)..LAYERS - margin.max(1) {
        let a = smooth[k];
        if a <= 0.0 || a > smooth[k - 1] || a > smooth[k + 1] {
            continue;
        }
        let around = below[k].min(above[k]);
        if around <= 0.0 {
            continue;
        }
        let ratio = a / around;
        if ratio <= NARROW {
            found.push(Narrowing {
                at: lo + (k as f64 + 0.5) * step,
                area_mm2: a,
                ratio,
            });
        }
    }
    found.sort_by(|a, b| a.ratio.total_cmp(&b.ratio));
    let mut kept: Vec<Narrowing> = Vec::new();
    for n in found {
        if kept.iter().all(|k| (k.at - n.at).abs() > APART * (hi - lo)) {
            kept.push(n);
        }
        if kept.len() == MOST {
            break;
        }
    }
    kept
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
