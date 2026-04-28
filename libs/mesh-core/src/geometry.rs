/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use core::f32;
use std::cmp::Ordering;

use anyhow::{Result, anyhow};

use crate::primitives::{Normal, Vertex};

/// Squared-area threshold below which a triangle is treated as degenerate.
///
/// Compared against `‖e1 × e2‖²` in f64 — twice the (squared) triangle
/// area. The numeric value matches the original `f32::EPSILON as f64`
/// guard so behaviour is preserved across the rename.
pub(crate) const DEGENERATE_AREA_SQUARED: f64 = f32::EPSILON as f64;

/// Tolerance for "is this f64 magnitude squared close enough to 1?" in
/// the unit-norm check on stored f32 normals widened to f64.
///
/// f32 unit normals widened to f64 can have squared magnitude deviating
/// from 1.0 by up to ~3·2⁻²³ ≈ 3.6e-7 due to f32 quantisation of each
/// component. 1e-6 leaves headroom above that.
pub(crate) const UNIT_NORM_TOLERANCE: f64 = 1e-6;

/// Cosine of the maximum tolerated angle (5°) between a stored normal
/// and the normal recomputed from the triangle's vertices.
///
/// Per-component epsilon comparisons are too sensitive: small triangles
/// and high-aspect-ratio triangles accumulate enough floating-point
/// error that their normals diverge by a few degrees from the stored
/// value. 5° filters numerical noise while still catching meaningfully
/// wrong normals. Stored as the precomputed cosine because the check is
/// against the dot product of two unit vectors.
pub(crate) const NORMAL_ANGLE_TOLERANCE_COS: f64 = 0.9961946980917455;

/// Threshold used to reject near-singular linear parts of an
/// `AffineTransform3D` (zero-magnitude rotation axis, near-zero
/// determinant, or zero-magnitude transformed normal).
///
/// `f64::EPSILON` is relative to 1.0 and may be too tight for
/// millimetre-scale matrices whose legitimate determinants can sit
/// around 1e-9. TODO: revisit and consider scaling by the matrix entries.
pub(crate) const SINGULARITY_THRESHOLD: f64 = f64::EPSILON;

/// Strict ordered comparison that returns `false` for NaN inputs.
///
/// Equivalent to `value > threshold` for ordered finite inputs, with one
/// crucial difference: when either operand is NaN, `partial_cmp` returns
/// `None` and this function returns `false`. Used as a safety guard in
/// degeneracy / singularity checks where NaN must be rejected (treated as
/// "not strictly greater") rather than propagated through `>`'s
/// always-false NaN behaviour, which is the same value but makes the
/// intent explicit at every call site.
#[inline]
pub(crate) fn gt_strict(value: f64, threshold: f64) -> bool {
    matches!(value.partial_cmp(&threshold), Some(Ordering::Greater))
}

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
    let v0 = vertices[0].to_f64();
    let v1 = vertices[1].to_f64();
    let v2 = vertices[2].to_f64();

    let e1 = v1.diff(&v0);
    let e2 = v2.diff(&v0);

    let cross = e1.cross_product(&e2);

    let cross_mag_sq = cross.squared_norm();

    // degenerate (zero-area) triangles; also handles NaN coordinates.
    if !gt_strict(cross_mag_sq, DEGENERATE_AREA_SQUARED) {
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
        let n = result.coords();
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
        let n = normal.coords();
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
        let n = normal.coords();
        let expected = 1.0 / 3.0_f32.sqrt();
        assert_normal_approx(&normal, [expected, expected, expected]);
        let mag = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((mag - 1.0).abs() < 1e-5);
    }

    #[test]
    fn gt_strict_strictly_greater_finite() {
        assert!(gt_strict(2.0, 1.0));
        assert!(gt_strict(1.0e-10, 0.0));
        assert!(gt_strict(f64::EPSILON * 2.0, f64::EPSILON));
    }

    #[test]
    fn gt_strict_equal_returns_false() {
        assert!(!gt_strict(1.0, 1.0));
        assert!(!gt_strict(0.0, 0.0));
        assert!(!gt_strict(f64::EPSILON, f64::EPSILON));
        assert!(!gt_strict(-0.0, 0.0));
    }

    #[test]
    fn gt_strict_strictly_less_finite() {
        assert!(!gt_strict(1.0, 2.0));
        assert!(!gt_strict(-1.0, 0.0));
        assert!(!gt_strict(0.0, f64::EPSILON));
    }

    #[test]
    fn gt_strict_nan_returns_false() {
        assert!(!gt_strict(f64::NAN, 0.0));
        assert!(!gt_strict(0.0, f64::NAN));
        assert!(!gt_strict(f64::NAN, f64::NAN));
        assert!(!gt_strict(f64::NAN, f64::INFINITY));
        assert!(!gt_strict(f64::INFINITY, f64::NAN));
    }

    #[test]
    fn gt_strict_infinities() {
        assert!(gt_strict(f64::INFINITY, 0.0));
        assert!(gt_strict(f64::INFINITY, f64::MAX));
        assert!(!gt_strict(f64::INFINITY, f64::INFINITY));
        assert!(!gt_strict(f64::NEG_INFINITY, 0.0));
        assert!(!gt_strict(f64::NEG_INFINITY, f64::NEG_INFINITY));
        assert!(gt_strict(0.0, f64::NEG_INFINITY));
    }

    #[test]
    fn gt_strict_subnormals() {
        let subnormal = f64::MIN_POSITIVE / 2.0;
        assert!(subnormal > 0.0 && subnormal < f64::MIN_POSITIVE);
        assert!(gt_strict(subnormal, 0.0));
        assert!(!gt_strict(0.0, subnormal));
        assert!(!gt_strict(subnormal, subnormal));
    }
}
