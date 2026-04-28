/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::primitives::Vertex;

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
}
