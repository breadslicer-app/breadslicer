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

use sc_mesh_core::primitives::{FaceIndex, VertexIndex};

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
        dx.mul_add(dx, dy * dy)
    }

    /// Linear interpolation: `t = 0` returns `self`, `t = 1` returns
    /// `other`. No clamping; the caller picks the range.
    #[inline]
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        Self {
            x: (other.x - self.x).mul_add(t, self.x),
            y: (other.y - self.y).mul_add(t, self.y),
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

/// Tag attached to one endpoint of a [`Segment`]. The stitcher mates
/// endpoints by equal `EndpointTag`.
///
/// - [`EndpointTag::Edge`] is used when the endpoint sits on the
///   interior of a mesh edge (the common straddling-triangle case).
///   On a manifold mesh, each interior mesh edge crossed by the
///   slicing plane contributes exactly two endpoints with the same
///   `Edge` tag, one per incident triangle, so pairing by tag is
///   sufficient.
/// - [`EndpointTag::Vertex`] is used when the endpoint sits exactly on
///   a mesh vertex (the through-vertex case, currently emitted by
///   the AOB triangle slice). A given on-plane mesh vertex can act as
///   a contour hub for `2k` such endpoints (`k` contour passes
///   through that vertex); the stitcher resolves the pairing using
///   the mesh's cyclic 1-ring of incident triangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EndpointTag {
    Edge(EdgeId),
    Vertex(VertexIndex),
}

/// 2D line segment carrying the identity of the mesh feature each
/// endpoint sits on. The stitcher matches `a_tag` / `b_tag` across
/// segments from neighbouring triangles to chain them into closed
/// rings; `source_face` records which mesh triangle emitted the
/// segment, which the stitcher uses to locate vertex-tagged
/// endpoints in the mesh's 1-ring cyclic order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: Point2D,
    pub a_tag: EndpointTag,
    pub b: Point2D,
    pub b_tag: EndpointTag,
    pub source_face: FaceIndex,
}

impl Segment {
    #[inline]
    pub const fn new(
        a: Point2D,
        a_tag: EndpointTag,
        b: Point2D,
        b_tag: EndpointTag,
        source_face: FaceIndex,
    ) -> Self {
        Self {
            a,
            a_tag,
            b,
            b_tag,
            source_face,
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
    fn segment_stores_endpoints_endpoint_tags_and_source_face() {
        let a = Point2D::new(0.0, 0.0);
        let b = Point2D::new(1.0, 1.0);
        let ta = EndpointTag::Edge(EdgeId::new(vi(0), vi(1)));
        let tb = EndpointTag::Edge(EdgeId::new(vi(1), vi(2)));
        let face = FaceIndex::from(42_usize);
        let seg = Segment::new(a, ta, b, tb, face);
        assert_eq!(seg.a, a);
        assert_eq!(seg.b, b);
        assert_eq!(seg.a_tag, ta);
        assert_eq!(seg.b_tag, tb);
        assert_eq!(seg.source_face, face);
    }

    #[test]
    fn endpoint_tag_equality_distinguishes_edge_and_vertex_kinds() {
        let edge_tag = EndpointTag::Edge(EdgeId::new(vi(0), vi(1)));
        let vertex_tag = EndpointTag::Vertex(vi(0));
        assert_ne!(edge_tag, vertex_tag);
    }

    #[test]
    fn endpoint_tag_vertex_compares_by_vertex_index() {
        assert_eq!(EndpointTag::Vertex(vi(7)), EndpointTag::Vertex(vi(7)));
        assert_ne!(EndpointTag::Vertex(vi(7)), EndpointTag::Vertex(vi(8)));
    }

    #[test]
    fn endpoint_tag_can_serve_as_hashmap_key() {
        use std::collections::HashMap;
        let mut map: HashMap<EndpointTag, &'static str> = HashMap::new();
        map.insert(EndpointTag::Edge(EdgeId::new(vi(1), vi(2))), "edge");
        map.insert(EndpointTag::Vertex(vi(1)), "vertex");
        assert_eq!(map.len(), 2);
        assert_eq!(map[&EndpointTag::Vertex(vi(1))], "vertex");
    }
}
