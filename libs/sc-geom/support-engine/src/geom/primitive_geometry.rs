/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Indexed cylinders (or cones) and spheres along Y, centred on the origin,
//! facing out. Each ring repeats its first vertex at its end, so a ring of
//! `n` segments has `n + 1` vertices.

use std::f64::consts::{PI, TAU};

use super::buffer_geometry::BufferGeometry;

fn geometry(vertices: &[f64], indices: Vec<u32>) -> BufferGeometry {
    let mut g = BufferGeometry::default();
    g.set_position(vertices);
    g.index = Some(indices);
    g
}

/// A cylinder from `y = height / 2` (radius `radius_top`) down to
/// `y = -height / 2` (radius `radius_bottom`), in `radial_segments` around and
/// `height_segments` up; its ends capped unless `open_ended`. A radius of 0
/// closes that end in a point instead of a cap.
pub fn cylinder_geometry(
    radius_top: f64,
    radius_bottom: f64,
    height: f64,
    radial_segments: f64,
    height_segments: f64,
    open_ended: bool,
) -> BufferGeometry {
    let around = radial_segments.floor() as usize;
    let rows = height_segments.floor() as usize;
    let half = height / 2.0;
    let mut vertices: Vec<f64> = Vec::new();
    let ring_at = |vertices: &mut Vec<f64>, radius: f64, y: f64| {
        for x in 0..=around {
            let theta = TAU * x as f64 / around as f64;
            vertices.extend([radius * theta.sin(), y, radius * theta.cos()]);
        }
    };
    for row in 0..=rows {
        let v = row as f64 / rows as f64;
        ring_at(
            &mut vertices,
            radius_top + v * (radius_bottom - radius_top),
            half - v * height,
        );
    }
    let at = |row: usize, x: usize| (row * (around + 1) + x) as u32;
    let mut indices = Vec::new();
    for x in 0..around {
        for row in 0..rows {
            let (a, b, c, d) = (
                at(row, x),
                at(row + 1, x),
                at(row + 1, x + 1),
                at(row, x + 1),
            );
            // A pointed end would make one of the two triangles a sliver.
            if radius_top > 0.0 || row != 0 {
                indices.extend([a, b, d]);
            }
            if radius_bottom > 0.0 || row != rows - 1 {
                indices.extend([b, c, d]);
            }
        }
    }
    if !open_ended {
        for (top, radius) in [(true, radius_top), (false, radius_bottom)] {
            if radius <= 0.0 {
                continue;
            }
            let y = if top { half } else { -half };
            let centres = (vertices.len() / 3) as u32;
            for _ in 0..around {
                vertices.extend([0.0, y, 0.0]);
            }
            let rim = (vertices.len() / 3) as u32;
            ring_at(&mut vertices, radius, y);
            for x in 0..around as u32 {
                let (c, i) = (centres + x, rim + x);
                if top {
                    indices.extend([i, i + 1, c]);
                } else {
                    indices.extend([i + 1, i, c]);
                }
            }
        }
    }
    geometry(&vertices, indices)
}

/// A sphere of `radius` in `width_segments` around (at least 3) and
/// `height_segments` from pole to pole (at least 2).
pub fn sphere_geometry(radius: f64, width_segments: f64, height_segments: f64) -> BufferGeometry {
    let around = width_segments.floor().max(3.0) as usize;
    let rows = height_segments.floor().max(2.0) as usize;
    let mut vertices: Vec<f64> = Vec::new();
    for row in 0..=rows {
        let theta = PI * row as f64 / rows as f64;
        for x in 0..=around {
            let phi = TAU * x as f64 / around as f64;
            vertices.extend([
                -radius * phi.cos() * theta.sin(),
                radius * theta.cos(),
                radius * phi.sin() * theta.sin(),
            ]);
        }
    }
    let at = |row: usize, x: usize| (row * (around + 1) + x) as u32;
    let mut indices = Vec::new();
    for row in 0..rows {
        for x in 0..around {
            let (a, b, c, d) = (
                at(row, x + 1),
                at(row, x),
                at(row + 1, x),
                at(row + 1, x + 1),
            );
            // The poles: one triangle per quad, the other would be a sliver.
            if row != 0 {
                indices.extend([a, b, d]);
            }
            if row != rows - 1 {
                indices.extend([b, c, d]);
            }
        }
    }
    geometry(&vertices, indices)
}
