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

#[cfg(test)]
mod geometry_tests {
    use super::*;
    use crate::primitives::Vertex;

    fn v(x: f32, y: f32, z: f32) -> Vertex {
        Vertex::new([x, y, z])
    }

    fn assert_normal_approx(result: &Normal, expected: [f32; 3]) {
        let n = result.0;
        for i in 0..3 {
            assert!(
                (n[i] - expected[i]).abs() < 1e-5,
                "component {i}: got {}, expected {}",
                n[i],
                expected[i],
            );
        }
    }

    #[test]
    fn xy_plane_triangle_gives_z_normal() {
        let verts = [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)];
        let normal = compute_normal_from_triangle_vertices(&verts).unwrap();
        assert_normal_approx(&normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn xz_plane_triangle_gives_negative_y_normal() {
        let verts = [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0)];
        let normal = compute_normal_from_triangle_vertices(&verts).unwrap();
        assert_normal_approx(&normal, [0.0, -1.0, 0.0]);
    }

    #[test]
    fn yz_plane_triangle_gives_x_normal() {
        let verts = [v(0.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 1.0)];
        let normal = compute_normal_from_triangle_vertices(&verts).unwrap();
        assert_normal_approx(&normal, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn reversed_winding_flips_normal() {
        let verts = [v(0.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(1.0, 0.0, 0.0)];
        let normal = compute_normal_from_triangle_vertices(&verts).unwrap();
        assert_normal_approx(&normal, [0.0, 0.0, -1.0]);
    }

    #[test]
    fn translated_triangle_same_normal() {
        let verts = [v(5.0, 5.0, 5.0), v(6.0, 5.0, 5.0), v(5.0, 6.0, 5.0)];
        let normal = compute_normal_from_triangle_vertices(&verts).unwrap();
        assert_normal_approx(&normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn result_is_unit_length() {
        let verts = [v(0.0, 0.0, 0.0), v(3.0, 0.0, 0.0), v(0.0, 4.0, 0.0)];
        let normal = compute_normal_from_triangle_vertices(&verts).unwrap();
        let n = normal.0;
        let mag = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((mag - 1.0).abs() < 1e-5, "magnitude was {mag}");
    }

    #[test]
    fn degenerate_collinear_points_returns_error() {
        let verts = [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(2.0, 0.0, 0.0)];
        assert!(compute_normal_from_triangle_vertices(&verts).is_err());
    }

    #[test]
    fn degenerate_duplicate_vertices_returns_error() {
        let verts = [v(1.0, 2.0, 3.0), v(1.0, 2.0, 3.0), v(1.0, 2.0, 3.0)];
        assert!(compute_normal_from_triangle_vertices(&verts).is_err());
    }

    #[test]
    fn degenerate_two_identical_vertices_returns_error() {
        let verts = [v(0.0, 0.0, 0.0), v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0)];
        assert!(compute_normal_from_triangle_vertices(&verts).is_err());
    }

    #[test]
    fn nan_coordinate_returns_error() {
        let verts = [v(f32::NAN, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)];
        assert!(compute_normal_from_triangle_vertices(&verts).is_err());
    }

    #[test]
    fn diagonal_triangle_normal_is_normalized() {
        let verts = [v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 1.0)];
        let normal = compute_normal_from_triangle_vertices(&verts).unwrap();
        let n = normal.0;
        let expected = 1.0 / 3.0_f32.sqrt();
        assert_normal_approx(&normal, [expected, expected, expected]);
        let mag = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((mag - 1.0).abs() < 1e-5);
    }
}
