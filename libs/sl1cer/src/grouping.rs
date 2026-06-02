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

use crate::contour::{ClosedRing, Polygon};
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

/// Pairing of one outer ring with its immediate hole rings, expressed
/// as indices into the original ring slice. Crate-internal because
/// the public surface is the `Vec<Polygon>` returned by the polygon
/// assembly step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RingGroup {
    pub outer_idx: usize,
    pub hole_indices: Vec<usize>,
}

/// Cluster `rings` into outer/hole groups by containment depth. Rings
/// at even depth are outer contours; rings at odd depth are holes
/// attached to the deepest enclosing outer (which, for cleanly nested
/// input, is the outer at depth `depth - 1`).
///
/// The returned vector lists one [`RingGroup`] per outer ring in the
/// order outers appear in `rings`. Hole indices within a group keep
/// the order they appear in `rings`.
pub(crate) fn group_rings(rings: &[ClosedRing]) -> Vec<RingGroup> {
    let n = rings.len();
    if n == 0 {
        return Vec::new();
    }

    let reps: Vec<Point2D> = rings.iter().map(ring_representative).collect();
    let depths: Vec<usize> = (0..n)
        .map(|i| {
            (0..n)
                .filter(|&j| j != i && point_in_ring(reps[i], &rings[j]))
                .count()
        })
        .collect();

    let mut groups: Vec<RingGroup> = Vec::new();
    let mut outer_to_group: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        if depths[i].is_multiple_of(2) {
            outer_to_group[i] = Some(groups.len());
            groups.push(RingGroup {
                outer_idx: i,
                hole_indices: Vec::new(),
            });
        }
    }
    for i in 0..n {
        if !depths[i].is_multiple_of(2) {
            let parent = (0..n)
                .filter(|&j| {
                    j != i && depths[j].is_multiple_of(2) && point_in_ring(reps[i], &rings[j])
                })
                .max_by_key(|&j| depths[j]);
            if let Some(p) = parent
                && let Some(g) = outer_to_group[p]
            {
                groups[g].hole_indices.push(i);
            }
        }
    }
    groups
}

/// Assemble closed CCW rings into outer-CCW + holes-CW polygons.
/// Outer rings keep their CCW vertex order; hole rings have their
/// vertex order reversed so they end up CW, matching the [`Polygon`]
/// invariant established at construction by
/// [`Polygon::new_unchecked`].
///
/// Precondition: every input ring has strictly positive signed area
/// (CCW), as produced by the stitcher's `normalise_to_ccw` pass.
/// Rings whose containment chain has no parent at the expected even
/// depth (a case ruled out for manifold-mesh input) are silently
/// dropped.
pub fn group_into_polygons(rings: Vec<ClosedRing>) -> Vec<Polygon> {
    let groups = group_rings(&rings);
    let mut taken: Vec<Option<ClosedRing>> = rings.into_iter().map(Some).collect();
    let mut polygons = Vec::with_capacity(groups.len());
    for group in groups {
        let outer = taken[group.outer_idx].take().unwrap();
        let holes: Vec<ClosedRing> = group
            .hole_indices
            .iter()
            .map(|&idx| {
                let mut hole = taken[idx].take().unwrap();
                hole.reverse();
                hole
            })
            .collect();
        polygons.push(Polygon::new_unchecked(outer, holes));
    }
    polygons
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

    #[test]
    fn group_rings_returns_empty_for_empty_input() {
        let groups = group_rings(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn group_rings_single_ring_is_one_outer_with_no_holes() {
        let rings = vec![ccw_square(0.0, 0.0, 1.0)];
        let groups = group_rings(&rings);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].outer_idx, 0);
        assert!(groups[0].hole_indices.is_empty());
    }

    #[test]
    fn group_rings_two_disjoint_rings_become_two_outers() {
        let rings = vec![ccw_square(0.0, 0.0, 1.0), ccw_square(5.0, 5.0, 1.0)];
        let groups = group_rings(&rings);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.hole_indices.is_empty()));
        let outer_indices: Vec<_> = groups.iter().map(|g| g.outer_idx).collect();
        assert_eq!(outer_indices, vec![0, 1]);
    }

    #[test]
    fn group_rings_nested_pair_yields_one_outer_with_one_hole() {
        let rings = vec![ccw_square(0.0, 0.0, 2.0), ccw_square(0.0, 0.0, 0.5)];
        let groups = group_rings(&rings);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].outer_idx, 0);
        assert_eq!(groups[0].hole_indices, vec![1]);
    }

    #[test]
    fn group_rings_three_concentric_levels_yield_two_polygons() {
        // Outer at depth 0, ring inside at depth 1 (hole of outer),
        // smaller ring at depth 2 (its own outer, sits inside the
        // depth-1 hole).
        let rings = vec![
            ccw_square(0.0, 0.0, 4.0),
            ccw_square(0.0, 0.0, 2.0),
            ccw_square(0.0, 0.0, 0.5),
        ];
        let groups = group_rings(&rings);
        assert_eq!(groups.len(), 2);
        let group_for_outer = |outer_idx: usize| {
            groups
                .iter()
                .find(|g| g.outer_idx == outer_idx)
                .expect("expected an outer group for this index")
        };
        assert_eq!(group_for_outer(0).hole_indices, vec![1]);
        assert!(group_for_outer(2).hole_indices.is_empty());
    }

    #[test]
    fn group_rings_holes_attach_to_immediate_parent_not_root() {
        // outer A (depth 0) contains hole A' (depth 1) which contains
        // outer B (depth 2) which contains hole H (depth 3). H must
        // attach to B, not A.
        let a = ccw_square(0.0, 0.0, 10.0);
        let a_hole = ccw_square(0.0, 0.0, 7.0);
        let b = ccw_square(0.0, 0.0, 5.0);
        let h = ccw_square(0.0, 0.0, 1.0);
        let rings = vec![a, a_hole, b, h];
        let groups = group_rings(&rings);
        assert_eq!(groups.len(), 2);
        let group_a = groups
            .iter()
            .find(|g| g.outer_idx == 0)
            .expect("group for outer A");
        let group_b = groups
            .iter()
            .find(|g| g.outer_idx == 2)
            .expect("group for outer B");
        assert_eq!(group_a.hole_indices, vec![1]);
        assert_eq!(group_b.hole_indices, vec![3]);
    }

    #[test]
    fn group_into_polygons_empty_input_returns_no_polygons() {
        let polygons = group_into_polygons(vec![]);
        assert!(polygons.is_empty());
    }

    #[test]
    fn group_into_polygons_single_ring_yields_one_outer_polygon_ccw() {
        let outer = ccw_square(0.0, 0.0, 1.0);
        let polygons = group_into_polygons(vec![outer.clone()]);
        assert_eq!(polygons.len(), 1);
        assert_eq!(polygons[0].outer(), &outer);
        assert!(polygons[0].holes().is_empty());
        assert!(polygons[0].outer().signed_area() > 0.0);
    }

    #[test]
    fn group_into_polygons_nested_pair_emits_one_polygon_with_one_cw_hole() {
        let outer = ccw_square(0.0, 0.0, 2.0);
        let inner = ccw_square(0.0, 0.0, 0.5);
        let polygons = group_into_polygons(vec![outer, inner]);
        assert_eq!(polygons.len(), 1);
        assert!(polygons[0].outer().signed_area() > 0.0, "outer must be CCW");
        assert_eq!(polygons[0].holes().len(), 1);
        assert!(
            polygons[0].holes()[0].signed_area() < 0.0,
            "hole must be CW after group_into_polygons"
        );
    }

    #[test]
    fn group_into_polygons_three_concentric_levels_have_correct_windings() {
        // Same fixture as the group_rings concentric test, but check
        // every ring's winding on the assembled Polygon values.
        let a = ccw_square(0.0, 0.0, 4.0);
        let b = ccw_square(0.0, 0.0, 2.0);
        let c = ccw_square(0.0, 0.0, 0.5);
        let polygons = group_into_polygons(vec![a, b, c]);
        assert_eq!(polygons.len(), 2);
        for poly in &polygons {
            assert!(
                poly.outer().signed_area() > 0.0,
                "every outer ring must be CCW",
            );
            for hole in poly.holes() {
                assert!(hole.signed_area() < 0.0, "every hole ring must be CW");
            }
        }
    }

    #[test]
    fn group_into_polygons_output_passes_checked_polygon_new() {
        // The grouping step claims to establish the Polygon invariants
        // by construction. Rebuilding each polygon through the checked
        // Polygon::new must therefore succeed.
        let outer = ccw_square(0.0, 0.0, 2.0);
        let hole = ccw_square(0.0, 0.0, 0.5);
        let polygons = group_into_polygons(vec![outer, hole]);
        assert_eq!(polygons.len(), 1);
        let outer_ring = polygons[0].outer().clone();
        let hole_rings: Vec<ClosedRing> = polygons[0].holes().to_vec();
        let rebuilt = crate::contour::Polygon::new(outer_ring, hole_rings)
            .expect("checked Polygon::new must accept group_into_polygons output");
        assert_eq!(rebuilt.holes().len(), 1);
    }
}
