/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::ops::{Add, Mul, Sub};

use crate::primitives::Vertex;
use crate::transform::AffineTransform3D;

mod arvo_float_sealed {
    pub trait Sealed {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

/// Float types accepted by [`arvo_transformed_aabb`]. Sealed; only `f32`
/// and `f64` implement it, by design: the closed-form Arvo formula is
/// the same for any float and the trait only exists so a single source
/// of truth can serve both the storage-of-record path (f64, in
/// [`Aabb::transformed_arvo`]) and the f32 hot paths in adapter crates.
pub trait ArvoFloat:
    Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + arvo_float_sealed::Sealed
{
    fn abs(self) -> Self;
    fn mul_add(self, factor: Self, addend: Self) -> Self;
    /// Returns `0.5` in `Self`. Used by Arvo's centre/half-extents
    /// reformulation of `(min + max) / 2` and `(max - min) / 2`.
    fn half() -> Self;
}

impl ArvoFloat for f32 {
    #[inline]
    fn abs(self) -> Self {
        f32::abs(self)
    }
    #[inline]
    fn mul_add(self, factor: Self, addend: Self) -> Self {
        f32::mul_add(self, factor, addend)
    }
    #[inline]
    fn half() -> Self {
        0.5_f32
    }
}

impl ArvoFloat for f64 {
    #[inline]
    fn abs(self) -> Self {
        f64::abs(self)
    }
    #[inline]
    fn mul_add(self, factor: Self, addend: Self) -> Self {
        f64::mul_add(self, factor, addend)
    }
    #[inline]
    fn half() -> Self {
        0.5_f64
    }
}

/// Arvo's transformed-AABB formula, generic over the float type.
///
/// > For an AABB with centre `c` and half-extents `h`, the transformed
/// > AABB has centre `M·c + t` and half-extents
/// > `h'ᵢ = Σⱼ |Mᵢⱼ| · hⱼ`.
///
/// Source: Jim Arvo, "Transforming Axis-Aligned Bounding Boxes",
/// *Graphics Gems I* (1990), pp. 548-550. See also Ericson,
/// *Real-Time Collision Detection*, §4.2.6. The result is conservative:
/// it contains every transformed vertex and is exactly tight for
/// axis-aligned scaling and translation.
///
/// Returns `(new_min, new_max)`. Callers responsible for promoting /
/// downcasting to the float type they want to operate in; the empty
/// AABB sentinel is the caller's concern (this function would propagate
/// `±∞`/NaN through the centre/half-extents math).
pub fn arvo_transformed_aabb<F: ArvoFloat>(
    min: [F; 3],
    max: [F; 3],
    linear: &[[F; 3]; 3],
    translation: &[F; 3],
) -> ([F; 3], [F; 3]) {
    let half = F::half();
    let cx = (min[0] + max[0]) * half;
    let cy = (min[1] + max[1]) * half;
    let cz = (min[2] + max[2]) * half;
    let hx = (max[0] - min[0]) * half;
    let hy = (max[1] - min[1]) * half;
    let hz = (max[2] - min[2]) * half;

    let m = linear;
    let t = translation;

    let centre = [
        m[0][0].mul_add(cx, m[0][1].mul_add(cy, m[0][2].mul_add(cz, t[0]))),
        m[1][0].mul_add(cx, m[1][1].mul_add(cy, m[1][2].mul_add(cz, t[1]))),
        m[2][0].mul_add(cx, m[2][1].mul_add(cy, m[2][2].mul_add(cz, t[2]))),
    ];
    let half_extents = [
        m[0][0]
            .abs()
            .mul_add(hx, m[0][1].abs().mul_add(hy, m[0][2].abs() * hz)),
        m[1][0]
            .abs()
            .mul_add(hx, m[1][1].abs().mul_add(hy, m[1][2].abs() * hz)),
        m[2][0]
            .abs()
            .mul_add(hx, m[2][1].abs().mul_add(hy, m[2][2].abs() * hz)),
    ];

    (
        [
            centre[0] - half_extents[0],
            centre[1] - half_extents[1],
            centre[2] - half_extents[2],
        ],
        [
            centre[0] + half_extents[0],
            centre[1] + half_extents[1],
            centre[2] + half_extents[2],
        ],
    )
}

/// Axis-aligned bounding box in the same coordinate space as the
/// vertices it contains.
///
/// Construct as [`Aabb::empty`] (a degenerate box with `+∞`/`-∞`
/// sentinels that any subsequent [`Aabb::extend`] will replace) or via
/// [`Aabb::from_min_max`] when the corners are already known. The
/// invariant `min[i] ≤ max[i]` holds for every constructor and method
/// except `empty()`, which violates it on purpose so that the first
/// `extend` produces the same box as constructing from that point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    min: Vertex,
    max: Vertex,
}

impl Aabb {
    /// Degenerate "no points yet" box with `min = +∞` / `max = -∞`.
    /// Folding any finite point into this via `extend` yields the same
    /// box you would have got from constructing directly from that point.
    pub fn empty() -> Self {
        Self {
            min: Vertex::new([f32::INFINITY; 3]),
            max: Vertex::new([f32::NEG_INFINITY; 3]),
        }
    }

    /// Construct from explicit min/max corners. Debug-asserts that
    /// `min[i] ≤ max[i]` for every axis on finite inputs (NaN / infinity
    /// inputs skip the per-axis check so the empty-sentinel path stays
    /// representable).
    pub fn from_min_max(min: Vertex, max: Vertex) -> Self {
        debug_assert!(
            (0..3).all(|i| { !(min[i].is_finite() && max[i].is_finite()) || min[i] <= max[i] }),
            "Aabb::from_min_max: min={:?} > max={:?}",
            min.coords(),
            max.coords()
        );
        Self { min, max }
    }

    /// `true` when the box contains no points (i.e. it is still the
    /// `empty()` sentinel or was otherwise constructed with
    /// `min[i] > max[i]` on at least one axis).
    pub fn is_empty(&self) -> bool {
        (0..3).any(|i| self.min[i] > self.max[i])
    }

    pub fn min(&self) -> Vertex {
        self.min
    }

    pub fn max(&self) -> Vertex {
        self.max
    }

    /// Fold a point into the box. After the call, the box contains
    /// `p` and every point it contained before.
    pub fn extend(&mut self, p: Vertex) {
        let pc = p.coords();
        let mut min = self.min.coords();
        let mut max = self.max.coords();
        for i in 0..3 {
            min[i] = min[i].min(pc[i]);
            max[i] = max[i].max(pc[i]);
        }
        self.min = Vertex::new(min);
        self.max = Vertex::new(max);
    }

    /// Fold another box into this one (component-wise min on `min`,
    /// component-wise max on `max`).
    pub fn union(&mut self, other: &Aabb) {
        let mut min = self.min.coords();
        let mut max = self.max.coords();
        let omin = other.min.coords();
        let omax = other.max.coords();
        for i in 0..3 {
            min[i] = min[i].min(omin[i]);
            max[i] = max[i].max(omax[i]);
        }
        self.min = Vertex::new(min);
        self.max = Vertex::new(max);
    }

    /// World-space AABB of this object-local AABB after applying an
    /// `AffineTransform3D`, computed in `f64` via Arvo's method:
    ///
    /// > For an AABB with centre `c` and half-extents `h`, the transformed
    /// > AABB has centre `M·c + t` and half-extents
    /// > `h'ᵢ = Σⱼ |Mᵢⱼ| · hⱼ`.
    ///
    /// Source: Jim Arvo, "Transforming Axis-Aligned Bounding Boxes",
    /// *Graphics Gems I* (1990), pp. 548-550. See also Ericson,
    /// *Real-Time Collision Detection*, §4.2.6. The output is conservative:
    /// it contains every transformed vertex and is exactly tight for
    /// axis-aligned scaling and translation.
    ///
    /// If `self.is_empty()` (the `Aabb::empty()` sentinel, or any other
    /// box with `min[i] > max[i]` on some axis), the routine returns
    /// `Aabb::empty()` directly. Running the centre/half-extents math
    /// on the `±∞` sentinels would otherwise produce `NaN` corners that
    /// `is_empty()` cannot detect.
    pub fn transformed_arvo(&self, a: &AffineTransform3D) -> Aabb {
        if self.is_empty() {
            return Aabb::empty();
        }
        let lmin = self.min.coords();
        let lmax = self.max.coords();
        // Promote to f64 so the matrix-vector products match the precision of
        // the rest of the transform module.
        let min_f64 = [lmin[0] as f64, lmin[1] as f64, lmin[2] as f64];
        let max_f64 = [lmax[0] as f64, lmax[1] as f64, lmax[2] as f64];

        let (world_min, world_max) =
            arvo_transformed_aabb::<f64>(min_f64, max_f64, a.linear(), a.translation());

        Aabb::from_min_max(
            Vertex::new([
                world_min[0] as f32,
                world_min[1] as f32,
                world_min[2] as f32,
            ]),
            Vertex::new([
                world_max[0] as f32,
                world_max[1] as f32,
                world_max[2] as f32,
            ]),
        )
    }
}

#[cfg(test)]
mod aabb_tests {
    use super::*;

    #[test]
    fn empty_is_empty() {
        assert!(Aabb::empty().is_empty());
    }

    #[test]
    fn from_min_max_with_equal_corners_is_a_point() {
        let v = Vertex::new([1.0, 2.0, 3.0]);
        let aabb = Aabb::from_min_max(v, v);
        assert_eq!(aabb.min(), v);
        assert_eq!(aabb.max(), v);
        assert!(!aabb.is_empty());
    }

    #[test]
    fn extend_replaces_empty_sentinels() {
        let mut aabb = Aabb::empty();
        let v = Vertex::new([1.0, -2.0, 3.0]);
        aabb.extend(v);
        assert_eq!(aabb.min(), v);
        assert_eq!(aabb.max(), v);
        assert!(!aabb.is_empty());
    }

    #[test]
    fn extend_grows_in_both_directions() {
        let mut aabb =
            Aabb::from_min_max(Vertex::new([0.0, 0.0, 0.0]), Vertex::new([1.0, 1.0, 1.0]));
        aabb.extend(Vertex::new([-1.0, 0.5, 2.0]));
        assert_eq!(aabb.min(), Vertex::new([-1.0, 0.0, 0.0]));
        assert_eq!(aabb.max(), Vertex::new([1.0, 1.0, 2.0]));
    }

    #[test]
    fn extend_inside_does_not_change_box() {
        let outer = Aabb::from_min_max(
            Vertex::new([-1.0, -1.0, -1.0]),
            Vertex::new([1.0, 1.0, 1.0]),
        );
        let mut aabb = outer;
        aabb.extend(Vertex::new([0.0, 0.0, 0.0]));
        assert_eq!(aabb, outer);
    }

    #[test]
    fn union_with_empty_is_identity() {
        let outer = Aabb::from_min_max(
            Vertex::new([-1.0, -2.0, -3.0]),
            Vertex::new([1.0, 2.0, 3.0]),
        );
        let mut aabb = outer;
        aabb.union(&Aabb::empty());
        assert_eq!(aabb, outer);
    }

    #[test]
    fn union_combines_two_boxes() {
        let mut a = Aabb::from_min_max(Vertex::new([0.0, 0.0, 0.0]), Vertex::new([1.0, 1.0, 1.0]));
        let b = Aabb::from_min_max(Vertex::new([0.5, -1.0, 0.5]), Vertex::new([2.0, 0.5, 1.5]));
        a.union(&b);
        assert_eq!(a.min(), Vertex::new([0.0, -1.0, 0.0]));
        assert_eq!(a.max(), Vertex::new([2.0, 1.0, 1.5]));
    }

    #[test]
    fn empty_unioned_with_real_box_takes_the_real_box() {
        let real = Aabb::from_min_max(
            Vertex::new([-1.0, -2.0, -3.0]),
            Vertex::new([1.0, 2.0, 3.0]),
        );
        let mut aabb = Aabb::empty();
        aabb.union(&real);
        assert_eq!(aabb, real);
    }

    #[test]
    fn transformed_arvo_on_empty_with_identity_stays_empty() {
        // Without the short-circuit the centre/half-extents math
        // produces NaN corners (∞ + -∞ = NaN), which `is_empty()`
        // cannot detect; the bug surfaces only at the first
        // downstream consumer that trusts `is_empty()`.
        let out = Aabb::empty().transformed_arvo(&AffineTransform3D::identity());
        assert!(out.is_empty());
        assert_eq!(out, Aabb::empty());
    }

    #[test]
    fn transformed_arvo_on_empty_stays_empty_for_arbitrary_transform() {
        // Rotation, non-uniform scale and translation: each on its
        // own and combined. A real transform's matrix entries would
        // otherwise propagate through the ±∞ sentinels and produce
        // NaN coords (sign-mixed products of zero matrix entries
        // with the infinite extents).
        let combined = AffineTransform3D::from_rotation_z(0.7)
            .compose(&AffineTransform3D::from_non_uniform_scale([2.0, 0.5, 3.0]))
            .compose(&AffineTransform3D::from_translation([10.0, -5.0, 1.0]));
        for t in [
            AffineTransform3D::from_rotation_z(0.5),
            AffineTransform3D::from_non_uniform_scale([2.0, 0.5, 3.0]),
            AffineTransform3D::from_translation([5.0, -2.0, 1.0]),
            combined,
        ] {
            let out = Aabb::empty().transformed_arvo(&t);
            assert!(out.is_empty(), "transform {:?} did not preserve empty", t);
            assert_eq!(out, Aabb::empty());
        }
    }

    #[test]
    fn transformed_arvo_on_empty_does_not_propagate_nan_to_output() {
        // Explicit NaN-coord check: every output coord must be the
        // canonical ±∞ sentinel, never NaN. This is the regression
        // guard for the original break.
        let out = Aabb::empty().transformed_arvo(&AffineTransform3D::identity());
        for axis in 0..3 {
            assert!(!out.min()[axis].is_nan(), "axis {axis}: min is NaN");
            assert!(!out.max()[axis].is_nan(), "axis {axis}: max is NaN");
        }
    }
}
