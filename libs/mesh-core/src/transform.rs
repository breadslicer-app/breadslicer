/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

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
}

impl Default for AffineTransform3D {
    fn default() -> Self {
        Self::identity()
    }
}

#[cfg(test)]
mod affine_transform_3d_tests {
    use super::*;

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
    fn struct_is_copy() {
        // Compile-time check: assigning a value-typed transform should not
        // require an explicit `.clone()`.
        let a = AffineTransform3D::from_translation([1.0, 2.0, 3.0]);
        let b = a;
        assert_eq!(a, b);
    }
}
