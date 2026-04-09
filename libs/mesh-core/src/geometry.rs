/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use core::f32;
use std::cmp::Ordering;

use anyhow::{Result, anyhow};

use crate::primitives::{Normal, Vector3D, Vertex};

/// Computes a unit normal for a triangle using f64 precision internally.
///
/// The f32 vertex coordinates are promoted to f64 before subtracting, taking
/// the cross product, and normalizing. This mitigates the catastrophic
/// cancellation that happens when vertices are far from the origin but the
/// triangle edges are relatively short: in f32, the difference `v_i - v_0`
/// can lose several bits of precision, and the resulting normal can differ
/// from the "true" one by as much as 1e-3 or so.
///
/// The result is returned as an f64 array so callers that want to compare it
/// against a stored f32 normal can do so at f64 precision.
pub(crate) fn compute_reference_normal_f64(vertices: &[Vertex; 3]) -> Result<[f64; 3]> {
    let v0 = Vector3D::<f64>::from(vertices[0]);
    let v1 = Vector3D::<f64>::from(vertices[1]);
    let v2 = Vector3D::<f64>::from(vertices[2]);

    let e1 = v1.diff(&v0);
    let e2 = v2.diff(&v0);

    let cross = e1.cross_product(&e2);

    let cross_mag_sq = cross.squared_norm();

    // degenerate (zero-area) triangles; also handles NaN coordinates.
    if match cross_mag_sq.partial_cmp(&(f32::EPSILON as f64)) {
        Some(Ordering::Greater) => false, // We don't enter into the block
        _ => true,
    } {
        return Err(anyhow!("Degenerate (near-zero area) triangle found"));
    }

    let cross_mag = cross_mag_sq.sqrt();
    Ok([
        cross[0] / cross_mag,
        cross[1] / cross_mag,
        cross[2] / cross_mag,
    ])
}

pub fn compute_normal_from_triangle_vertices(vertices: &[Vertex; 3]) -> Result<Normal> {
    let unit = compute_reference_normal_f64(vertices)?;
    Ok(Normal::new([
        unit[0] as f32,
        unit[1] as f32,
        unit[2] as f32,
    ]))
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
