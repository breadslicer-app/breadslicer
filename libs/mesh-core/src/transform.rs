/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::cmp::Ordering;

use anyhow::{Result, anyhow};

use crate::primitives::{Normal, Vector3D, Vertex};

/// 3×4 affine transform: a 3×3 linear part plus a 3-vec translation.
///
/// Stored as `f64` to keep precision under repeated composition (GUI
/// nudges, gizmo drags). Values are downcast to `f32` only when applying
/// the transform to mesh data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffineTransform3D {
    /// Row-major 3×3 linear part.
    pub linear: [[f64; 3]; 3],
    /// Translation applied after the linear part.
    pub translation: [f64; 3],
}

impl AffineTransform3D {
    pub const fn identity() -> Self {
        Self {
            linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            translation: [0.0, 0.0, 0.0],
        }
    }

    pub const fn from_translation(translation: [f64; 3]) -> Self {
        Self {
            linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            translation,
        }
    }

    pub const fn from_uniform_scale(s: f64) -> Self {
        Self {
            linear: [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]],
            translation: [0.0, 0.0, 0.0],
        }
    }

    pub const fn from_non_uniform_scale(s: [f64; 3]) -> Self {
        Self {
            linear: [[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]],
            translation: [0.0, 0.0, 0.0],
        }
    }

    /// Rotation about the X axis by `angle` radians, right-handed.
    pub fn from_rotation_x(angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self {
            linear: [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
            translation: [0.0, 0.0, 0.0],
        }
    }

    /// Rotation about the Y axis by `angle` radians, right-handed.
    pub fn from_rotation_y(angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self {
            linear: [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
            translation: [0.0, 0.0, 0.0],
        }
    }

    /// Rotation about the Z axis by `angle` radians, right-handed.
    pub fn from_rotation_z(angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self {
            linear: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
            translation: [0.0, 0.0, 0.0],
        }
    }

    /// Rodrigues rotation about an arbitrary axis by `angle` radians,
    /// right-handed. The axis is normalised internally; passing a zero
    /// (or near-zero) axis returns an error.
    pub fn from_axis_angle(axis: [f64; 3], angle: f64) -> Result<Self> {
        let norm_sq = axis[0].mul_add(axis[0], axis[1].mul_add(axis[1], axis[2] * axis[2]));
        // Threshold matches the f64 working precision of the rest of the crate;
        // a near-zero axis cannot be normalised without catastrophic loss.
        // `partial_cmp` form rejects NaN (which compares as `None`) too.
        if !matches!(norm_sq.partial_cmp(&f64::EPSILON), Some(Ordering::Greater)) {
            return Err(anyhow!("Rotation axis must be non-zero"));
        }
        let inv_norm = 1.0 / norm_sq.sqrt();
        let (x, y, z) = (axis[0] * inv_norm, axis[1] * inv_norm, axis[2] * inv_norm);

        let (s, c) = angle.sin_cos();
        let one_minus_c = 1.0 - c;

        // Rodrigues' rotation formula in matrix form.
        let xx = x * x;
        let yy = y * y;
        let zz = z * z;
        let xy = x * y;
        let xz = x * z;
        let yz = y * z;

        Ok(Self {
            linear: [
                [
                    xx.mul_add(one_minus_c, c),
                    xy.mul_add(one_minus_c, -z * s),
                    xz.mul_add(one_minus_c, y * s),
                ],
                [
                    xy.mul_add(one_minus_c, z * s),
                    yy.mul_add(one_minus_c, c),
                    yz.mul_add(one_minus_c, -x * s),
                ],
                [
                    xz.mul_add(one_minus_c, -y * s),
                    yz.mul_add(one_minus_c, x * s),
                    zz.mul_add(one_minus_c, c),
                ],
            ],
            translation: [0.0, 0.0, 0.0],
        })
    }

    /// Builds an affine transform from a raw linear part and translation.
    /// Intended for tests and import paths; prefer the dedicated
    /// constructors when applicable.
    pub const fn from_linear_and_translation(linear: [[f64; 3]; 3], translation: [f64; 3]) -> Self {
        Self {
            linear,
            translation,
        }
    }

    /// Applies the transform to a point: `M · v + t`.
    /// Math is performed in `f64`; the result is downcast to `f32` to fit
    /// the mesh's storage type.
    pub fn apply_to_vertex(&self, v: Vertex) -> Vertex {
        let v = [v.0[0] as f64, v.0[1] as f64, v.0[2] as f64];
        let m = &self.linear;
        let t = &self.translation;
        let x = m[0][0].mul_add(v[0], m[0][1].mul_add(v[1], m[0][2].mul_add(v[2], t[0])));
        let y = m[1][0].mul_add(v[0], m[1][1].mul_add(v[1], m[1][2].mul_add(v[2], t[1])));
        let z = m[2][0].mul_add(v[0], m[2][1].mul_add(v[1], m[2][2].mul_add(v[2], t[2])));
        Vertex::new([x as f32, y as f32, z as f32])
    }

    /// Applies the linear part of the transform to a direction or
    /// displacement, ignoring the translation. Use this for tangents and
    /// other vectors that should not be affected by translation. For
    /// surface normals see `apply_to_normal`, which uses the
    /// inverse-transpose.
    pub fn apply_to_vector(&self, v: Vector3D<f32>) -> Vector3D<f32> {
        let v = [v.0[0] as f64, v.0[1] as f64, v.0[2] as f64];
        let m = &self.linear;
        let x = m[0][0].mul_add(v[0], m[0][1].mul_add(v[1], m[0][2] * v[2]));
        let y = m[1][0].mul_add(v[0], m[1][1].mul_add(v[1], m[1][2] * v[2]));
        let z = m[2][0].mul_add(v[0], m[2][1].mul_add(v[1], m[2][2] * v[2]));
        Vector3D::new([x as f32, y as f32, z as f32])
    }

    /// Determinant of the 3×3 linear part. Module-internal for now; a
    /// public version is introduced alongside `inverse` and `compose`.
    pub(super) fn linear_determinant(&self) -> f64 {
        let m = &self.linear;
        // Cofactor expansion along the first row, written with `mul_add`
        // chains to keep precision consistent with the rest of the module.
        let c0 = m[1][1].mul_add(m[2][2], -m[1][2] * m[2][1]);
        let c1 = m[1][0].mul_add(m[2][2], -m[1][2] * m[2][0]);
        let c2 = m[1][0].mul_add(m[2][1], -m[1][1] * m[2][0]);
        m[0][0].mul_add(c0, m[0][1].mul_add(-c1, m[0][2] * c2))
    }

    /// Applies the transform to a unit surface normal.
    ///
    /// The mathematically correct way to transform a normal under a
    /// general affine map is to multiply by the **inverse-transpose** of
    /// the linear part and then re-normalise. For a pure rotation or
    /// uniform scale the inverse-transpose equals the linear part up to a
    /// positive scalar, so the renormalised result is identical; for
    /// non-uniform scale, shear, or mirror it is not, and using the
    /// linear part directly would yield a normal that is no longer
    /// perpendicular to the transformed surface.
    ///
    /// Returns an error if the linear part is singular (determinant zero
    /// to within `f64::EPSILON`) or if the resulting normal has zero
    /// magnitude (which can happen for pathological inputs).
    pub fn apply_to_normal(&self, n: Normal) -> Result<Normal> {
        let det = self.linear_determinant();
        if !matches!(
            det.abs().partial_cmp(&f64::EPSILON),
            Some(Ordering::Greater)
        ) {
            return Err(anyhow!("Cannot transform normal: singular linear part"));
        }

        // The (i, j) entry of M⁻ᵀ is the (j, i) cofactor of M divided by
        // det(M). The 1/|det| factor is removed by the final
        // renormalisation, but its sign is not — a mirror (det < 0)
        // must flip the normal, so the sign is reapplied explicitly
        // after the matrix multiply.
        let m = &self.linear;
        let cof = [
            [
                m[1][1].mul_add(m[2][2], -m[1][2] * m[2][1]),
                -m[1][0].mul_add(m[2][2], -m[1][2] * m[2][0]),
                m[1][0].mul_add(m[2][1], -m[1][1] * m[2][0]),
            ],
            [
                -m[0][1].mul_add(m[2][2], -m[0][2] * m[2][1]),
                m[0][0].mul_add(m[2][2], -m[0][2] * m[2][0]),
                -m[0][0].mul_add(m[2][1], -m[0][1] * m[2][0]),
            ],
            [
                m[0][1].mul_add(m[1][2], -m[0][2] * m[1][1]),
                -m[0][0].mul_add(m[1][2], -m[0][2] * m[1][0]),
                m[0][0].mul_add(m[1][1], -m[0][1] * m[1][0]),
            ],
        ];

        let n64 = [n.0[0] as f64, n.0[1] as f64, n.0[2] as f64];
        // (M⁻ᵀ)ᵢⱼ = cof(M)ᵢⱼ / det(M); (M⁻ᵀ · n)ᵢ = Σⱼ cof(M)ᵢⱼ · nⱼ / det.
        let mut out = [
            cof[0][0].mul_add(n64[0], cof[0][1].mul_add(n64[1], cof[0][2] * n64[2])),
            cof[1][0].mul_add(n64[0], cof[1][1].mul_add(n64[1], cof[1][2] * n64[2])),
            cof[2][0].mul_add(n64[0], cof[2][1].mul_add(n64[1], cof[2][2] * n64[2])),
        ];

        // Apply the sign of det so that a mirror (det < 0) genuinely
        // flips the normal. The magnitude is removed by renormalisation
        // below.
        if det < 0.0 {
            out = [-out[0], -out[1], -out[2]];
        }

        let mag_sq = out[0].mul_add(out[0], out[1].mul_add(out[1], out[2] * out[2]));
        if !matches!(mag_sq.partial_cmp(&f64::EPSILON), Some(Ordering::Greater)) {
            return Err(anyhow!("Cannot transform normal: zero-magnitude result"));
        }
        let inv_mag = 1.0 / mag_sq.sqrt();
        Ok(Normal::new([
            (out[0] * inv_mag) as f32,
            (out[1] * inv_mag) as f32,
            (out[2] * inv_mag) as f32,
        ]))
    }
}

impl Default for AffineTransform3D {
    fn default() -> Self {
        Self::identity()
    }
}

#[cfg(test)]
mod affine_transform_3d_tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    fn assert_linear_approx(actual: &[[f64; 3]; 3], expected: &[[f64; 3]; 3], eps: f64) {
        for (a_row, e_row) in actual.iter().zip(expected.iter()) {
            for (a, e) in a_row.iter().zip(e_row.iter()) {
                assert_approx_eq!(f64, *a, *e, epsilon = eps);
            }
        }
    }

    fn assert_vertex_approx(actual: Vertex, expected: [f32; 3], eps: f32) {
        for (a, e) in actual.0.iter().zip(expected.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
    }

    fn assert_vector_approx(actual: Vector3D<f32>, expected: [f32; 3], eps: f32) {
        for (a, e) in actual.0.iter().zip(expected.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
    }

    fn assert_normal_approx(actual: Normal, expected: [f32; 3], eps: f32) {
        for (a, e) in actual.0.iter().zip(expected.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
    }

    #[test]
    fn identity_has_unit_diagonal_and_zero_translation() {
        let t = AffineTransform3D::identity();
        assert_eq!(
            t.linear,
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        );
        assert_eq!(t.translation, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn default_equals_identity() {
        assert_eq!(AffineTransform3D::default(), AffineTransform3D::identity());
    }

    #[test]
    fn from_translation_keeps_identity_linear() {
        let t = AffineTransform3D::from_translation([1.5, -2.0, 7.25]);
        assert_eq!(
            t.linear,
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        );
        assert_eq!(t.translation, [1.5, -2.0, 7.25]);
    }

    #[test]
    fn from_translation_zero_equals_identity() {
        assert_eq!(
            AffineTransform3D::from_translation([0.0, 0.0, 0.0]),
            AffineTransform3D::identity()
        );
    }

    #[test]
    fn from_uniform_scale_has_zero_translation_and_diagonal_linear() {
        let t = AffineTransform3D::from_uniform_scale(3.0);
        assert_eq!(
            t.linear,
            [[3.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 3.0]]
        );
        assert_eq!(t.translation, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn from_uniform_scale_one_equals_identity() {
        assert_eq!(
            AffineTransform3D::from_uniform_scale(1.0),
            AffineTransform3D::identity()
        );
    }

    #[test]
    fn from_uniform_scale_negative_mirrors() {
        // A negative uniform scale is a valid affine transform that mirrors
        // every axis. We don't reject it here — callers (e.g. the future
        // determinant check) handle handedness.
        let t = AffineTransform3D::from_uniform_scale(-2.0);
        assert_eq!(
            t.linear,
            [[-2.0, 0.0, 0.0], [0.0, -2.0, 0.0], [0.0, 0.0, -2.0]]
        );
    }

    #[test]
    fn from_non_uniform_scale_has_diagonal_linear_and_zero_translation() {
        let t = AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0]);
        assert_eq!(
            t.linear,
            [[2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 4.0]]
        );
        assert_eq!(t.translation, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn from_non_uniform_scale_ones_equals_identity() {
        assert_eq!(
            AffineTransform3D::from_non_uniform_scale([1.0, 1.0, 1.0]),
            AffineTransform3D::identity()
        );
    }

    #[test]
    fn from_rotation_x_zero_equals_identity() {
        assert_linear_approx(
            &AffineTransform3D::from_rotation_x(0.0).linear,
            &AffineTransform3D::identity().linear,
            1e-15,
        );
    }

    #[test]
    fn from_rotation_x_quarter_turn_swaps_y_into_z() {
        // Right-handed: (0, 1, 0) goes to (0, 0, 1).
        let t = AffineTransform3D::from_rotation_x(FRAC_PI_2);
        assert_linear_approx(
            &t.linear,
            &[[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
            1e-15,
        );
    }

    #[test]
    fn from_rotation_y_quarter_turn_swaps_z_into_x() {
        // Right-handed: (0, 0, 1) goes to (1, 0, 0).
        let t = AffineTransform3D::from_rotation_y(FRAC_PI_2);
        assert_linear_approx(
            &t.linear,
            &[[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
            1e-15,
        );
    }

    #[test]
    fn from_rotation_z_quarter_turn_swaps_x_into_y() {
        // Right-handed: (1, 0, 0) goes to (0, 1, 0).
        let t = AffineTransform3D::from_rotation_z(FRAC_PI_2);
        assert_linear_approx(
            &t.linear,
            &[[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            1e-15,
        );
    }

    #[test]
    fn from_rotation_axis_aligned_matches_dedicated_builders() {
        let r_x = AffineTransform3D::from_axis_angle([1.0, 0.0, 0.0], FRAC_PI_4).unwrap();
        let r_y = AffineTransform3D::from_axis_angle([0.0, 1.0, 0.0], FRAC_PI_4).unwrap();
        let r_z = AffineTransform3D::from_axis_angle([0.0, 0.0, 1.0], FRAC_PI_4).unwrap();
        assert_linear_approx(
            &r_x.linear,
            &AffineTransform3D::from_rotation_x(FRAC_PI_4).linear,
            1e-15,
        );
        assert_linear_approx(
            &r_y.linear,
            &AffineTransform3D::from_rotation_y(FRAC_PI_4).linear,
            1e-15,
        );
        assert_linear_approx(
            &r_z.linear,
            &AffineTransform3D::from_rotation_z(FRAC_PI_4).linear,
            1e-15,
        );
    }

    #[test]
    fn from_axis_angle_normalises_input_axis() {
        // A non-unit axis should produce the same matrix as the unit axis.
        let unit = AffineTransform3D::from_axis_angle([0.0, 0.0, 1.0], FRAC_PI_4).unwrap();
        let scaled = AffineTransform3D::from_axis_angle([0.0, 0.0, 5.0], FRAC_PI_4).unwrap();
        assert_linear_approx(&unit.linear, &scaled.linear, 1e-15);
    }

    #[test]
    fn from_axis_angle_pi_around_diagonal_swaps_axes() {
        // Rotation by π about the (1,1,1)/√3 axis cycles (x,y,z) ↦ in a known
        // pattern. Specifically, the matrix is (2/3)·J − I where J is the
        // all-ones matrix, giving diagonal entries -1/3 and off-diagonal 2/3.
        let t = AffineTransform3D::from_axis_angle([1.0, 1.0, 1.0], PI).unwrap();
        let third = 1.0 / 3.0;
        assert_linear_approx(
            &t.linear,
            &[
                [-third, 2.0 * third, 2.0 * third],
                [2.0 * third, -third, 2.0 * third],
                [2.0 * third, 2.0 * third, -third],
            ],
            1e-15,
        );
    }

    #[test]
    fn from_axis_angle_zero_axis_errors() {
        let result = AffineTransform3D::from_axis_angle([0.0, 0.0, 0.0], FRAC_PI_4);
        assert!(result.is_err());
    }

    #[test]
    fn from_linear_and_translation_stores_inputs_verbatim() {
        let linear = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        let translation = [10.0, 11.0, 12.0];
        let t = AffineTransform3D::from_linear_and_translation(linear, translation);
        assert_eq!(t.linear, linear);
        assert_eq!(t.translation, translation);
    }

    #[test]
    fn apply_to_vertex_identity_is_pass_through() {
        let t = AffineTransform3D::identity();
        let v = Vertex::new([1.5, -2.0, 7.25]);
        assert_eq!(t.apply_to_vertex(v), v);
    }

    #[test]
    fn apply_to_vertex_translation_adds_offset() {
        let t = AffineTransform3D::from_translation([10.0, -5.0, 2.5]);
        let v = Vertex::new([1.0, 2.0, 3.0]);
        assert_vertex_approx(t.apply_to_vertex(v), [11.0, -3.0, 5.5], 1e-6);
    }

    #[test]
    fn apply_to_vertex_uniform_scale_scales_each_coordinate() {
        let t = AffineTransform3D::from_uniform_scale(2.5);
        let v = Vertex::new([1.0, -2.0, 3.0]);
        assert_vertex_approx(t.apply_to_vertex(v), [2.5, -5.0, 7.5], 1e-6);
    }

    #[test]
    fn apply_to_vertex_non_uniform_scale_scales_each_axis_independently() {
        let t = AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0]);
        let v = Vertex::new([1.0, 1.0, 1.0]);
        assert_vertex_approx(t.apply_to_vertex(v), [2.0, 3.0, 4.0], 1e-6);
    }

    #[test]
    fn apply_to_vertex_rotation_z_quarter_turn_sends_x_to_y() {
        let t = AffineTransform3D::from_rotation_z(FRAC_PI_2);
        let v = Vertex::new([1.0, 0.0, 0.0]);
        assert_vertex_approx(t.apply_to_vertex(v), [0.0, 1.0, 0.0], 1e-6);
    }

    #[test]
    fn apply_to_vertex_rotation_x_quarter_turn_sends_y_to_z() {
        let t = AffineTransform3D::from_rotation_x(FRAC_PI_2);
        let v = Vertex::new([0.0, 1.0, 0.0]);
        assert_vertex_approx(t.apply_to_vertex(v), [0.0, 0.0, 1.0], 1e-6);
    }

    #[test]
    fn apply_to_vertex_rotation_y_quarter_turn_sends_z_to_x() {
        let t = AffineTransform3D::from_rotation_y(FRAC_PI_2);
        let v = Vertex::new([0.0, 0.0, 1.0]);
        assert_vertex_approx(t.apply_to_vertex(v), [1.0, 0.0, 0.0], 1e-6);
    }

    #[test]
    fn apply_to_vertex_translation_then_rotation_uses_correct_order() {
        // The transform is applied as `M·v + t`, so the linear part acts on
        // `v` first and the translation is added after.
        let t = AffineTransform3D::from_linear_and_translation(
            // Rotation by π/2 about Z.
            [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [10.0, 20.0, 30.0],
        );
        let v = Vertex::new([1.0, 0.0, 0.0]);
        // M·v = (0, 1, 0); + t = (10, 21, 30).
        assert_vertex_approx(t.apply_to_vertex(v), [10.0, 21.0, 30.0], 1e-6);
    }

    #[test]
    fn apply_to_vertex_far_from_origin_keeps_precision() {
        // A short edge anchored far from the origin: in pure-f32 arithmetic
        // the rotated direction would lose several bits of precision, but
        // f64 internals preserve it.
        let base = 1.0e6_f32;
        let t = AffineTransform3D::from_rotation_z(FRAC_PI_2);
        let near = Vertex::new([base, 0.0, 0.0]);
        let far = Vertex::new([base + 1.0, 0.0, 0.0]);
        let near_rot = t.apply_to_vertex(near);
        let far_rot = t.apply_to_vertex(far);
        // The image should still be a unit-length displacement along +Y.
        assert_approx_eq!(f32, far_rot.0[0] - near_rot.0[0], 0.0, epsilon = 1e-3);
        assert_approx_eq!(f32, far_rot.0[1] - near_rot.0[1], 1.0, epsilon = 1e-3);
        assert_approx_eq!(f32, far_rot.0[2] - near_rot.0[2], 0.0, epsilon = 1e-3);
    }

    #[test]
    fn apply_to_vector_ignores_translation() {
        let t = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [100.0, -50.0, 7.0],
        );
        let v = Vector3D::<f32>::new([1.0, 2.0, 3.0]);
        // Linear part is identity; the result must be unchanged despite the
        // huge translation.
        assert_vector_approx(t.apply_to_vector(v), [1.0, 2.0, 3.0], 1e-6);
    }

    #[test]
    fn apply_to_vector_uses_linear_part() {
        let t = AffineTransform3D::from_linear_and_translation(
            // Rotation by π/2 about Z.
            [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [999.0, 999.0, 999.0],
        );
        let v = Vector3D::<f32>::new([1.0, 0.0, 0.0]);
        assert_vector_approx(t.apply_to_vector(v), [0.0, 1.0, 0.0], 1e-6);
    }

    #[test]
    fn apply_to_vector_scales_displacement() {
        let t = AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0]);
        let v = Vector3D::<f32>::new([1.0, 1.0, 1.0]);
        assert_vector_approx(t.apply_to_vector(v), [2.0, 3.0, 4.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_identity_is_pass_through() {
        let t = AffineTransform3D::identity();
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_normal_approx(t.apply_to_normal(n).unwrap(), [0.0, 0.0, 1.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_translation_does_not_change_normal() {
        let t = AffineTransform3D::from_translation([10.0, -5.0, 2.5]);
        let n = Normal::new([0.0, 1.0, 0.0]);
        assert_normal_approx(t.apply_to_normal(n).unwrap(), [0.0, 1.0, 0.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_rotation_rotates_the_normal() {
        // A π/2 rotation about Z sends +X to +Y; a normal pointing along
        // +X should be rotated identically.
        let t = AffineTransform3D::from_rotation_z(FRAC_PI_2);
        let n = Normal::new([1.0, 0.0, 0.0]);
        assert_normal_approx(t.apply_to_normal(n).unwrap(), [0.0, 1.0, 0.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_uniform_scale_does_not_change_normal() {
        // Uniform scale: M⁻ᵀ = (1/s)·I, so the normal direction is
        // unchanged after renormalisation.
        let t = AffineTransform3D::from_uniform_scale(7.5);
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_normal_approx(t.apply_to_normal(n).unwrap(), [0.0, 0.0, 1.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_non_uniform_scale_uses_inverse_transpose() {
        // Surface in the XY plane (normal = +Z), tilted by stretching
        // along X. With non-uniform scale s = (s_x, 1, 1), naively
        // applying the linear part would give (s_x, 0, 0)·n = (0, 0, 1)
        // (correct here because the normal has no X component) — so a
        // more meaningful test uses a tilted plane.
        //
        // Consider the surface containing (1,0,0), (0,1,0), (0,0,0).
        // Its (unit) normal is (1, 1, 0)/√2. Apply non-uniform scale
        // s = (2, 1, 1). The transformed surface contains (2,0,0),
        // (0,1,0), (0,0,0); its true normal is (1, 2, 0)/√5.
        //
        // Using the linear part naively would give
        // (2,1,0)/√5 ≠ (1,2,0)/√5 — the inverse-transpose is required.
        let t = AffineTransform3D::from_non_uniform_scale([2.0, 1.0, 1.0]);
        let n = Normal::new([
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        ]);
        let result = t.apply_to_normal(n).unwrap();
        let inv_sqrt5 = 1.0 / 5.0_f32.sqrt();
        assert_normal_approx(result, [inv_sqrt5, 2.0 * inv_sqrt5, 0.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_result_is_unit_length() {
        // For any non-singular transform, the result must be unit-length.
        let t = AffineTransform3D::from_non_uniform_scale([3.0, 0.5, 7.0]);
        let n = Normal::new([
            1.0 / 3.0_f32.sqrt(),
            1.0 / 3.0_f32.sqrt(),
            1.0 / 3.0_f32.sqrt(),
        ]);
        let result = t.apply_to_normal(n).unwrap();
        let mag = result.0.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert_approx_eq!(f32, mag, 1.0, epsilon = 1e-6);
    }

    #[test]
    fn apply_to_normal_mirror_along_z_flips_the_normal() {
        // A reflection across the XY plane has linear = diag(1, 1, -1)
        // and det = -1. A surface whose original normal was +Z faces -Z
        // after the reflection. `apply_to_normal` returns the normal
        // direction that matches the *winding-reversed* triangle (which
        // is what `merge_meshes` produces for negative-det transforms),
        // so the result here is -Z.
        let t = AffineTransform3D::from_non_uniform_scale([1.0, 1.0, -1.0]);
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_normal_approx(t.apply_to_normal(n).unwrap(), [0.0, 0.0, -1.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_mirror_along_x_does_not_flip_z_normal() {
        // A reflection across the YZ plane (linear = diag(-1, 1, 1)) is
        // also a negative-determinant transform, but a normal along +Z
        // remains pointing along +Z because the mirrored +Z face still
        // faces +Z. The internal sign-flip exactly compensates for the
        // cofactor's sign change, leaving the +Z direction untouched.
        let t = AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]);
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_normal_approx(t.apply_to_normal(n).unwrap(), [0.0, 0.0, 1.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_singular_linear_returns_error() {
        // A rank-2 linear part: column 2 is zero.
        let t = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert!(t.apply_to_normal(n).is_err());
    }

    #[test]
    fn apply_to_normal_zero_input_returns_error() {
        // A zero-length normal cannot be normalised after multiplication.
        let t = AffineTransform3D::from_uniform_scale(2.0);
        let n = Normal::new([0.0, 0.0, 0.0]);
        assert!(t.apply_to_normal(n).is_err());
    }

    #[test]
    fn apply_to_normal_rotation_then_translation_ignores_translation() {
        let t = AffineTransform3D::from_linear_and_translation(
            [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [1000.0, -500.0, 7.0],
        );
        let n = Normal::new([1.0, 0.0, 0.0]);
        assert_normal_approx(t.apply_to_normal(n).unwrap(), [0.0, 1.0, 0.0], 1e-6);
    }

    #[test]
    fn struct_is_copy() {
        // Compile-time check: assigning a value-typed transform should not
        // require an explicit `.clone()`.
        let a = AffineTransform3D::from_translation([1.0, 2.0, 3.0]);
        let b = a;
        assert_eq!(a, b);
    }
}
