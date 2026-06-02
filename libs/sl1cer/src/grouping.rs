/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Containment analysis for a layer's worth of closed rings: decide
//! which rings are outer contours and which are holes nested inside
//! them.
//!
//! Inputs are assumed to be geometrically valid in the sense a
//! manifold-mesh slice guarantees: every pair of rings either nests
//! cleanly (one strictly contains the other) or is disjoint. Rings
//! that overlap but neither contain the other would break the
//! containment-tree assumption; the slicer's upstream contract rules
//! that case out.
//!
//! Depth is the number of other rings that contain a representative
//! point of the ring being measured. Even depth means outer; odd
//! depth means hole. The convention falls out of the even-odd
//! point-in-polygon rule applied to nested rings.

use crate::contour::ClosedRing;
use crate::geom2d::Point2D;

/// Even-odd point-in-polygon test. Shoots a horizontal ray in the +X
/// direction from `point` and counts how many edges of `ring` it
/// crosses. The classic "vertices touched by the ray" ambiguity is
/// resolved with the half-open-interval rule: an edge from `a` to `b`
/// is counted iff `(a.y > point.y) != (b.y > point.y)`. Each shared
/// vertex sits at the closed end of exactly one of its two adjoining
/// edges, so every vertex contributes to the crossing count at most
/// once even when the ray passes through it.
///
/// Rings with fewer than three vertices have no interior and the
/// function returns `false`.
pub fn point_in_ring(point: Point2D, ring: &ClosedRing) -> bool {
    let vs = ring.vertices();
    let n = vs.len();
    if n < 3 {
        return false;
    }
    let mut crossings = 0u32;
    for i in 0..n {
        let a = vs[i];
        let b = vs[(i + 1) % n];
        if (a.y > point.y) != (b.y > point.y) {
            let t = (point.y - a.y) / (b.y - a.y);
            let x_int = (b.x - a.x).mul_add(t, a.x);
            if x_int > point.x {
                crossings += 1;
            }
        }
    }
    crossings & 1 == 1
}

/// First vertex of a ring. Used as the representative point for
/// containment-depth queries.
///
/// The vertex sits on the ring's own boundary, so the representative
/// is never tested against its own ring. Against any other ring, a
/// boundary point of `ring` is inside the other iff `ring` itself is
/// inside the other, which is what containment-depth needs.
///
/// The mean-of-vertices alternative fails for concentric rings whose
/// vertex sets share a common centroid: every centroid then lies
/// inside every smaller concentric ring, and depth counts collapse.
pub(crate) fn ring_representative(ring: &ClosedRing) -> Point2D {
    let vs = ring.vertices();
    debug_assert!(!vs.is_empty(), "empty rings have no representative");
    vs[0]
}

/// Number of rings in `others` that contain a representative point of
/// `ring`. Precondition: `others` must not include `ring` itself.
pub fn containment_depth(ring: &ClosedRing, others: &[ClosedRing]) -> usize {
    let rep = ring_representative(ring);
    others
        .iter()
        .filter(|other| point_in_ring(rep, other))
        .count()
}

#[cfg(test)]
mod grouping_tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2D {
        Point2D::new(x, y)
    }

    fn ccw_square(cx: f64, cy: f64, half: f64) -> ClosedRing {
        ClosedRing::new(vec![
            p(cx - half, cy - half),
            p(cx + half, cy - half),
            p(cx + half, cy + half),
            p(cx - half, cy + half),
        ])
    }

    #[test]
    fn point_in_ring_returns_true_for_centre_of_unit_square() {
        let ring = ccw_square(0.5, 0.5, 0.5);
        assert!(point_in_ring(p(0.5, 0.5), &ring));
    }

    #[test]
    fn point_in_ring_returns_false_for_point_outside_unit_square() {
        let ring = ccw_square(0.5, 0.5, 0.5);
        assert!(!point_in_ring(p(2.0, 0.5), &ring));
        assert!(!point_in_ring(p(-1.0, 0.5), &ring));
        assert!(!point_in_ring(p(0.5, 2.0), &ring));
        assert!(!point_in_ring(p(0.5, -1.0), &ring));
    }

    #[test]
    fn point_in_ring_handles_ray_through_vertex_without_double_counting() {
        // A diamond with vertices at the four cardinal points; the ray
        // from the centre passes exactly through the rightmost vertex
        // (1, 0). The half-open rule must count that vertex at most
        // once across its two adjoining edges.
        let diamond = ClosedRing::new(vec![p(1.0, 0.0), p(0.0, 1.0), p(-1.0, 0.0), p(0.0, -1.0)]);
        assert!(point_in_ring(p(0.0, 0.0), &diamond));
    }

    #[test]
    fn point_in_ring_returns_false_for_rings_with_fewer_than_three_vertices() {
        assert!(!point_in_ring(p(0.0, 0.0), &ClosedRing::new(vec![])));
        assert!(!point_in_ring(
            p(0.0, 0.0),
            &ClosedRing::new(vec![p(0.0, 0.0)])
        ));
        assert!(!point_in_ring(
            p(0.0, 0.0),
            &ClosedRing::new(vec![p(0.0, 0.0), p(1.0, 0.0)])
        ));
    }

    #[test]
    fn containment_depth_counts_enclosing_rings() {
        let outer = ccw_square(0.0, 0.0, 2.0);
        let middle = ccw_square(0.0, 0.0, 1.0);
        let inner = ccw_square(0.0, 0.0, 0.25);
        // inner sits inside both middle and outer.
        let depth = containment_depth(&inner, &[outer.clone(), middle.clone()]);
        assert_eq!(depth, 2);
        // middle sits inside outer only.
        let depth = containment_depth(&middle, &[outer.clone(), inner.clone()]);
        assert_eq!(depth, 1);
        // outer sits inside nothing.
        let depth = containment_depth(&outer, &[middle.clone(), inner.clone()]);
        assert_eq!(depth, 0);
    }

    #[test]
    fn containment_depth_is_zero_for_empty_others_slice() {
        let ring = ccw_square(0.0, 0.0, 1.0);
        assert_eq!(containment_depth(&ring, &[]), 0);
    }
}
