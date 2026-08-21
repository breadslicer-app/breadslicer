/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fmt;

use crate::geometry::{SINGULARITY_THRESHOLD, gt_strict};
use crate::primitives::{Normal, Vector3D, Vertex};

/// Failure modes of `AffineTransform3D` / `Transform3D` operations that
/// can encounter degenerate inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformError {
    /// The linear part has a determinant whose magnitude is below the
    /// singularity threshold, so it has no inverse and no
    /// inverse-transpose.
    SingularLinear,
    /// A rotation axis was given with magnitude at or below the
    /// singularity threshold, so it cannot be normalised.
    ZeroAxis,
    /// Applying the inverse-transpose to a unit normal produced a
    /// zero-magnitude vector, so no direction can be recovered.
    ZeroMagnitudeNormal,
}

impl fmt::Display for TransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SingularLinear => "linear part of the transform is singular",
            Self::ZeroAxis => "rotation axis must be non-zero",
            Self::ZeroMagnitudeNormal => "transformed normal has zero magnitude",
        })
    }
}

impl std::error::Error for TransformError {}

/// 3×4 affine transform: a 3×3 linear part plus a 3-vec translation.
///
/// Stored as `f64` to keep precision under repeated composition (GUI
/// nudges, gizmo drags). Values are downcast to `f32` only when applying
/// the transform to mesh data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffineTransform3D {
    /// Row-major 3×3 linear part.
    pub(crate) linear: [[f64; 3]; 3],
    /// Translation applied after the linear part.
    pub(crate) translation: [f64; 3],
}

impl AffineTransform3D {
    /// Row-major 3×3 linear part.
    pub fn linear(&self) -> &[[f64; 3]; 3] {
        &self.linear
    }

    /// Translation applied after the linear part.
    pub fn translation(&self) -> &[f64; 3] {
        &self.translation
    }

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
    /// (or near-zero) axis returns [`TransformError::ZeroAxis`].
    pub fn from_axis_angle(axis: [f64; 3], angle: f64) -> Result<Self, TransformError> {
        let norm_sq = axis[0].mul_add(axis[0], axis[1].mul_add(axis[1], axis[2] * axis[2]));
        if !gt_strict(norm_sq, SINGULARITY_THRESHOLD) {
            return Err(TransformError::ZeroAxis);
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
        let v = v.coords();
        let v = [v[0] as f64, v[1] as f64, v[2] as f64];
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
        let v: [f32; 3] = v.into();
        let v = [v[0] as f64, v[1] as f64, v[2] as f64];
        let m = &self.linear;
        let x = m[0][0].mul_add(v[0], m[0][1].mul_add(v[1], m[0][2] * v[2]));
        let y = m[1][0].mul_add(v[0], m[1][1].mul_add(v[1], m[1][2] * v[2]));
        let z = m[2][0].mul_add(v[0], m[2][1].mul_add(v[1], m[2][2] * v[2]));
        Vector3D::new([x as f32, y as f32, z as f32])
    }

    /// Determinant of the 3×3 linear part.
    ///
    /// The translation does not affect the determinant. A negative
    /// determinant indicates the transform mirrors space (reverses
    /// orientation); zero indicates a singular transform that cannot be
    /// inverted.
    pub fn determinant(&self) -> f64 {
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
    /// Returns [`TransformError::SingularLinear`] if the linear part is
    /// singular (determinant zero to within [`SINGULARITY_THRESHOLD`]),
    /// or [`TransformError::ZeroMagnitudeNormal`] if the resulting
    /// normal has zero magnitude (which can happen for pathological
    /// inputs).
    pub fn apply_to_normal(&self, n: Normal) -> Result<Normal, TransformError> {
        let det = self.determinant();
        if !gt_strict(det.abs(), SINGULARITY_THRESHOLD) {
            return Err(TransformError::SingularLinear);
        }

        // The (i, j) entry of M⁻ᵀ is the (j, i) cofactor of M divided by
        // det(M). The 1/|det| factor is removed by the final
        // renormalisation, but its sign is not; a mirror (det < 0)
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

        let nc = n.coords();
        let n64 = [nc[0] as f64, nc[1] as f64, nc[2] as f64];
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
        if !gt_strict(mag_sq, SINGULARITY_THRESHOLD) {
            return Err(TransformError::ZeroMagnitudeNormal);
        }
        let inv_mag = 1.0 / mag_sq.sqrt();
        Ok(Normal::new([
            (out[0] * inv_mag) as f32,
            (out[1] * inv_mag) as f32,
            (out[2] * inv_mag) as f32,
        ]))
    }

    /// Composes two transforms: `self.compose(other)` is the transform
    /// that applies `other` first and then `self`. That is,
    /// `self.compose(other).apply_to_vertex(v)` equals
    /// `self.apply_to_vertex(other.apply_to_vertex(v))`.
    pub fn compose(&self, other: &Self) -> Self {
        let a = &self.linear;
        let b = &other.linear;

        // Linear part: A · B.
        let mut linear = [[0.0_f64; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                linear[i][j] =
                    a[i][0].mul_add(b[0][j], a[i][1].mul_add(b[1][j], a[i][2] * b[2][j]));
            }
        }

        // Translation: A · t_b + t_a (because (A·B)·v + A·t_b + t_a =
        // A·(B·v + t_b) + t_a, i.e. apply `other` first, then `self`).
        let tb = &other.translation;
        let ta = &self.translation;
        let translation = [
            a[0][0].mul_add(tb[0], a[0][1].mul_add(tb[1], a[0][2].mul_add(tb[2], ta[0]))),
            a[1][0].mul_add(tb[0], a[1][1].mul_add(tb[1], a[1][2].mul_add(tb[2], ta[1]))),
            a[2][0].mul_add(tb[0], a[2][1].mul_add(tb[1], a[2][2].mul_add(tb[2], ta[2]))),
        ];

        Self {
            linear,
            translation,
        }
    }

    /// Returns the inverse transform `T⁻¹` such that
    /// `self.compose(&self.inverse()?).apply_to_vertex(v) == v` and
    /// `self.inverse()?.compose(self).apply_to_vertex(v) == v`.
    ///
    /// Returns [`TransformError::SingularLinear`] if the linear part is
    /// singular (determinant magnitude below [`SINGULARITY_THRESHOLD`]).
    pub fn inverse(&self) -> Result<Self, TransformError> {
        let det = self.determinant();
        if !gt_strict(det.abs(), SINGULARITY_THRESHOLD) {
            return Err(TransformError::SingularLinear);
        }
        let inv_det = 1.0 / det;

        let m = &self.linear;
        // M⁻¹ = adj(M) / det = cof(M)ᵀ / det.
        let inv_linear = [
            [
                (m[1][1].mul_add(m[2][2], -m[1][2] * m[2][1])) * inv_det,
                -(m[0][1].mul_add(m[2][2], -m[0][2] * m[2][1])) * inv_det,
                (m[0][1].mul_add(m[1][2], -m[0][2] * m[1][1])) * inv_det,
            ],
            [
                -(m[1][0].mul_add(m[2][2], -m[1][2] * m[2][0])) * inv_det,
                (m[0][0].mul_add(m[2][2], -m[0][2] * m[2][0])) * inv_det,
                -(m[0][0].mul_add(m[1][2], -m[0][2] * m[1][0])) * inv_det,
            ],
            [
                (m[1][0].mul_add(m[2][1], -m[1][1] * m[2][0])) * inv_det,
                -(m[0][0].mul_add(m[2][1], -m[0][1] * m[2][0])) * inv_det,
                (m[0][0].mul_add(m[1][1], -m[0][1] * m[1][0])) * inv_det,
            ],
        ];

        // For T(v) = M·v + t, the inverse is T⁻¹(w) = M⁻¹·(w - t)
        //                             = M⁻¹·w + (-M⁻¹·t).
        let t = &self.translation;
        let il = &inv_linear;
        let inv_translation = [
            -(il[0][0].mul_add(t[0], il[0][1].mul_add(t[1], il[0][2] * t[2]))),
            -(il[1][0].mul_add(t[0], il[1][1].mul_add(t[1], il[1][2] * t[2]))),
            -(il[2][0].mul_add(t[0], il[2][1].mul_add(t[1], il[2][2] * t[2]))),
        ];

        Ok(Self {
            linear: inv_linear,
            translation: inv_translation,
        })
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
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
    }

    fn assert_vector_approx(actual: Vector3D<f32>, expected: [f32; 3], eps: f32) {
        for (a, e) in actual.0.iter().zip(expected.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
    }

    fn assert_normal_approx(actual: Normal, expected: [f32; 3], eps: f32) {
        for (a, e) in actual.iter().zip(expected.iter()) {
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
        // every axis. We don't reject it here; callers (e.g. the future
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
        // pattern. Specifically, the matrix is (2/3)·J - I where J is the
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
        assert_eq!(result.unwrap_err(), TransformError::ZeroAxis);
    }

    #[test]
    fn from_axis_angle_zero_angle_is_exact_identity() {
        // sin(0) and cos(0) are both representable exactly in IEEE 754,
        // so one_minus_c = 0 and every Rodrigues term collapses; the
        // matrix is bit-identical to the identity regardless of axis.
        let identity = AffineTransform3D::identity();
        for axis in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [1.0, 1.0, 1.0]] {
            let t = AffineTransform3D::from_axis_angle(axis, 0.0).unwrap();
            assert_eq!(t.linear, identity.linear, "axis {:?}", axis);
        }
    }

    #[test]
    fn from_axis_angle_subnormal_angle_is_near_identity() {
        // Angle 1e-9 rad: sin ≈ 1e-9, 1 - cos ≈ 5e-19 (below f64 precision
        // for 1.0). The resulting linear part must differ from identity
        // only on the order of the angle, with no catastrophic
        // cancellation in the Rodrigues formula.
        let t = AffineTransform3D::from_axis_angle([0.0, 0.0, 1.0], 1.0e-9).unwrap();
        let identity = AffineTransform3D::identity();
        assert_linear_approx(&t.linear, &identity.linear, 1.0e-8);
    }

    #[test]
    fn from_axis_angle_two_pi_is_near_identity() {
        // 2π is not exactly representable in f64; sin(2π) ≈ -2.4e-16,
        // 1 - cos(2π) ≈ 1.7e-16. The resulting linear part must be
        // identity to within that residual, with no axis-dependent
        // drift introduced by the cofactor-style cross terms.
        let t = AffineTransform3D::from_axis_angle([0.0, 0.0, 1.0], 2.0 * PI).unwrap();
        let identity = AffineTransform3D::identity();
        assert_linear_approx(&t.linear, &identity.linear, 1.0e-14);
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
        assert_approx_eq!(f32, far_rot[0] - near_rot[0], 0.0, epsilon = 1e-3);
        assert_approx_eq!(f32, far_rot[1] - near_rot[1], 1.0, epsilon = 1e-3);
        assert_approx_eq!(f32, far_rot[2] - near_rot[2], 0.0, epsilon = 1e-3);
    }

    #[test]
    fn apply_to_vertex_overflow_produces_inf_caught_by_validate_vertices() {
        // f64 product 1e30 * 1e15 = 1e45 saturates to f32::INFINITY on
        // downcast. The transformed mesh must remain constructible; the
        // contract is that the next `validate_vertices()` call reports
        // the affected vertex / axis. Downstream callers rely on this
        // catch instead of an inline `apply_to_vertex` error path.
        let t = AffineTransform3D::from_uniform_scale(1.0e30);
        let v = t.apply_to_vertex(Vertex::new([1.0e15, 0.0, 0.0]));
        assert!(v[0].is_infinite());
        assert!(v[0] > 0.0);
        assert_eq!(v[1], 0.0);
        assert_eq!(v[2], 0.0);

        let mesh = crate::IndexedMesh::new(None, vec![v], vec![]);
        let errors = mesh
            .validate_vertices()
            .expect_err("transformed inf vertex must surface as InfiniteCoordinate");
        assert!(errors.iter().any(|e| matches!(
            e,
            crate::indexed_mesh::MeshVertexError::InfiniteCoordinate {
                vertex_idx: 0,
                coord: crate::primitives::CoordAxis::X,
            }
        )));
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
        // (correct here because the normal has no X component), so a
        // more meaningful test uses a tilted plane.
        //
        // Consider the surface containing (1,0,0), (0,1,0), (0,0,0).
        // Its (unit) normal is (1, 1, 0)/√2. Apply non-uniform scale
        // s = (2, 1, 1). The transformed surface contains (2,0,0),
        // (0,1,0), (0,0,0); its true normal is (1, 2, 0)/√5.
        //
        // Using the linear part naively would give
        // (2,1,0)/√5 ≠ (1,2,0)/√5; the inverse-transpose is required.
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
        let mag = result.iter().map(|c| c * c).sum::<f32>().sqrt();
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
        assert_eq!(
            t.apply_to_normal(n).unwrap_err(),
            TransformError::SingularLinear
        );
    }

    #[test]
    fn apply_to_normal_zero_input_returns_error() {
        // A zero-length normal cannot be normalised after multiplication.
        let t = AffineTransform3D::from_uniform_scale(2.0);
        let n = Normal::new([0.0, 0.0, 0.0]);
        assert_eq!(
            t.apply_to_normal(n).unwrap_err(),
            TransformError::ZeroMagnitudeNormal
        );
    }

    #[test]
    fn apply_to_normal_at_exact_singularity_threshold_returns_error() {
        // det = SINGULARITY_THRESHOLD exactly. The check uses
        // `gt_strict`, so equal-to-threshold is treated as singular.
        // Diagonal scale: det = f64::EPSILON · 1 · 1 = f64::EPSILON.
        let t = AffineTransform3D::from_linear_and_translation(
            [[f64::EPSILON, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0.0, 0.0, 0.0],
        );
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_eq!(
            t.apply_to_normal(n).unwrap_err(),
            TransformError::SingularLinear
        );
    }

    #[test]
    fn apply_to_normal_just_above_singularity_threshold_succeeds() {
        // det = 2 · SINGULARITY_THRESHOLD: the smallest determinant
        // that should still pass the first guard. The normal is
        // aligned with the squeezed axis so the cofactor product
        // (`cof[0] · n = [1, 0, 0]`) has unit magnitude, isolating
        // the determinant check from the post-multiply magnitude
        // check that fires for normals perpendicular to a collapsing
        // direction.
        let t = AffineTransform3D::from_linear_and_translation(
            [
                [2.0 * f64::EPSILON, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            [0.0, 0.0, 0.0],
        );
        let n = Normal::new([1.0, 0.0, 0.0]);
        assert_normal_approx(t.apply_to_normal(n).unwrap(), [1.0, 0.0, 0.0], 1e-6);
    }

    #[test]
    fn apply_to_normal_negative_determinant_uses_abs_threshold() {
        // det = -SINGULARITY_THRESHOLD. The check is on the magnitude,
        // so this must still be rejected (mirrors with a singular
        // magnitude are no more invertible than scales are).
        let t = AffineTransform3D::from_linear_and_translation(
            [[-f64::EPSILON, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0.0, 0.0, 0.0],
        );
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_eq!(
            t.apply_to_normal(n).unwrap_err(),
            TransformError::SingularLinear
        );
    }

    #[test]
    fn apply_to_normal_nan_determinant_returns_error() {
        // A NaN entry propagates into the determinant. `gt_strict`
        // rejects NaN by returning false, so the singular branch
        // fires. Without this guard a NaN linear part would propagate
        // through the cofactor matrix and surface as a non-finite
        // normal much further downstream.
        let t = AffineTransform3D::from_linear_and_translation(
            [[f64::NAN, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0.0, 0.0, 0.0],
        );
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_eq!(
            t.apply_to_normal(n).unwrap_err(),
            TransformError::SingularLinear
        );
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

    fn assert_transform_approx(a: &AffineTransform3D, b: &AffineTransform3D, eps: f64) {
        assert_linear_approx(&a.linear, &b.linear, eps);
        for (av, bv) in a.translation.iter().zip(b.translation.iter()) {
            assert_approx_eq!(f64, *av, *bv, epsilon = eps);
        }
    }

    #[test]
    fn determinant_identity_is_one() {
        assert_approx_eq!(f64, AffineTransform3D::identity().determinant(), 1.0);
    }

    #[test]
    fn determinant_translation_is_one() {
        let t = AffineTransform3D::from_translation([5.0, -3.0, 1.5]);
        assert_approx_eq!(f64, t.determinant(), 1.0);
    }

    #[test]
    fn determinant_uniform_scale_is_cubed() {
        let t = AffineTransform3D::from_uniform_scale(2.0);
        assert_approx_eq!(f64, t.determinant(), 8.0);
    }

    #[test]
    fn determinant_non_uniform_scale_is_product() {
        let t = AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0]);
        assert_approx_eq!(f64, t.determinant(), 24.0);
    }

    #[test]
    fn determinant_rotation_is_one() {
        let t = AffineTransform3D::from_rotation_z(FRAC_PI_4);
        assert_approx_eq!(f64, t.determinant(), 1.0, epsilon = 1e-15);
    }

    #[test]
    fn determinant_mirror_is_negative() {
        let t = AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]);
        assert_approx_eq!(f64, t.determinant(), -1.0);
    }

    #[test]
    fn determinant_singular_is_zero() {
        // Two identical rows ⇒ rank 2 ⇒ determinant 0.
        let t = AffineTransform3D::from_linear_and_translation(
            [[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            [0.0, 0.0, 0.0],
        );
        assert_approx_eq!(f64, t.determinant(), 0.0, epsilon = 1e-15);
    }

    #[test]
    fn compose_with_identity_on_either_side_is_a_no_op() {
        let a = AffineTransform3D::from_axis_angle([1.0, 2.0, 3.0], FRAC_PI_4)
            .unwrap()
            .compose(&AffineTransform3D::from_translation([4.0, 5.0, 6.0]));
        let id = AffineTransform3D::identity();
        assert_transform_approx(&id.compose(&a), &a, 1e-15);
        assert_transform_approx(&a.compose(&id), &a, 1e-15);
    }

    #[test]
    fn compose_apply_equals_sequential_apply() {
        // (A ∘ B)(v) == A(B(v)) for an arbitrary point.
        let a = AffineTransform3D::from_rotation_z(FRAC_PI_4);
        let b = AffineTransform3D::from_translation([1.0, 2.0, 3.0]);
        let v = Vertex::new([4.0, -5.0, 6.0]);
        let composed = a.compose(&b).apply_to_vertex(v);
        let sequential = a.apply_to_vertex(b.apply_to_vertex(v));
        assert_vertex_approx(composed, sequential.coords(), 1e-5);
    }

    #[test]
    fn compose_is_associative() {
        // (A ∘ B) ∘ C == A ∘ (B ∘ C). We compare the resulting transforms
        // structurally, not just their action on a single point.
        let a = AffineTransform3D::from_rotation_x(0.3);
        let b = AffineTransform3D::from_non_uniform_scale([1.5, 0.75, 2.0]);
        let c = AffineTransform3D::from_translation([7.0, -2.0, 0.5]);
        let left = a.compose(&b).compose(&c);
        let right = a.compose(&b.compose(&c));
        assert_transform_approx(&left, &right, 1e-14);
    }

    #[test]
    fn compose_translates_translations() {
        // Composing two pure translations adds their offsets.
        let a = AffineTransform3D::from_translation([1.0, 2.0, 3.0]);
        let b = AffineTransform3D::from_translation([10.0, 20.0, 30.0]);
        let composed = a.compose(&b);
        assert_transform_approx(
            &composed,
            &AffineTransform3D::from_translation([11.0, 22.0, 33.0]),
            1e-15,
        );
    }

    #[test]
    fn compose_translation_then_rotation_orders_correctly() {
        // Apply translation first, then rotation: R ∘ T.
        // For v = (1, 0, 0): T sends it to (2, 0, 0); R(π/2 about Z)
        // sends that to (0, 2, 0).
        let r = AffineTransform3D::from_rotation_z(FRAC_PI_2);
        let t = AffineTransform3D::from_translation([1.0, 0.0, 0.0]);
        let composed = r.compose(&t);
        let v = Vertex::new([1.0, 0.0, 0.0]);
        assert_vertex_approx(composed.apply_to_vertex(v), [0.0, 2.0, 0.0], 1e-6);
    }

    #[test]
    fn compose_chain_of_twenty_subrotations_is_near_full_rotation() {
        // 20 rotations of π/10 about Z compose to 2π, i.e. identity.
        // The check stresses matrix-multiply drift across the chain:
        // each compose introduces ~3·eps per entry; 20 of them should
        // stay within ~6e-15 of identity. The 1e-12 tolerance leaves
        // generous headroom for platform variation.
        let step = AffineTransform3D::from_rotation_z(PI / 10.0);
        let chain = (0..20).fold(AffineTransform3D::identity(), |acc, _| acc.compose(&step));
        assert_transform_approx(&chain, &AffineTransform3D::identity(), 1e-12);
    }

    #[test]
    fn compose_chain_apply_matches_sequential_apply_at_length_twenty() {
        // Build a 20-element heterogeneous chain (rotation, scale,
        // translation cycling); applying the composed transform to a
        // point must match the sequential apply within f32 precision,
        // even as f64 internals accumulate roundings along the way.
        let parts: Vec<AffineTransform3D> = (0..20)
            .map(|i| match i % 3 {
                0 => AffineTransform3D::from_rotation_z(PI / 17.0 + (i as f64) * 0.01),
                1 => AffineTransform3D::from_non_uniform_scale([
                    1.0 + (i as f64) * 0.001,
                    1.0 - (i as f64) * 0.001,
                    1.0,
                ]),
                _ => AffineTransform3D::from_translation([
                    (i as f64) * 0.5,
                    -(i as f64) * 0.25,
                    (i as f64) * 0.1,
                ]),
            })
            .collect();
        let composed = parts
            .iter()
            .fold(AffineTransform3D::identity(), |acc, p| acc.compose(p));
        let v = Vertex::new([2.5, -1.25, 3.75]);
        let via_compose = composed.apply_to_vertex(v);
        let via_sequence = parts.iter().rev().fold(v, |acc, p| p.apply_to_vertex(acc));
        assert_vertex_approx(via_compose, via_sequence.coords(), 1e-3);
    }

    #[test]
    fn inverse_of_identity_is_identity() {
        let inv = AffineTransform3D::identity().inverse().unwrap();
        assert_transform_approx(&inv, &AffineTransform3D::identity(), 1e-15);
    }

    #[test]
    fn inverse_of_translation_negates_offset() {
        let t = AffineTransform3D::from_translation([1.0, -2.0, 3.5]);
        let inv = t.inverse().unwrap();
        assert_transform_approx(
            &inv,
            &AffineTransform3D::from_translation([-1.0, 2.0, -3.5]),
            1e-15,
        );
    }

    #[test]
    fn inverse_compose_self_is_identity() {
        let cases = [
            AffineTransform3D::from_translation([3.0, -1.0, 2.0]),
            AffineTransform3D::from_uniform_scale(2.5),
            AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0]),
            AffineTransform3D::from_rotation_x(0.7),
            AffineTransform3D::from_rotation_y(-1.2),
            AffineTransform3D::from_rotation_z(FRAC_PI_4),
            AffineTransform3D::from_axis_angle([1.0, 2.0, 3.0], 0.4).unwrap(),
        ];
        for t in cases.iter() {
            let inv = t.inverse().unwrap();
            assert_transform_approx(&t.compose(&inv), &AffineTransform3D::identity(), 1e-12);
            assert_transform_approx(&inv.compose(t), &AffineTransform3D::identity(), 1e-12);
        }
    }

    #[test]
    fn inverse_of_combined_transform_is_correct() {
        // Build a non-trivial transform: rotate, scale non-uniformly,
        // translate. Verify that inverse undoes it on a point.
        let t = AffineTransform3D::from_translation([10.0, -5.0, 2.0])
            .compose(&AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0]))
            .compose(&AffineTransform3D::from_rotation_z(FRAC_PI_4));
        let inv = t.inverse().unwrap();
        let v = Vertex::new([1.5, -0.5, 3.25]);
        let round_trip = inv.apply_to_vertex(t.apply_to_vertex(v));
        assert_vertex_approx(round_trip, v.coords(), 1e-4);
    }

    #[test]
    fn inverse_singular_returns_error() {
        let t = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        assert_eq!(t.inverse().unwrap_err(), TransformError::SingularLinear);
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

/// Per-object transform. The `Identity` variant lets callers skip the
/// matrix application entirely for the (very common) case where an object
/// has not been moved since import.
///
/// Semantics: parent-relative transform of a graph node, i.e. the
/// composition of an ancestor chain's `Transform3D`s yields the node's
/// world transform. For a top-level node, the parent-relative
/// transform equals the world transform.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Transform3D {
    #[default]
    Identity,
    /// 3x3 linear part plus a translation.
    Affine(AffineTransform3D),
}

impl Transform3D {
    /// Applies the transform to a point. Identity short-circuits.
    pub fn apply_to_vertex(&self, v: Vertex) -> Vertex {
        match self {
            Self::Identity => v,
            Self::Affine(a) => a.apply_to_vertex(v),
        }
    }

    /// Applies the transform to a unit surface normal. Identity
    /// short-circuits; for `Affine`, see
    /// `AffineTransform3D::apply_to_normal` for the precise semantics
    /// (inverse-transpose with sign-flip on negative determinant, so that
    /// the result matches a winding-reversed triangle).
    pub fn apply_to_normal(&self, n: Normal) -> Result<Normal, TransformError> {
        match self {
            Self::Identity => Ok(n),
            Self::Affine(a) => a.apply_to_normal(n),
        }
    }

    /// Composes two transforms: `self.compose(other)` applies `other`
    /// first and then `self`. Identity short-circuits on either side
    /// without touching float arithmetic.
    pub fn compose(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Identity, _) => *other,
            (_, Self::Identity) => *self,
            (Self::Affine(a), Self::Affine(b)) => Self::Affine(a.compose(b)),
        }
    }

    /// Inverse of the transform. Identity is its own inverse;
    /// `Affine` propagates the singular-matrix error from
    /// `AffineTransform3D::inverse`.
    pub fn inverse(&self) -> Result<Self, TransformError> {
        match self {
            Self::Identity => Ok(Self::Identity),
            Self::Affine(a) => Ok(Self::Affine(a.inverse()?)),
        }
    }

    /// Determinant of the linear part. Identity returns `1.0` exactly
    /// without going through the cofactor expansion.
    pub fn determinant(&self) -> f64 {
        match self {
            Self::Identity => 1.0,
            Self::Affine(a) => a.determinant(),
        }
    }

    /// Whether the transform is the explicit identity variant. Useful
    /// for callers that want to take a fast path without doing the full
    /// match themselves.
    pub fn is_identity(&self) -> bool {
        matches!(self, Self::Identity)
    }
}

#[cfg(test)]
mod transform_3d_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn translation_affine() -> AffineTransform3D {
        AffineTransform3D::from_translation([1.0, 2.0, 3.0])
    }

    #[test]
    fn identity_is_default() {
        assert_eq!(Transform3D::default(), Transform3D::Identity);
    }

    #[test]
    fn is_identity_distinguishes_variants() {
        assert!(Transform3D::Identity.is_identity());
        assert!(!Transform3D::Affine(translation_affine()).is_identity());
    }

    #[test]
    fn identity_apply_to_vertex_is_pass_through() {
        let v = Vertex::new([1.5, -2.0, 7.25]);
        assert_eq!(Transform3D::Identity.apply_to_vertex(v), v);
    }

    #[test]
    fn affine_apply_to_vertex_delegates_to_inner() {
        let t = Transform3D::Affine(translation_affine());
        let v = Vertex::new([10.0, 20.0, 30.0]);
        let expected = translation_affine().apply_to_vertex(v);
        assert_eq!(t.apply_to_vertex(v), expected);
    }

    #[test]
    fn identity_apply_to_normal_is_pass_through() {
        let n = Normal::new([0.0, 0.0, 1.0]);
        assert_eq!(Transform3D::Identity.apply_to_normal(n).unwrap(), n);
    }

    #[test]
    fn affine_apply_to_normal_delegates_to_inner() {
        let t = Transform3D::Affine(AffineTransform3D::from_rotation_z(
            std::f64::consts::FRAC_PI_2,
        ));
        let n = Normal::new([1.0, 0.0, 0.0]);
        let result = t.apply_to_normal(n).unwrap();
        // π/2 rotation about Z sends +X to +Y.
        assert_approx_eq!(f32, result[0], 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result[1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f32, result[2], 0.0, epsilon = 1e-6);
    }

    #[test]
    fn compose_identity_with_anything_returns_other() {
        let a = Transform3D::Affine(translation_affine());
        assert_eq!(Transform3D::Identity.compose(&a), a);
        assert_eq!(a.compose(&Transform3D::Identity), a);
    }

    #[test]
    fn compose_two_identities_is_identity() {
        let composed = Transform3D::Identity.compose(&Transform3D::Identity);
        assert_eq!(composed, Transform3D::Identity);
    }

    #[test]
    fn compose_two_affines_returns_affine() {
        let a = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0]));
        let b = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 2.0, 0.0]));
        match a.compose(&b) {
            Transform3D::Affine(inner) => {
                assert_eq!(inner.translation, [1.0, 2.0, 0.0]);
            }
            Transform3D::Identity => panic!("expected Affine, got Identity"),
        }
    }

    #[test]
    fn inverse_of_identity_is_identity() {
        assert_eq!(
            Transform3D::Identity.inverse().unwrap(),
            Transform3D::Identity
        );
    }

    #[test]
    fn inverse_of_affine_round_trips() {
        let a = Transform3D::Affine(translation_affine());
        let inv = a.inverse().unwrap();
        let composed = a.compose(&inv);
        // The composition equals the affine identity, but the variant is
        // `Affine`, not `Identity`: `compose` doesn't fold an `Affine`
        // `Affine` back to `Identity`. Check the action on a point.
        let v = Vertex::new([5.0, 7.0, -1.0]);
        let round_trip = composed.apply_to_vertex(v);
        assert_approx_eq!(f32, round_trip[0], v[0], epsilon = 1e-5);
        assert_approx_eq!(f32, round_trip[1], v[1], epsilon = 1e-5);
        assert_approx_eq!(f32, round_trip[2], v[2], epsilon = 1e-5);
    }

    #[test]
    fn inverse_of_singular_affine_propagates_error() {
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        assert!(Transform3D::Affine(singular).inverse().is_err());
    }

    #[test]
    fn determinant_of_identity_is_one() {
        assert_approx_eq!(f64, Transform3D::Identity.determinant(), 1.0);
    }

    #[test]
    fn determinant_of_affine_delegates_to_inner() {
        let t = Transform3D::Affine(AffineTransform3D::from_uniform_scale(2.0));
        assert_approx_eq!(f64, t.determinant(), 8.0);
    }
}
