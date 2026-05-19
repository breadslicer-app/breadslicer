/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-node transform and shared metadata used throughout the scene
//! graph. [`Transform3D`] is the affine-with-identity-shortcut local
//! transform carried by every graph node; [`SceneMetadata`] is the
//! optional name/comment pair shared by graph nodes and graph roots.

use crate::primitives::{Normal, Vertex};
use crate::transform::{AffineTransform3D, TransformError};

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneMetadata {
    pub name: Option<String>,
    pub comment: Option<String>,
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
