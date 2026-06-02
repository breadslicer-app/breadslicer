/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Layer-stitch step: glue a layer of crossing segments into closed
//! polygon rings.
//!
//! On a watertight manifold mesh, every interior edge crossed by a
//! slicing plane contributes exactly two segment endpoints to the
//! layer's segment list (one per incident triangle). Pairing the
//! endpoints by [`EdgeId`] and following the chain until it returns
//! to the starting segment produces one closed ring per topological
//! contour component.
//!
//! Endpoints whose `EdgeId` does not appear in exactly two segments
//! raise [`SliceError::NonManifoldDuringStitching`] as a runtime
//! safety net for inputs that slipped the upstream manifold check.

use std::collections::HashMap;

use crate::contour::ClosedRing;
use crate::error::SliceError;
use crate::geom2d::{EdgeId, Point2D, Segment};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    A,
    B,
}

impl Side {
    fn opposite(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

fn point_at(seg: &Segment, side: Side) -> Point2D {
    match side {
        Side::A => seg.a,
        Side::B => seg.b,
    }
}

fn edge_at(seg: &Segment, side: Side) -> EdgeId {
    match side {
        Side::A => seg.a_edge,
        Side::B => seg.b_edge,
    }
}

/// Stitch one layer of crossing segments into closed rings. `z_mm`
/// is carried through to the error variant for diagnostics. Returns
/// one [`ClosedRing`] per chain, in arbitrary order; the vertex
/// orientation of each ring is not normalised here (that's the
/// winding-canonicalisation pass).
pub fn stitch_layer(segments: &[Segment], z_mm: f64) -> Result<Vec<ClosedRing>, SliceError> {
    let mut by_edge: HashMap<EdgeId, Vec<(usize, Side)>> = HashMap::new();
    for (i, seg) in segments.iter().enumerate() {
        by_edge.entry(seg.a_edge).or_default().push((i, Side::A));
        by_edge.entry(seg.b_edge).or_default().push((i, Side::B));
    }

    let mut dangling = 0usize;
    for entries in by_edge.values() {
        if entries.len() != 2 {
            dangling += entries.len();
        }
    }
    if dangling > 0 {
        return Err(SliceError::NonManifoldDuringStitching {
            z_mm,
            dangling_endpoints: dangling,
        });
    }

    let mut visited = vec![false; segments.len()];
    let mut rings = Vec::new();

    for start in 0..segments.len() {
        if visited[start] {
            continue;
        }
        let mut ring_points = Vec::new();
        let mut current_idx = start;
        let mut entry_side = Side::A;
        loop {
            if visited[current_idx] {
                break;
            }
            visited[current_idx] = true;
            let seg = &segments[current_idx];
            ring_points.push(point_at(seg, entry_side));
            let exit_side = entry_side.opposite();
            let exit_edge = edge_at(seg, exit_side);
            let pair = &by_edge[&exit_edge];
            let (next_idx, next_entry_side) = if pair[0] == (current_idx, exit_side) {
                pair[1]
            } else {
                pair[0]
            };
            current_idx = next_idx;
            entry_side = next_entry_side;
        }
        rings.push(ClosedRing::new(ring_points));
    }

    Ok(rings)
}

/// Reverse the vertex order of any clockwise ring so every ring's
/// vertices end up in counter-clockwise order (strictly positive
/// signed area). Rings with zero signed area (degenerate) are left
/// alone.
///
/// This is a pure orientation pass. Outer-vs-hole classification is
/// the grouping step's job and assumes CCW input, so downstream code
/// can read "CCW means traversal direction" from any ring this
/// function has touched.
pub fn normalise_to_ccw(rings: &mut [ClosedRing]) {
    for ring in rings.iter_mut() {
        if ring.signed_area() < 0.0 {
            ring.reverse();
        }
    }
}

#[cfg(test)]
mod stitch_tests {
    use sc_mesh_core::primitives::VertexIndex;

    use super::*;

    fn vi(i: u32) -> VertexIndex {
        VertexIndex::from(i)
    }

    fn p(x: f64, y: f64) -> Point2D {
        Point2D::new(x, y)
    }

    fn seg(a: (f64, f64), a_edge: EdgeId, b: (f64, f64), b_edge: EdgeId) -> Segment {
        Segment::new(p(a.0, a.1), a_edge, p(b.0, b.1), b_edge)
    }

    #[test]
    fn empty_input_returns_no_rings() {
        let rings = stitch_layer(&[], 0.0).unwrap();
        assert!(rings.is_empty());
    }

    #[test]
    fn four_segments_forming_a_square_yield_one_ring_with_four_vertices() {
        let e0 = EdgeId::new(vi(0), vi(1));
        let e1 = EdgeId::new(vi(1), vi(2));
        let e2 = EdgeId::new(vi(2), vi(3));
        let e3 = EdgeId::new(vi(3), vi(0));
        let segments = vec![
            seg((0.0, 0.0), e0, (1.0, 0.0), e1),
            seg((1.0, 0.0), e1, (1.0, 1.0), e2),
            seg((1.0, 1.0), e2, (0.0, 1.0), e3),
            seg((0.0, 1.0), e3, (0.0, 0.0), e0),
        ];
        let rings = stitch_layer(&segments, 0.0).unwrap();
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].vertices().len(), 4);
    }

    #[test]
    fn triangle_segments_yield_one_ring_with_three_vertices() {
        let e0 = EdgeId::new(vi(0), vi(1));
        let e1 = EdgeId::new(vi(1), vi(2));
        let e2 = EdgeId::new(vi(2), vi(0));
        let segments = vec![
            seg((0.0, 0.0), e0, (1.0, 0.0), e1),
            seg((1.0, 0.0), e1, (0.5, 1.0), e2),
            seg((0.5, 1.0), e2, (0.0, 0.0), e0),
        ];
        let rings = stitch_layer(&segments, 0.0).unwrap();
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].vertices().len(), 3);
    }

    #[test]
    fn two_disjoint_squares_yield_two_rings() {
        let e0 = EdgeId::new(vi(0), vi(1));
        let e1 = EdgeId::new(vi(1), vi(2));
        let e2 = EdgeId::new(vi(2), vi(3));
        let e3 = EdgeId::new(vi(3), vi(0));
        let f0 = EdgeId::new(vi(4), vi(5));
        let f1 = EdgeId::new(vi(5), vi(6));
        let f2 = EdgeId::new(vi(6), vi(7));
        let f3 = EdgeId::new(vi(7), vi(4));
        let segments = vec![
            seg((0.0, 0.0), e0, (1.0, 0.0), e1),
            seg((1.0, 0.0), e1, (1.0, 1.0), e2),
            seg((1.0, 1.0), e2, (0.0, 1.0), e3),
            seg((0.0, 1.0), e3, (0.0, 0.0), e0),
            seg((2.0, 2.0), f0, (3.0, 2.0), f1),
            seg((3.0, 2.0), f1, (3.0, 3.0), f2),
            seg((3.0, 3.0), f2, (2.0, 3.0), f3),
            seg((2.0, 3.0), f3, (2.0, 2.0), f0),
        ];
        let rings = stitch_layer(&segments, 0.0).unwrap();
        assert_eq!(rings.len(), 2);
        assert_eq!(rings[0].vertices().len(), 4);
        assert_eq!(rings[1].vertices().len(), 4);
    }

    #[test]
    fn normalise_to_ccw_flips_clockwise_rings_and_keeps_ccw_rings() {
        // CCW unit square at origin.
        let ccw = ClosedRing::new(vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)]);
        // CW unit square at origin (same set of corners, reverse order).
        let cw = ClosedRing::new(vec![p(0.0, 0.0), p(0.0, 1.0), p(1.0, 1.0), p(1.0, 0.0)]);
        let ccw_original: Vec<_> = ccw.vertices().to_vec();
        let mut rings = vec![ccw, cw];
        normalise_to_ccw(&mut rings);
        for ring in &rings {
            assert!(
                ring.signed_area() > 0.0,
                "expected CCW, got area {}",
                ring.signed_area(),
            );
        }
        // The already-CCW ring is untouched in vertex order.
        assert_eq!(rings[0].vertices(), ccw_original.as_slice());
    }

    #[test]
    fn normalise_to_ccw_leaves_zero_area_rings_alone() {
        // Three collinear vertices bake to zero signed area.
        let degenerate = ClosedRing::new(vec![p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0)]);
        let original: Vec<_> = degenerate.vertices().to_vec();
        let mut rings = vec![degenerate];
        normalise_to_ccw(&mut rings);
        assert_eq!(rings[0].vertices(), original.as_slice());
    }

    #[test]
    fn square_missing_one_segment_returns_non_manifold_during_stitching() {
        let e0 = EdgeId::new(vi(0), vi(1));
        let e1 = EdgeId::new(vi(1), vi(2));
        let e2 = EdgeId::new(vi(2), vi(3));
        let e3 = EdgeId::new(vi(3), vi(0));
        // Three sides of the square; e0 and e3 each appear in only
        // one segment endpoint instead of two.
        let segments = vec![
            seg((0.0, 0.0), e0, (1.0, 0.0), e1),
            seg((1.0, 0.0), e1, (1.0, 1.0), e2),
            seg((1.0, 1.0), e2, (0.0, 1.0), e3),
        ];
        let err = stitch_layer(&segments, 0.42).unwrap_err();
        match err {
            SliceError::NonManifoldDuringStitching {
                z_mm,
                dangling_endpoints,
            } => {
                assert_eq!(z_mm, 0.42);
                assert!(
                    dangling_endpoints >= 2,
                    "expected at least 2 dangling endpoints, got {dangling_endpoints}",
                );
            }
            other => panic!("expected NonManifoldDuringStitching, got {other:?}"),
        }
    }
}
