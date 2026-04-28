/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 *
 * Property tests for `sc-mesh-core` algebra. These complement the
 * example-based tests embedded in each module by checking algebraic
 * identities (commutativity, associativity, inverse/identity laws) over
 * generated inputs.
 *
 * Float strategies are deliberately narrow: bounded magnitude, no NaN /
 * ±∞ / subnormals, and `prop_assume!` to reject near-degenerate cases.
 * Comparisons use `float_cmp::approx_eq` per the project's float rule.
 */

use float_cmp::approx_eq;
use proptest::prelude::*;
use sc_mesh_core::primitives::{Vector3D, Vertex};
use sc_mesh_core::transform::AffineTransform3D;

// ---------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------

/// Bounded finite f32 generator: magnitude up to ~1e3 to keep
/// `squared_norm` and `mul_add` chains far from overflow, no
/// subnormals or NaN.
fn bounded_f32() -> impl Strategy<Value = f32> {
    -1.0e3_f32..1.0e3_f32
}

/// Bounded finite f64 generator: magnitude up to ~1e3 for the same
/// reasons.
fn bounded_f64() -> impl Strategy<Value = f64> {
    -1.0e3_f64..1.0e3_f64
}

fn vector3d_f32() -> impl Strategy<Value = Vector3D<f32>> {
    (bounded_f32(), bounded_f32(), bounded_f32()).prop_map(|(x, y, z)| Vector3D::new([x, y, z]))
}

fn vector3d_f64() -> impl Strategy<Value = Vector3D<f64>> {
    (bounded_f64(), bounded_f64(), bounded_f64()).prop_map(|(x, y, z)| Vector3D::new([x, y, z]))
}

// ---------------------------------------------------------------------
// Vector3D properties
// ---------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn dot_product_is_commutative_f32(a in vector3d_f32(), b in vector3d_f32()) {
        let ab = a.dot_product(&b);
        let ba = b.dot_product(&a);
        // Tight relative tolerance: same operands re-ordered.
        prop_assert!(
            approx_eq!(f32, ab, ba, epsilon = 1e-3),
            "dot(a,b)={ab}, dot(b,a)={ba}"
        );
    }

    #[test]
    fn dot_product_is_commutative_f64(a in vector3d_f64(), b in vector3d_f64()) {
        let ab = a.dot_product(&b);
        let ba = b.dot_product(&a);
        prop_assert!(
            approx_eq!(f64, ab, ba, epsilon = 1e-9),
            "dot(a,b)={ab}, dot(b,a)={ba}"
        );
    }

    #[test]
    fn cross_product_is_anti_commutative_f64(a in vector3d_f64(), b in vector3d_f64()) {
        // cross(a, b) == -cross(b, a) — exact under reorder of the same
        // arithmetic, but `mul_add` rounding can cause 1-ulp differences,
        // so we still use approx_eq.
        let ab = a.cross_product(&b);
        let ba = b.cross_product(&a);
        for i in 0..3 {
            prop_assert!(
                approx_eq!(f64, ab[i], -ba[i], epsilon = 1e-9),
                "axis {i}: cross(a,b)={}, cross(b,a)={}",
                ab[i],
                ba[i]
            );
        }
    }

    #[test]
    fn cross_product_is_orthogonal_to_inputs_f64(
        a in vector3d_f64(),
        b in vector3d_f64(),
    ) {
        let c = a.cross_product(&b);
        let dot_a = c.dot_product(&a);
        let dot_b = c.dot_product(&b);
        // Magnitude-relative tolerance: |c| can be ~1e6 (bounded coords squared),
        // so absolute zero with 1e-6 epsilon would be too strict.
        let scale = c.norm().max(a.norm()).max(b.norm()).max(1.0);
        prop_assert!(
            approx_eq!(f64, dot_a / scale, 0.0, epsilon = 1e-9),
            "cross·a not orthogonal: {dot_a} (scale {scale})"
        );
        prop_assert!(
            approx_eq!(f64, dot_b / scale, 0.0, epsilon = 1e-9),
            "cross·b not orthogonal: {dot_b} (scale {scale})"
        );
    }

    #[test]
    fn diff_norm_is_symmetric_f32(a in vector3d_f32(), b in vector3d_f32()) {
        // ‖a - b‖ == ‖b - a‖.
        let n_ab = a.diff(&b).norm();
        let n_ba = b.diff(&a).norm();
        prop_assert!(
            approx_eq!(f32, n_ab, n_ba, epsilon = 1e-4),
            "‖a-b‖={n_ab}, ‖b-a‖={n_ba}"
        );
    }

    #[test]
    fn squared_norm_equals_dot_with_self_f64(a in vector3d_f64()) {
        let sq = a.squared_norm();
        let dot = a.dot_product(&a);
        prop_assert!(
            approx_eq!(f64, sq, dot, epsilon = 1e-9),
            "squared_norm={sq}, dot(a,a)={dot}"
        );
    }

    #[test]
    fn norm_is_non_negative_f32(a in vector3d_f32()) {
        let n = a.norm();
        prop_assert!(n >= 0.0, "norm is negative: {n}");
        prop_assert!(n.is_finite(), "norm is non-finite: {n}");
    }

    #[test]
    fn f32_to_f64_to_f32_round_trip(v in vector3d_f32()) {
        // Casting f32 -> f64 -> f32 is lossless for any finite, non-NaN
        // f32 (f64 can exactly represent every f32, the back cast picks
        // the same value bit-for-bit).
        let widened: Vector3D<f64> = v.into();
        let back: Vector3D<f32> = widened.into();
        prop_assert_eq!(v, back);
    }
}

// ---------------------------------------------------------------------
// AffineTransform3D properties
// ---------------------------------------------------------------------

fn affine_rotation_or_translation() -> impl Strategy<Value = AffineTransform3D> {
    // A grab-bag of well-conditioned transforms: pure translation, axis
    // rotation, uniform scale, and arbitrary axis-angle. All have a
    // determinant well away from zero, so inverse() is reliable.
    prop_oneof![
        (bounded_f64(), bounded_f64(), bounded_f64())
            .prop_map(|(x, y, z)| AffineTransform3D::from_translation([x, y, z])),
        (-std::f64::consts::PI..std::f64::consts::PI).prop_map(AffineTransform3D::from_rotation_x),
        (-std::f64::consts::PI..std::f64::consts::PI).prop_map(AffineTransform3D::from_rotation_y),
        (-std::f64::consts::PI..std::f64::consts::PI).prop_map(AffineTransform3D::from_rotation_z),
        (0.1_f64..10.0_f64).prop_map(AffineTransform3D::from_uniform_scale),
    ]
}

fn axis_angle_safe() -> impl Strategy<Value = ([f64; 3], f64)> {
    // Generate axes with magnitude clearly above the f64::EPSILON
    // threshold the constructor rejects. Bound the angle to one full
    // turn either way — Rodrigues is well-defined everywhere, this just
    // makes failure cases readable.
    (
        (
            -10.0_f64..10.0_f64,
            -10.0_f64..10.0_f64,
            -10.0_f64..10.0_f64,
        ),
        -std::f64::consts::TAU..std::f64::consts::TAU,
    )
        .prop_filter(
            "axis must have magnitude >> sqrt(EPSILON)",
            |((x, y, z), _)| x.mul_add(*x, y.mul_add(*y, z * z)) > 1.0,
        )
        .prop_map(|((x, y, z), a)| ([x, y, z], a))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn compose_with_identity_is_identity_left(t in affine_rotation_or_translation()) {
        let id = AffineTransform3D::identity();
        let composed = id.compose(&t);
        for i in 0..3 {
            for j in 0..3 {
                prop_assert!(
                    approx_eq!(f64, composed.linear()[i][j], t.linear()[i][j], epsilon = 1e-12),
                );
            }
            prop_assert!(
                approx_eq!(f64, composed.translation()[i], t.translation()[i], epsilon = 1e-12),
            );
        }
    }

    #[test]
    fn compose_with_identity_is_identity_right(t in affine_rotation_or_translation()) {
        let id = AffineTransform3D::identity();
        let composed = t.compose(&id);
        for i in 0..3 {
            for j in 0..3 {
                prop_assert!(
                    approx_eq!(f64, composed.linear()[i][j], t.linear()[i][j], epsilon = 1e-12),
                );
            }
            prop_assert!(
                approx_eq!(f64, composed.translation()[i], t.translation()[i], epsilon = 1e-12),
            );
        }
    }

    #[test]
    fn compose_is_associative(
        a in affine_rotation_or_translation(),
        b in affine_rotation_or_translation(),
        c in affine_rotation_or_translation(),
    ) {
        // (A ∘ B) ∘ C == A ∘ (B ∘ C). With well-conditioned generators
        // the structural comparison holds to ~1e-9.
        let left = a.compose(&b).compose(&c);
        let right = a.compose(&b.compose(&c));
        for i in 0..3 {
            for j in 0..3 {
                prop_assert!(
                    approx_eq!(f64, left.linear()[i][j], right.linear()[i][j], epsilon = 1e-9),
                    "linear[{i}][{j}]: left={}, right={}",
                    left.linear()[i][j], right.linear()[i][j]
                );
            }
            prop_assert!(
                approx_eq!(f64, left.translation()[i], right.translation()[i], epsilon = 1e-7),
            );
        }
    }

    #[test]
    fn compose_apply_equals_sequential_apply(
        a in affine_rotation_or_translation(),
        b in affine_rotation_or_translation(),
        v in vector3d_f32(),
    ) {
        // (A ∘ B)(v) == A(B(v)).
        let v = Vertex::new(v.coords());
        let composed = a.compose(&b).apply_to_vertex(v);
        let sequential = a.apply_to_vertex(b.apply_to_vertex(v));
        // The two routes accumulate slightly different f32 rounding, so
        // the tolerance is loose relative to the bounded coord range.
        for (i, (c, s)) in composed.iter().zip(sequential.iter()).enumerate() {
            prop_assert!(
                approx_eq!(f32, *c, *s, epsilon = 1e-1),
                "axis {i}: composed={c}, sequential={s}",
            );
        }
    }

    #[test]
    fn inverse_then_apply_is_round_trip(
        t in affine_rotation_or_translation(),
        v in vector3d_f32(),
    ) {
        let v = Vertex::new(v.coords());
        let inv = t.inverse().expect("generator only produces well-conditioned transforms");
        let round_trip = inv.apply_to_vertex(t.apply_to_vertex(v));
        // Loose tolerance: f64 internals lose precision when we round-
        // trip through f32 once and then once more.
        for (i, (r, x)) in round_trip.iter().zip(v.iter()).enumerate() {
            prop_assert!(
                approx_eq!(f32, *r, *x, epsilon = 5e-2),
                "axis {i}: round_trip={r}, v={x}",
            );
        }
    }

    #[test]
    fn from_axis_angle_with_non_zero_axis_succeeds_and_has_unit_determinant(
        (axis, angle) in axis_angle_safe(),
    ) {
        let t = AffineTransform3D::from_axis_angle(axis, angle).unwrap();
        let det = t.determinant();
        prop_assert!(
            approx_eq!(f64, det, 1.0, epsilon = 1e-9),
            "det={det} for axis={axis:?}, angle={angle}"
        );
    }

    #[test]
    fn from_axis_angle_is_invariant_to_axis_scaling(
        (axis, angle) in axis_angle_safe(),
        scale in 0.1_f64..10.0_f64,
    ) {
        // The constructor normalises the axis, so any positive scale
        // should yield the same matrix.
        let unit = AffineTransform3D::from_axis_angle(axis, angle).unwrap();
        let scaled = AffineTransform3D::from_axis_angle(
            [axis[0] * scale, axis[1] * scale, axis[2] * scale],
            angle,
        ).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                prop_assert!(
                    approx_eq!(f64, unit.linear()[i][j], scaled.linear()[i][j], epsilon = 1e-12),
                );
            }
        }
    }

    #[test]
    fn from_axis_angle_with_negated_axis_is_inverse_rotation(
        (axis, angle) in axis_angle_safe(),
    ) {
        // R(-axis, angle) == R(axis, -angle): negating the axis flips
        // the right-handed direction of rotation.
        let r = AffineTransform3D::from_axis_angle(axis, angle).unwrap();
        let r_neg_axis = AffineTransform3D::from_axis_angle(
            [-axis[0], -axis[1], -axis[2]],
            angle,
        ).unwrap();
        let r_neg_angle = AffineTransform3D::from_axis_angle(axis, -angle).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                prop_assert!(
                    approx_eq!(
                        f64,
                        r_neg_axis.linear()[i][j],
                        r_neg_angle.linear()[i][j],
                        epsilon = 1e-12
                    ),
                );
                let _ = r; // r itself isn't compared here; kept to keep the test self-explanatory.
            }
        }
    }
}
