/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use core::f32;
use std::cmp::Ordering;

use anyhow::{Result, anyhow};

use crate::primitives::{Normal, Vertex};

pub fn compute_normal_from_triangle_vertices(vertices: &[Vertex; 3]) -> Result<Normal> {
    let v0 = vertices[0].0;
    let v1 = vertices[1].0;
    let v2 = vertices[2].0;

    let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
    let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

    // Right-hand rule: (v1 - v0) × (v2 - v0)
    let cross = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];

    let cross_mag_sq = cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2];

    // degenerate (zero-area) triangles; also handles NaN coordinates.
    if match cross_mag_sq.partial_cmp(&f32::EPSILON) {
        Some(Ordering::Greater) => false, // We don't enter into the block
        _ => true,
    } {
        return Err(anyhow!("Degenerate (near-zero area) triangle found"));
    }

    // Normalize computed normal vecctor
    let cross_mag = cross_mag_sq.sqrt();
    let cross = [
        cross[0] / cross_mag,
        cross[1] / cross_mag,
        cross[2] / cross_mag,
    ];

    Ok(Normal::new(cross))
}
