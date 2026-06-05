/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Triangle versus Z plane intersection.
//!
//! Pure function, deterministic for fixed inputs. The hot kernel of
//! the slicer's vector pipeline: a layer of crossing segments is built
//! by calling [`intersect_triangle_z`] once per face of the mesh.

use sc_mesh_core::primitives::{FaceIndex, Vertex, VertexIndex};

use crate::geom2d::{EdgeId, EndpointTag, Point2D, Segment};

/// Sub-micron tolerance (in mm) for the on-plane vertex
/// classification. Sized two orders of magnitude below typical
/// resin-printer XY pixel size and well below f32 vertex-coordinate
/// precision for build-volume-scale values.
pub const Z_CLASSIFY_EPS_MM: f64 = 1e-6;

/// Position of a vertex relative to a horizontal slicing plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexClass {
    Above,
    On,
    Below,
}

/// Classify a vertex's Z relative to the plane. `eps_mm` is the
/// inclusive band around the plane that counts as `On`.
#[inline]
pub fn classify_vertex_z(vertex_z_mm: f32, z_plane_mm: f64, eps_mm: f64) -> VertexClass {
    let d = f64::from(vertex_z_mm) - z_plane_mm;
    if d > eps_mm {
        VertexClass::Above
    } else if d < -eps_mm {
        VertexClass::Below
    } else {
        VertexClass::On
    }
}

/// Result of intersecting one triangle with one horizontal plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriangleSlice {
    /// Nothing to emit. Covers all-above, all-below, and the
    /// "skim through one On vertex" cases (AAO, BBO).
    Empty,
    /// All three vertices lie on the plane. Recorded as its own
    /// variant so the caller can choose to skip it without confusing
    /// the case with [`TriangleSlice::Empty`].
    Coplanar,
    /// Exactly two vertices on the plane: the triangle edge between
    /// them lies on the plane. Both `Segment` endpoints carry a
    /// [`EndpointTag::Vertex`] tag (one per on-vertex). `edge_id`
    /// identifies the on-edge so the slicer can resolve, via the
    /// mesh's edge-to-faces index, whether the on-edge actually
    /// belongs to the contour: it does iff the other triangle of the
    /// edge has its third vertex on the opposite side of the plane.
    Edge { edge_id: EdgeId, segment: Segment },
    /// The common straddling case: the plane cuts exactly two
    /// triangle edges, producing one crossing segment whose endpoints
    /// carry the originating mesh-edge `EdgeId`.
    Crossing { segment: Segment },
    /// Exactly one vertex on the plane, one above, one below (the
    /// AOB case). The plane cuts the triangle from the on-vertex to
    /// a point on the opposite mesh edge. The emitted segment has
    /// one [`EndpointTag::Vertex`] endpoint at the on-vertex and one
    /// [`EndpointTag::Edge`] endpoint on the opposite edge; the
    /// stitcher pairs vertex-tagged endpoints around a hub using the
    /// mesh's 1-ring cyclic order.
    ThroughVertex {
        vertex_idx: VertexIndex,
        segment: Segment,
    },
}

/// Intersect a single triangle with a horizontal plane at `z_plane_mm`.
///
/// `vertices` and `classes` are in the same order; `edge_ids[i]` is
/// the canonical id of the triangle edge between vertex `i` and
/// vertex `(i + 1) % 3`. `face_idx` is the index of the triangle in
/// the source mesh; it is recorded on any emitted [`Segment`] so the
/// stitcher can locate vertex-tagged endpoints in the mesh's 1-ring
/// cyclic order.
///
/// The endpoint interpolation uses the signed-distance ratio in f64
/// so precision stays predictable across edges of very different
/// lengths.
pub fn intersect_triangle_z(
    vertices: [Vertex; 3],
    classes: [VertexClass; 3],
    z_plane_mm: f64,
    edge_ids: [EdgeId; 3],
    face_idx: FaceIndex,
) -> TriangleSlice {
    let mut on_count = 0usize;
    let mut above_count = 0usize;
    let mut below_count = 0usize;
    for c in classes {
        match c {
            VertexClass::On => on_count += 1,
            VertexClass::Above => above_count += 1,
            VertexClass::Below => below_count += 1,
        }
    }

    if on_count == 3 {
        return TriangleSlice::Coplanar;
    }

    if on_count == 2 {
        let mut on_indices = [usize::MAX, usize::MAX];
        let mut written = 0;
        for (i, c) in classes.iter().enumerate() {
            if matches!(c, VertexClass::On) {
                on_indices[written] = i;
                written += 1;
            }
        }
        let edge_pos = triangle_edge_index(on_indices[0], on_indices[1]);
        let edge_id = edge_ids[edge_pos];
        let a = project_xy(vertices[on_indices[0]]);
        let b = project_xy(vertices[on_indices[1]]);
        let v_a = slot_vertex(on_indices[0], &edge_ids);
        let v_b = slot_vertex(on_indices[1], &edge_ids);
        return TriangleSlice::Edge {
            edge_id,
            segment: Segment::new(
                a,
                EndpointTag::Vertex(v_a),
                b,
                EndpointTag::Vertex(v_b),
                face_idx,
            ),
        };
    }

    if on_count == 1 && above_count == 1 && below_count == 1 {
        let on_idx = first_index_with(&classes, VertexClass::On);
        let above_idx = first_index_with(&classes, VertexClass::Above);
        let below_idx = first_index_with(&classes, VertexClass::Below);

        let edge_ab_pos = triangle_edge_index(above_idx, below_idx);
        let edge_va_pos = triangle_edge_index(on_idx, above_idx);
        let edge_vb_pos = triangle_edge_index(on_idx, below_idx);

        let vertex_idx = common_vertex(edge_ids[edge_va_pos], edge_ids[edge_vb_pos]);

        let v_2d = project_xy(vertices[on_idx]);
        let p_2d = interpolate_to_plane(vertices[above_idx], vertices[below_idx], z_plane_mm);

        return TriangleSlice::ThroughVertex {
            vertex_idx,
            segment: Segment::new(
                v_2d,
                EndpointTag::Vertex(vertex_idx),
                p_2d,
                EndpointTag::Edge(edge_ids[edge_ab_pos]),
                face_idx,
            ),
        };
    }

    if above_count == 0 || below_count == 0 || on_count == 1 {
        return TriangleSlice::Empty;
    }

    let lone_idx = if above_count == 1 {
        first_index_with(&classes, VertexClass::Above)
    } else {
        first_index_with(&classes, VertexClass::Below)
    };

    let other_indices: [usize; 2] = match lone_idx {
        0 => [1, 2],
        1 => [0, 2],
        2 => [0, 1],
        _ => unreachable!("triangle vertex index out of range"),
    };

    let edge_pos_a = triangle_edge_index(lone_idx, other_indices[0]);
    let edge_pos_b = triangle_edge_index(lone_idx, other_indices[1]);

    let pt_a = interpolate_to_plane(vertices[lone_idx], vertices[other_indices[0]], z_plane_mm);
    let pt_b = interpolate_to_plane(vertices[lone_idx], vertices[other_indices[1]], z_plane_mm);

    TriangleSlice::Crossing {
        segment: Segment::new(
            pt_a,
            EndpointTag::Edge(edge_ids[edge_pos_a]),
            pt_b,
            EndpointTag::Edge(edge_ids[edge_pos_b]),
            face_idx,
        ),
    }
}

/// Return the [`VertexIndex`] at triangle `slot` (0, 1, or 2). For
/// slot `k`, the vertex is the common endpoint of the two triangle
/// edges that touch slot `k`: edges `(k, k+1)` and `(k, k-1)` modulo 3.
#[inline]
fn slot_vertex(slot: usize, edge_ids: &[EdgeId; 3]) -> VertexIndex {
    match slot {
        0 => common_vertex(edge_ids[0], edge_ids[2]),
        1 => common_vertex(edge_ids[0], edge_ids[1]),
        2 => common_vertex(edge_ids[1], edge_ids[2]),
        _ => unreachable!("triangle slot index out of range: {slot}"),
    }
}

/// Return the [`VertexIndex`] shared by two edges. Used in the AOB
/// branch to recover the on-vertex's index from the two mesh edges
/// touching it (the third edge of the triangle is the above-below
/// edge that the crossing point sits on).
#[inline]
fn common_vertex(e1: EdgeId, e2: EdgeId) -> VertexIndex {
    if e1.lo() == e2.lo() || e1.lo() == e2.hi() {
        e1.lo()
    } else {
        debug_assert!(
            e1.hi() == e2.lo() || e1.hi() == e2.hi(),
            "common_vertex: edges {e1:?} and {e2:?} do not share a vertex",
        );
        e1.hi()
    }
}

#[inline]
fn project_xy(v: Vertex) -> Point2D {
    Point2D::new(f64::from(v[0]), f64::from(v[1]))
}

fn interpolate_to_plane(va: Vertex, vb: Vertex, z_plane_mm: f64) -> Point2D {
    let xa = f64::from(va[0]);
    let ya = f64::from(va[1]);
    let za = f64::from(va[2]);
    let xb = f64::from(vb[0]);
    let yb = f64::from(vb[1]);
    let zb = f64::from(vb[2]);
    let t = (z_plane_mm - za) / (zb - za);
    Point2D::new((xb - xa).mul_add(t, xa), (yb - ya).mul_add(t, ya))
}

fn triangle_edge_index(a: usize, b: usize) -> usize {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    match (lo, hi) {
        (0, 1) => 0,
        (1, 2) => 1,
        (0, 2) => 2,
        _ => unreachable!("invalid triangle vertex index pair: ({a}, {b})"),
    }
}

fn first_index_with(classes: &[VertexClass; 3], target: VertexClass) -> usize {
    for (i, c) in classes.iter().enumerate() {
        if *c == target {
            return i;
        }
    }
    unreachable!("first_index_with: no vertex with the requested class");
}

#[cfg(test)]
mod plane_intersect_tests {
    use proptest::prelude::*;

    use super::*;

    fn vi(i: u32) -> VertexIndex {
        VertexIndex::from(i)
    }

    /// Test triangles use vertex indices 0, 1, 2 in slot order.
    fn unit_edges() -> [EdgeId; 3] {
        [
            EdgeId::new(vi(0), vi(1)),
            EdgeId::new(vi(1), vi(2)),
            EdgeId::new(vi(2), vi(0)),
        ]
    }

    fn unit_face_idx() -> FaceIndex {
        FaceIndex::from(0_usize)
    }

    fn classify_all(vs: [Vertex; 3], z_plane: f64) -> [VertexClass; 3] {
        [
            classify_vertex_z(vs[0][2], z_plane, Z_CLASSIFY_EPS_MM),
            classify_vertex_z(vs[1][2], z_plane, Z_CLASSIFY_EPS_MM),
            classify_vertex_z(vs[2][2], z_plane, Z_CLASSIFY_EPS_MM),
        ]
    }

    fn run(vs: [Vertex; 3], z_plane: f64) -> TriangleSlice {
        let classes = classify_all(vs, z_plane);
        intersect_triangle_z(vs, classes, z_plane, unit_edges(), unit_face_idx())
    }

    #[test]
    fn classify_vertex_z_above_below_on_and_band_edges() {
        let z_plane = 0.0;
        let eps = 1e-6;
        assert_eq!(classify_vertex_z(1.0, z_plane, eps), VertexClass::Above);
        assert_eq!(classify_vertex_z(-1.0, z_plane, eps), VertexClass::Below);
        assert_eq!(classify_vertex_z(0.0, z_plane, eps), VertexClass::On);
        // Within band counts as On.
        assert_eq!(classify_vertex_z(5e-7, z_plane, eps), VertexClass::On);
        assert_eq!(classify_vertex_z(-5e-7, z_plane, eps), VertexClass::On);
        // Outside band classifies away.
        assert_eq!(classify_vertex_z(2e-6, z_plane, eps), VertexClass::Above);
        assert_eq!(classify_vertex_z(-2e-6, z_plane, eps), VertexClass::Below);
    }

    #[test]
    fn triangle_entirely_above_returns_empty() {
        let vs = [
            Vertex::new([0.0, 0.0, 1.0]),
            Vertex::new([1.0, 0.0, 1.0]),
            Vertex::new([0.0, 1.0, 2.0]),
        ];
        assert_eq!(run(vs, 0.0), TriangleSlice::Empty);
    }

    #[test]
    fn triangle_entirely_below_returns_empty() {
        let vs = [
            Vertex::new([0.0, 0.0, -1.0]),
            Vertex::new([1.0, 0.0, -1.0]),
            Vertex::new([0.0, 1.0, -2.0]),
        ];
        assert_eq!(run(vs, 0.0), TriangleSlice::Empty);
    }

    #[test]
    fn triangle_straddling_returns_crossing_segment_on_plane() {
        // V0 below at origin, V1 and V2 above. Crossings sit at the
        // midpoints of edges (V0, V1) and (V2, V0).
        let vs = [
            Vertex::new([0.0, 0.0, -1.0]),
            Vertex::new([1.0, 0.0, 1.0]),
            Vertex::new([0.0, 1.0, 1.0]),
        ];
        let result = run(vs, 0.0);
        let TriangleSlice::Crossing { segment } = result else {
            panic!("expected Crossing, got {result:?}");
        };
        assert!((segment.a.x - 0.5).abs() < 1e-9);
        assert!((segment.a.y - 0.0).abs() < 1e-9);
        assert!((segment.b.x - 0.0).abs() < 1e-9);
        assert!((segment.b.y - 0.5).abs() < 1e-9);
        assert_eq!(segment.a_tag, EndpointTag::Edge(EdgeId::new(vi(0), vi(1))));
        assert_eq!(segment.b_tag, EndpointTag::Edge(EdgeId::new(vi(2), vi(0))));
    }

    #[test]
    fn triangle_coplanar_returns_coplanar() {
        let vs = [
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ];
        assert_eq!(run(vs, 0.0), TriangleSlice::Coplanar);
    }

    #[test]
    fn triangle_skim_at_single_on_vertex_above_pair_returns_empty() {
        // AAO: two above, one On.
        let vs = [
            Vertex::new([0.0, 0.0, 1.0]),
            Vertex::new([1.0, 0.0, 1.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ];
        assert_eq!(run(vs, 0.0), TriangleSlice::Empty);
    }

    #[test]
    fn triangle_skim_at_single_on_vertex_below_pair_returns_empty() {
        // BBO: two below, one On.
        let vs = [
            Vertex::new([0.0, 0.0, -1.0]),
            Vertex::new([1.0, 0.0, -1.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ];
        assert_eq!(run(vs, 0.0), TriangleSlice::Empty);
    }

    #[test]
    fn triangle_aob_through_on_vertex_v1_emits_through_vertex_segment() {
        // AOB with V1 (slot index 1) on the plane: V0 above, V1 on, V2 below.
        // The segment runs from V1 (projected) to the crossing on edge V0-V2.
        let vs = [
            Vertex::new([0.0, 0.0, 1.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, -1.0]),
        ];
        let result = run(vs, 0.0);
        let TriangleSlice::ThroughVertex {
            vertex_idx,
            segment,
        } = result
        else {
            panic!("expected ThroughVertex, got {result:?}");
        };
        // unit_edges() ties slot index k to vertex index k, so the on-vertex
        // at slot 1 is VertexIndex 1.
        assert_eq!(vertex_idx, vi(1));
        // Endpoint A sits exactly on V1's XY projection (1.0, 0.0).
        assert!((segment.a.x - 1.0).abs() < 1e-9);
        assert!((segment.a.y - 0.0).abs() < 1e-9);
        assert_eq!(segment.a_tag, EndpointTag::Vertex(vi(1)));
        // Endpoint B sits at the midpoint of edge V0-V2 (a 1:-1 z split).
        assert!((segment.b.x - 0.0).abs() < 1e-9);
        assert!((segment.b.y - 0.5).abs() < 1e-9);
        assert_eq!(segment.b_tag, EndpointTag::Edge(EdgeId::new(vi(2), vi(0))));
        assert_eq!(segment.source_face, unit_face_idx());
    }

    #[test]
    fn triangle_aob_through_on_vertex_v0_emits_through_vertex_segment() {
        // AOB with V0 (slot 0) on the plane: V0 on, V1 above, V2 below.
        let vs = [
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 1.0]),
            Vertex::new([0.0, 1.0, -1.0]),
        ];
        let result = run(vs, 0.0);
        let TriangleSlice::ThroughVertex {
            vertex_idx,
            segment,
        } = result
        else {
            panic!("expected ThroughVertex, got {result:?}");
        };
        assert_eq!(vertex_idx, vi(0));
        assert_eq!(segment.a_tag, EndpointTag::Vertex(vi(0)));
        assert_eq!(segment.b_tag, EndpointTag::Edge(EdgeId::new(vi(1), vi(2))));
    }

    #[test]
    fn triangle_aob_through_on_vertex_v2_emits_through_vertex_segment() {
        // AOB with V2 (slot 2) on the plane: V0 below, V1 above, V2 on.
        let vs = [
            Vertex::new([0.0, 0.0, -1.0]),
            Vertex::new([1.0, 0.0, 1.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ];
        let result = run(vs, 0.0);
        let TriangleSlice::ThroughVertex {
            vertex_idx,
            segment,
        } = result
        else {
            panic!("expected ThroughVertex, got {result:?}");
        };
        assert_eq!(vertex_idx, vi(2));
        assert_eq!(segment.a_tag, EndpointTag::Vertex(vi(2)));
        assert_eq!(segment.b_tag, EndpointTag::Edge(EdgeId::new(vi(0), vi(1))));
    }

    #[test]
    fn triangle_with_one_edge_on_plane_above_returns_edge_with_vertex_tags() {
        // AOO: V0 above, V1 and V2 on plane. Edge between V1 and V2
        // lies on the plane, edge index 1. The emitted segment has
        // both endpoints tagged with the respective on-vertex.
        let vs = [
            Vertex::new([0.0, 0.0, 1.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ];
        let result = run(vs, 0.0);
        let TriangleSlice::Edge { edge_id, segment } = result else {
            panic!("expected Edge, got {result:?}");
        };
        assert_eq!(edge_id, EdgeId::new(vi(1), vi(2)));
        assert_eq!(segment.a_tag, EndpointTag::Vertex(vi(1)));
        assert_eq!(segment.b_tag, EndpointTag::Vertex(vi(2)));
        assert!((segment.a.x - 1.0).abs() < 1e-9);
        assert!((segment.a.y - 0.0).abs() < 1e-9);
        assert!((segment.b.x - 0.0).abs() < 1e-9);
        assert!((segment.b.y - 1.0).abs() < 1e-9);
    }

    #[test]
    fn triangle_with_one_edge_on_plane_below_returns_edge_with_vertex_tags() {
        // BOO: V0 below, V1 and V2 on plane.
        let vs = [
            Vertex::new([0.0, 0.0, -1.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
        ];
        let result = run(vs, 0.0);
        let TriangleSlice::Edge { edge_id, segment } = result else {
            panic!("expected Edge, got {result:?}");
        };
        assert_eq!(edge_id, EdgeId::new(vi(1), vi(2)));
        assert_eq!(segment.a_tag, EndpointTag::Vertex(vi(1)));
        assert_eq!(segment.b_tag, EndpointTag::Vertex(vi(2)));
    }

    proptest! {
        /// Random straddling triangle (V0 below, V1 and V2 above) at
        /// the Z = 0 plane always produces a Crossing whose endpoints:
        /// (a) sit on the plane up to the recovered-z epsilon, and
        /// (b) come from distinct mesh edges.
        #[test]
        fn straddling_triangle_returns_crossing_with_endpoints_on_plane(
            x0 in -10.0f32..10.0,
            y0 in -10.0f32..10.0,
            x1 in -10.0f32..10.0,
            y1 in -10.0f32..10.0,
            x2 in -10.0f32..10.0,
            y2 in -10.0f32..10.0,
            z0 in -10.0f32..-0.1,
            z1 in 0.1f32..10.0,
            z2 in 0.1f32..10.0,
        ) {
            let vs = [
                Vertex::new([x0, y0, z0]),
                Vertex::new([x1, y1, z1]),
                Vertex::new([x2, y2, z2]),
            ];
            let result = run(vs, 0.0);
            let TriangleSlice::Crossing { segment } = result else {
                prop_assert!(false, "expected Crossing, got {result:?}");
                return Ok(());
            };
            prop_assert_ne!(segment.a_tag, segment.b_tag);

            for (endpoint, tag) in [
                (segment.a, segment.a_tag),
                (segment.b, segment.b_tag),
            ] {
                let EndpointTag::Edge(edge_id) = tag else {
                    prop_assert!(false, "Crossing endpoint must carry an Edge tag, got {tag:?}");
                    return Ok(());
                };
                // EdgeIds were minted from indices 0..3 matching the
                // triangle slot order, so they map straight back.
                let lo = u32::from(edge_id.lo()) as usize;
                let hi = u32::from(edge_id.hi()) as usize;
                let va = vs[lo];
                let vb = vs[hi];
                let dx = f64::from(vb[0]) - f64::from(va[0]);
                let dy = f64::from(vb[1]) - f64::from(va[1]);
                let dz = f64::from(vb[2]) - f64::from(va[2]);
                // Pick the longer XY axis to recover t and avoid
                // dividing by tiny denominators.
                let t = if dx.abs() >= dy.abs() && dx.abs() > 0.0 {
                    (endpoint.x - f64::from(va[0])) / dx
                } else if dy.abs() > 0.0 {
                    (endpoint.y - f64::from(va[1])) / dy
                } else {
                    // Edge is XY-vertical: solve directly from z.
                    (0.0 - f64::from(va[2])) / dz
                };
                let z_reconstructed = dz.mul_add(t, f64::from(va[2]));
                prop_assert!(
                    z_reconstructed.abs() < 1e-3,
                    "endpoint reconstructed z = {z_reconstructed}, expected 0",
                );
            }
        }
    }
}
