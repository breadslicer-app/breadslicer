/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Containment analysis for a layer's worth of closed rings: decide
//! which rings are outer contours and which are holes nested inside
//! them.
//!
//! A ring's facing says which side of it is solid: the slicer measures
//! it from the faces the ring cuts (a validated mesh is consistently
//! oriented). A ring with its solid inside is an outer, one with its
//! solid outside a hole, attached to the smallest outer that contains
//! it. Rings of one shell can overlap without either containing the
//! other, as where a posed mesh runs through itself: both stay outers,
//! and the non-zero raster unites them.
//!
//! A ring whose facing is unknown (zero) falls back to containment
//! parity, which assumes the rings nest cleanly or are disjoint: depth
//! is the number of other rings that contain a representative point of
//! the ring, even depth means outer and odd depth means hole. For
//! cleanly nested rings of an outward-facing shell, facing and parity
//! agree.

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

/// Cluster CCW `rings` into outer/hole groups. `facing[i]` is positive
/// when ring `i`'s solid lies inside it (an outer), negative when it
/// lies outside (a hole), and zero when unknown, which falls back to
/// containment parity (see the module docs). Each hole attaches to the
/// smallest outer whose ring contains it.
///
/// Returns 1 [`RingGroup`] per outer ring, in the order outers appear
/// in `rings`, with hole indices in the order they appear in `rings`;
/// then the holes no outer contains, in order.
pub(crate) fn group_rings(rings: &[ClosedRing], facing: &[f64]) -> (Vec<RingGroup>, Vec<usize>) {
    let n = rings.len();
    debug_assert_eq!(facing.len(), n, "1 facing per ring");
    if n == 0 {
        return (Vec::new(), Vec::new());
    }

    let reps: Vec<Point2D> = rings.iter().map(ring_representative).collect();
    let is_outer: Vec<bool> = (0..n)
        .map(|i| {
            if facing[i] != 0.0 {
                return facing[i] > 0.0;
            }
            let depth = (0..n)
                .filter(|&j| j != i && point_in_ring(reps[i], &rings[j]))
                .count();
            depth.is_multiple_of(2)
        })
        .collect();

    let mut groups: Vec<RingGroup> = Vec::new();
    let mut outer_to_group: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        if is_outer[i] {
            outer_to_group[i] = Some(groups.len());
            groups.push(RingGroup {
                outer_idx: i,
                hole_indices: Vec::new(),
            });
        }
    }
    let mut orphans = Vec::new();
    for i in 0..n {
        if !is_outer[i] {
            let parent = (0..n)
                .filter(|&j| is_outer[j] && point_in_ring(reps[i], &rings[j]))
                .min_by(|&a, &b| {
                    rings[a]
                        .signed_area()
                        .abs()
                        .total_cmp(&rings[b].signed_area().abs())
                });
            match parent.and_then(|p| outer_to_group[p]) {
                Some(g) => groups[g].hole_indices.push(i),
                None => orphans.push(i),
            }
        }
    }
    (groups, orphans)
}

/// Assemble closed CCW rings into outer-CCW + holes-CW polygons by
/// containment parity: [`group_by_facing`] with every facing unknown.
/// Outer rings keep their CCW vertex order; hole rings have their
/// vertex order reversed so they end up CW, matching the [`Polygon`]
/// invariant established at construction by
/// [`Polygon::new_unchecked`].
///
/// Precondition: every input ring has strictly positive signed area
/// (CCW), as produced by the stitcher's `normalise_to_ccw` pass.
/// Rings whose containment chain has no parent at the expected even
/// depth (a case ruled out for cleanly nested input) are silently
/// dropped.
pub fn group_into_polygons(rings: Vec<ClosedRing>) -> Vec<Polygon> {
    let facing = vec![0.0; rings.len()];
    group_by_facing(rings, &facing).0
}

/// Assemble closed CCW rings into outer-CCW + holes-CW polygons, each
/// ring an outer or a hole by its facing (see [`group_rings`]). Returns
/// the polygons, then the holes no outer contains, still CCW.
pub(crate) fn group_by_facing(
    rings: Vec<ClosedRing>,
    facing: &[f64],
) -> (Vec<Polygon>, Vec<ClosedRing>) {
    let (groups, orphans) = group_rings(&rings, facing);
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
    let orphans = orphans
        .into_iter()
        .map(|idx| taken[idx].take().unwrap())
        .collect();
    (polygons, orphans)
}

#[cfg(test)]
mod grouping_tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2D {
        Point2D::new(x, y)
    }

    /// [`group_rings`] by containment parity alone, without orphans.
    fn parity_groups(rings: &[ClosedRing]) -> Vec<RingGroup> {
        group_rings(rings, &vec![0.0; rings.len()]).0
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
        let groups = parity_groups(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn group_rings_single_ring_is_one_outer_with_no_holes() {
        let rings = vec![ccw_square(0.0, 0.0, 1.0)];
        let groups = parity_groups(&rings);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].outer_idx, 0);
        assert!(groups[0].hole_indices.is_empty());
    }

    #[test]
    fn group_rings_two_disjoint_rings_become_two_outers() {
        let rings = vec![ccw_square(0.0, 0.0, 1.0), ccw_square(5.0, 5.0, 1.0)];
        let groups = parity_groups(&rings);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.hole_indices.is_empty()));
        let outer_indices: Vec<_> = groups.iter().map(|g| g.outer_idx).collect();
        assert_eq!(outer_indices, vec![0, 1]);
    }

    #[test]
    fn group_rings_nested_pair_yields_one_outer_with_one_hole() {
        let rings = vec![ccw_square(0.0, 0.0, 2.0), ccw_square(0.0, 0.0, 0.5)];
        let groups = parity_groups(&rings);
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
        let groups = parity_groups(&rings);
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
        let groups = parity_groups(&rings);
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
    fn facing_keeps_overlapping_solids_as_outers() {
        // Two solid squares of one shell that cross: parity makes the
        // second a hole of the first, facing keeps both solid.
        let rings = vec![ccw_square(0.0, 0.0, 1.0), ccw_square(0.5, 0.5, 1.0)];
        assert_eq!(parity_groups(&rings).len(), 1);
        let (groups, orphans) = group_rings(&rings, &[1.0, 1.0]);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.hole_indices.is_empty()));
        assert!(orphans.is_empty());
    }

    #[test]
    fn facing_keeps_a_solid_inside_a_solid_as_an_outer() {
        let rings = vec![ccw_square(0.0, 0.0, 2.0), ccw_square(0.0, 0.0, 0.5)];
        let (groups, _) = group_rings(&rings, &[1.0, 1.0]);
        let outers: Vec<_> = groups.iter().map(|g| g.outer_idx).collect();
        assert_eq!(outers, vec![0, 1]);
    }

    #[test]
    fn a_hole_attaches_to_the_smallest_outer_containing_it() {
        // A big solid, a smaller solid overlapping it, and a hole inside
        // both: the hole belongs to the smaller one.
        let rings = vec![
            ccw_square(0.0, 0.0, 4.0),
            ccw_square(1.0, 0.0, 2.0),
            ccw_square(1.0, 0.0, 0.5),
        ];
        let (groups, orphans) = group_rings(&rings, &[1.0, 1.0, -1.0]);
        assert_eq!(groups[0].hole_indices, Vec::<usize>::new());
        assert_eq!(groups[1].hole_indices, vec![2]);
        assert!(orphans.is_empty());
    }

    #[test]
    fn a_hole_no_outer_contains_is_an_orphan() {
        let rings = vec![ccw_square(0.0, 0.0, 1.0), ccw_square(5.0, 0.0, 1.0)];
        let (groups, orphans) = group_rings(&rings, &[1.0, -1.0]);
        assert_eq!(groups.len(), 1);
        assert_eq!(orphans, vec![1]);
        let (polygons, orphan_rings) = group_by_facing(rings.clone(), &[1.0, -1.0]);
        assert_eq!(polygons.len(), 1);
        assert_eq!(orphan_rings, vec![rings[1].clone()], "orphans stay CCW");
    }

    #[test]
    fn unknown_facing_falls_back_to_parity() {
        let rings = vec![ccw_square(0.0, 0.0, 2.0), ccw_square(0.0, 0.0, 0.5)];
        let (groups, _) = group_rings(&rings, &[1.0, 0.0]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].hole_indices, vec![1]);
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
        let rebuilt = Polygon::new(outer_ring, hole_rings)
            .expect("checked Polygon::new must accept group_into_polygons output");
        assert_eq!(rebuilt.holes().len(), 1);
    }
}
