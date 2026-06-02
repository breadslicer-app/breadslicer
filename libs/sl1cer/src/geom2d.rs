/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! 2D millimetre primitives used by the contour-tier IR.
//!
//! Coordinates are `f64` because the upstream baking pipeline (scene
//! transforms) is already `f64` and the contour math (signed area,
//! plane intersection, scanline X coordinates) wants the wider
//! precision; rounding to integer pixel grids happens at the raster
//! boundary.

use sc_mesh_core::primitives::VertexIndex;

/// 2D point in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point2D {
    pub x: f64,
    pub y: f64,
}

impl Point2D {
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn distance_squared(&self, other: &Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// Linear interpolation: `t = 0` returns `self`, `t = 1` returns
    /// `other`. No clamping; the caller picks the range.
    #[inline]
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
        }
    }
}

/// Unordered identity of a mesh edge, canonicalised to `(min, max)` of
/// its two endpoint indices. Used as the join key when stitching
/// crossing segments back into closed rings: each interior edge of a
/// watertight mesh is incident to exactly two triangles, so the same
/// `EdgeId` appears in exactly two segment endpoints per Z layer and
/// the stitcher pairs them by equality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeId {
    lo: VertexIndex,
    hi: VertexIndex,
}

impl EdgeId {
    /// Build an `EdgeId` from an unordered vertex-index pair. The
    /// invariant `lo <= hi` is established here, so callers can rely
    /// on it (and on `PartialEq` / `Hash`) without re-canonicalising.
    #[inline]
    pub fn new(a: VertexIndex, b: VertexIndex) -> Self {
        if a <= b {
            Self { lo: a, hi: b }
        } else {
            Self { lo: b, hi: a }
        }
    }

    #[inline]
    pub fn lo(self) -> VertexIndex {
        self.lo
    }

    #[inline]
    pub fn hi(self) -> VertexIndex {
        self.hi
    }
}

/// 2D line segment carrying the identity of the mesh edge each
/// endpoint sits on. The stitcher matches `a_edge` / `b_edge` across
/// segments from neighbouring triangles to chain them into closed
/// rings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: Point2D,
    pub a_edge: EdgeId,
    pub b: Point2D,
    pub b_edge: EdgeId,
}

impl Segment {
    #[inline]
    pub const fn new(a: Point2D, a_edge: EdgeId, b: Point2D, b_edge: EdgeId) -> Self {
        Self {
            a,
            a_edge,
            b,
            b_edge,
        }
    }
}

#[cfg(test)]
mod geom2d_tests {
    use super::*;

    fn vi(i: u32) -> VertexIndex {
        VertexIndex::from(i)
    }

    #[test]
    fn point2d_distance_squared_is_symmetric_and_non_negative() {
        let p = Point2D::new(1.0, 2.0);
        let q = Point2D::new(4.0, 6.0);
        assert_eq!(p.distance_squared(&q), 25.0);
        assert_eq!(p.distance_squared(&q), q.distance_squared(&p));
        assert_eq!(p.distance_squared(&p), 0.0);
    }

    #[test]
    fn point2d_lerp_hits_endpoints_and_midpoint() {
        let a = Point2D::new(0.0, 0.0);
        let b = Point2D::new(10.0, 20.0);
        assert_eq!(a.lerp(&b, 0.0), a);
        assert_eq!(a.lerp(&b, 1.0), b);
        assert_eq!(a.lerp(&b, 0.5), Point2D::new(5.0, 10.0));
    }

    #[test]
    fn edge_id_canonicalises_to_min_max() {
        let forward = EdgeId::new(vi(3), vi(7));
        let reverse = EdgeId::new(vi(7), vi(3));
        assert_eq!(forward, reverse);
        assert_eq!(forward.lo(), vi(3));
        assert_eq!(forward.hi(), vi(7));
    }

    #[test]
    fn edge_id_equal_endpoints_collapses_cleanly() {
        // Not produced by the slicer (zero-length edges fail face
        // validation upstream), but the type must still behave: both
        // accessors return the same index.
        let degenerate = EdgeId::new(vi(5), vi(5));
        assert_eq!(degenerate.lo(), vi(5));
        assert_eq!(degenerate.hi(), vi(5));
    }

    #[test]
    fn edge_id_hash_keys_match_after_canonicalisation() {
        use std::collections::HashMap;
        let mut map: HashMap<EdgeId, &'static str> = HashMap::new();
        map.insert(EdgeId::new(vi(1), vi(4)), "first");
        // Inserting the swapped pair must overwrite, not coexist.
        map.insert(EdgeId::new(vi(4), vi(1)), "second");
        assert_eq!(map.len(), 1);
        assert_eq!(map[&EdgeId::new(vi(1), vi(4))], "second");
    }

    #[test]
    fn segment_stores_endpoints_and_edge_ids() {
        let a = Point2D::new(0.0, 0.0);
        let b = Point2D::new(1.0, 1.0);
        let ea = EdgeId::new(vi(0), vi(1));
        let eb = EdgeId::new(vi(1), vi(2));
        let seg = Segment::new(a, ea, b, eb);
        assert_eq!(seg.a, a);
        assert_eq!(seg.b, b);
        assert_eq!(seg.a_edge, ea);
        assert_eq!(seg.b_edge, eb);
    }
}
