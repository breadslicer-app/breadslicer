/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::cmp::Ordering;

use anyhow::{Result, anyhow};

use crate::primitives::{Vector3D, Vertex};

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
    /// surface normals see `apply_to_normal` (TBD), which uses the
    /// inverse-transpose.
    pub fn apply_to_vector(&self, v: Vector3D<f32>) -> Vector3D<f32> {
        let v = [v.0[0] as f64, v.0[1] as f64, v.0[2] as f64];
        let m = &self.linear;
        let x = m[0][0].mul_add(v[0], m[0][1].mul_add(v[1], m[0][2] * v[2]));
        let y = m[1][0].mul_add(v[0], m[1][1].mul_add(v[1], m[1][2] * v[2]));
        let z = m[2][0].mul_add(v[0], m[2][1].mul_add(v[1], m[2][2] * v[2]));
        Vector3D::new([x as f32, y as f32, z as f32])
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
    fn struct_is_copy() {
        // Compile-time check: assigning a value-typed transform should not
        // require an explicit `.clone()`.
        let a = AffineTransform3D::from_translation([1.0, 2.0, 3.0]);
        let b = a;
        assert_eq!(a, b);
    }
}
