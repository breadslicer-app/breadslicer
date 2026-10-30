/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fmt;

/// The coordinates of a vertex in a form with total equality, order and hash.
/// Vertices whose coordinates are equal as numbers give equal values, so `-0.0`
/// and `0.0` count as 1 value. The order has no geometric meaning: it only puts
/// equal values next to each other in a sort.
///
/// Only `Vertex::canonical_coords` builds it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalCoords([u32; 3]);

impl CanonicalCoords {
    /// Returns the canonical form of `coords`, or `None` when a coordinate is
    /// NaN or infinite.
    #[inline]
    pub(super) fn from_coords(coords: [f32; 3]) -> Option<Self> {
        let [x, y, z] = coords.map(|coord| {
            // Also true for -0.0, which then gets the bits of 0.0.
            let coord = if coord == 0.0 { 0.0 } else { coord };
            coord.is_finite().then_some(coord.to_bits())
        });
        Some(Self([x?, y?, z?]))
    }

    /// Returns the bit patterns of the canonical coordinates.
    #[inline]
    pub(crate) fn bits(self) -> [u32; 3] {
        self.0
    }
}

// Prints the coordinates instead of their bits.
impl fmt::Debug for CanonicalCoords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CanonicalCoords")
            .field(&self.0.map(f32::from_bits))
            .finish()
    }
}

#[cfg(test)]
mod canonical_coords_tests {
    use proptest::prelude::*;

    use super::*;

    fn canonical(coords: [f32; 3]) -> CanonicalCoords {
        CanonicalCoords::from_coords(coords).expect("finite coordinates")
    }

    #[test]
    fn negative_zero_equals_positive_zero() {
        for axis in 0..3 {
            let mut positive = [1.0, 2.0, 3.0];
            positive[axis] = 0.0;
            let mut negative = positive;
            negative[axis] = -0.0;
            assert_eq!(canonical(positive), canonical(negative), "axis {axis}");
        }
    }

    #[test]
    fn the_next_float_on_1_axis_gives_another_value() {
        for axis in 0..3 {
            let base = [1.0_f32, 2.0, 3.0];
            let mut moved = base;
            moved[axis] = base[axis].next_up();
            assert_ne!(canonical(base), canonical(moved), "axis {axis}");
        }
    }

    #[test]
    fn a_nan_or_infinite_coordinate_gives_none() {
        for axis in 0..3 {
            for bad in [f32::NAN, -f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut coords = [1.0, 2.0, 3.0];
                coords[axis] = bad;
                assert_eq!(CanonicalCoords::from_coords(coords), None, "{coords:?}");
            }
        }
    }

    #[test]
    fn debug_prints_the_coordinates() {
        assert_eq!(
            format!("{:?}", canonical([-0.0, 1.5, -2.25])),
            "CanonicalCoords([0.0, 1.5, -2.25])"
        );
        assert_eq!(
            format!("{:?}", canonical([f32::MIN, f32::MAX, f32::MIN_POSITIVE])),
            "CanonicalCoords([-3.4028235e38, 3.4028235e38, 1.1754944e-38])"
        );
    }

    fn pool() -> impl Strategy<Value = f32> {
        prop_oneof![
            Just(0.0_f32),
            Just(-0.0_f32),
            Just(1.0_f32),
            Just(-1.0_f32),
            Just(2.5_f32),
            Just(-2.5_f32),
            Just(1.0e-30_f32),
            Just(-1.0e-30_f32),
            Just(f32::MIN_POSITIVE),
            Just(f32::MIN),
            Just(f32::MAX),
        ]
    }

    proptest! {
        // `copy` makes `b` take some coordinates of `a`, so that equal pairs
        // are frequent.
        #[test]
        fn equal_values_exactly_when_the_coordinates_are_equal(
            a in [pool(), pool(), pool()],
            b in [pool(), pool(), pool()],
            copy in any::<[bool; 3]>(),
        ) {
            let b: [f32; 3] = std::array::from_fn(|i| if copy[i] { a[i] } else { b[i] });
            prop_assert_eq!(canonical(a) == canonical(b), a == b);
        }
    }
}
