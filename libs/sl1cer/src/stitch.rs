/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Layer-stitch step: glue a layer of crossing segments into closed
//! polygon rings.
//!
//! On a watertight manifold mesh, every interior edge crossed by a
//! slicing plane contributes exactly two segment endpoints to the
//! layer's segment list (one per incident triangle), and every
//! vertex sitting exactly on the plane acts as a hub for `2k`
//! through-vertex endpoints (one per AOB triangle incident to it,
//! `k` contour passes through the vertex).
//!
//! Pairing proceeds in two ways:
//!
//! - [`EndpointTag::Edge`] endpoints are paired by tag equality;
//!   exactly two per `Edge` tag is the manifold guarantee.
//! - [`EndpointTag::Vertex`] endpoints are sorted by their source
//!   face's position in the vertex's cyclic 1-ring of incident
//!   faces, then paired in consecutive groups of two. This is the
//!   well-formed pairing for a saddle vertex on an orientable
//!   manifold.
//!
//! Endpoints whose pairing budget cannot be balanced raise
//! [`SliceError::NonManifoldDuringStitching`] as a runtime safety
//! net for inputs that slipped the upstream manifold check.

use sc_mesh_core::indexed_mesh::MeshTopology;
use sc_mesh_core::primitives::{FaceIndex, VertexIndex};

use crate::contour::ClosedRing;
use crate::error::SliceError;
use crate::geom2d::{EndpointTag, Point2D, Segment};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Side {
    A,
    B,
}

/// One endpoint slot, identified by its segment index and side.
type Endpoint = (usize, Side);

impl Side {
    fn opposite(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

/// A segment endpoint's position in the flat pairing table: two
/// consecutive slots per segment, side A then side B.
fn slot(endpoint: Endpoint) -> usize {
    let (seg_idx, side) = endpoint;
    seg_idx * 2
        + match side {
            Side::A => 0,
            Side::B => 1,
        }
}

fn endpoint_from_slot(slot: usize) -> Endpoint {
    let side = if slot.is_multiple_of(2) {
        Side::A
    } else {
        Side::B
    };
    (slot / 2, side)
}

/// Marker for a slot whose mate has not been written yet. Only ever
/// observed during construction: a layer with any slot left unpaired
/// is rejected as non-manifold before traversal.
const UNPAIRED: usize = usize::MAX;

/// Endpoint-to-mate table built once per layer, then traversed to
/// recover closed rings. A flat vector indexed by [`slot`]; on a
/// manifold layer every slot is written exactly once.
struct EndpointPairings(Vec<usize>);

impl EndpointPairings {
    fn for_segment_count(segments: usize) -> Self {
        Self(vec![UNPAIRED; segments * 2])
    }

    /// Record `lhs` and `rhs` as each other's mates.
    fn pair(&mut self, lhs: Endpoint, rhs: Endpoint) {
        self.0[slot(lhs)] = slot(rhs);
        self.0[slot(rhs)] = slot(lhs);
    }

    /// The mate recorded for `endpoint`.
    fn mate(&self, endpoint: Endpoint) -> Endpoint {
        endpoint_from_slot(self.0[slot(endpoint)])
    }
}

fn point_at(seg: &Segment, side: Side) -> Point2D {
    match side {
        Side::A => seg.a,
        Side::B => seg.b,
    }
}

/// Stitch one layer of crossing and through-vertex segments into
/// closed rings.
///
/// `topology` provides the cyclic 1-ring of each on-plane vertex so
/// that [`EndpointTag::Vertex`] hubs can be paired by consecutive
/// AOB-incident faces. `z_mm` is carried through to the error
/// variant for diagnostics. Returns one [`ClosedRing`] per chain, in
/// arbitrary order; the vertex orientation of each ring is not
/// normalised here (that is the winding-canonicalisation pass).
pub fn stitch_layer(
    segments: &[Segment],
    z_mm: f64,
    topology: &MeshTopology,
) -> Result<Vec<ClosedRing>, SliceError> {
    // One record per segment endpoint, stably sorted by tag: equal
    // tags form contiguous runs that keep their push order (ascending
    // segment index, side A before B), the same order the former
    // hash-map grouping accumulated per tag.
    let mut endpoints: Vec<(EndpointTag, Endpoint)> = Vec::with_capacity(segments.len() * 2);
    for (i, seg) in segments.iter().enumerate() {
        endpoints.push((seg.a_tag, (i, Side::A)));
        endpoints.push((seg.b_tag, (i, Side::B)));
    }
    endpoints.sort_by_key(|&(tag, _)| tag);

    let pairings = match build_pairings(segments, &endpoints, topology) {
        Ok(p) => p,
        Err(dangling_endpoints) => {
            return Err(SliceError::NonManifoldDuringStitching {
                z_mm,
                dangling_endpoints,
            });
        }
    };

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
            let (next_idx, next_entry_side) = pairings.mate((current_idx, exit_side));
            current_idx = next_idx;
            entry_side = next_entry_side;
        }
        rings.push(ClosedRing::new(ring_points));
    }

    Ok(rings)
}

/// Mate each endpoint with exactly one peer, returning the flat
/// endpoint-to-mate table.
///
/// `endpoints` must be sorted by tag so that equal tags form
/// contiguous runs (see [`stitch_layer`]). On a malformed layer (any
/// tag whose total endpoint count is not pairing-compatible),
/// `Err(dangling_endpoints)` is returned with the total number of
/// unpaired endpoints across all malformed tags.
fn build_pairings(
    segments: &[Segment],
    endpoints: &[(EndpointTag, Endpoint)],
    topology: &MeshTopology,
) -> Result<EndpointPairings, usize> {
    let mut pairings = EndpointPairings::for_segment_count(segments.len());
    let mut dangling = 0usize;

    for run in endpoints.chunk_by(|lhs, rhs| lhs.0 == rhs.0) {
        match run[0].0 {
            EndpointTag::Edge(_) => {
                if run.len() != 2 {
                    dangling += run.len();
                    continue;
                }
                pairings.pair(run[0].1, run[1].1);
            }
            EndpointTag::Vertex(v) => {
                if !run.len().is_multiple_of(2) {
                    dangling += run.len();
                    continue;
                }
                let Some(pairs) = pair_vertex_hub(v, run, segments, topology) else {
                    dangling += run.len();
                    continue;
                };
                for (lhs, rhs) in pairs {
                    pairings.pair(lhs, rhs);
                }
            }
        }
    }

    if dangling > 0 {
        Err(dangling)
    } else {
        Ok(pairings)
    }
}

/// Sort the `entries` of an on-plane vertex hub by their source
/// face's position in `vertex`'s cyclic 1-ring, then pair them in
/// consecutive groups of two. Returns `None` if the topology lacks
/// a 1-ring for the vertex or any entry's source face is not part
/// of it.
///
/// The position lookup is a linear scan of the ring: hubs and rings
/// are both a handful of entries, so a lookup table would cost more
/// to build than the scans it saves.
fn pair_vertex_hub(
    vertex: VertexIndex,
    entries: &[(EndpointTag, Endpoint)],
    segments: &[Segment],
    topology: &MeshTopology,
) -> Option<Vec<(Endpoint, Endpoint)>> {
    let ring = topology.vertex_one_ring(vertex)?;
    let ring_position = |face: FaceIndex| ring.iter().position(|&f| f == face);

    let mut sorted: Vec<(usize, Endpoint)> = Vec::with_capacity(entries.len());
    for &(_, endpoint) in entries {
        let position = ring_position(segments[endpoint.0].source_face)?;
        sorted.push((position, endpoint));
    }
    sorted.sort_by_key(|&(position, _)| position);

    Some(
        sorted
            .chunks_exact(2)
            .map(|chunk| (chunk[0].1, chunk[1].1))
            .collect(),
    )
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
    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex, VertexIndex};

    use super::*;
    use crate::geom2d::EdgeId;

    fn vi(i: u32) -> VertexIndex {
        VertexIndex::from(i)
    }

    fn p(x: f64, y: f64) -> Point2D {
        Point2D::new(x, y)
    }

    /// Synthetic segments in these tests carry only edge-tagged
    /// endpoints. The source face is recorded for completeness; the
    /// stitcher never consults it on edge-tag pairings.
    fn dummy_face_idx() -> FaceIndex {
        FaceIndex::from(0_usize)
    }

    fn seg(a: (f64, f64), a_edge: EdgeId, b: (f64, f64), b_edge: EdgeId) -> Segment {
        Segment::new(
            p(a.0, a.1),
            EndpointTag::Edge(a_edge),
            p(b.0, b.1),
            EndpointTag::Edge(b_edge),
            dummy_face_idx(),
        )
    }

    /// Minimal manifold mesh used to build a [`MeshTopology`] for
    /// edge-tag-only stitching tests. The stitcher's vertex-hub
    /// pairing never fires for these tests; topology is only present
    /// to satisfy the signature.
    fn synthetic_topology() -> (IndexedMesh, MeshTopology) {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 1.0, 1.0]),
                Vertex::new([1.0, -1.0, -1.0]),
                Vertex::new([-1.0, 1.0, -1.0]),
                Vertex::new([-1.0, -1.0, 1.0]),
            ],
            faces: vec![
                IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 2]),
                IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 3, 1]),
                IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 2, 3]),
                IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [1, 3, 2]),
            ],
        };
        let topology = {
            let witness = mesh
                .validated_faces()
                .expect("tetrahedron has in-range vertex indices")
                .validated_manifold()
                .expect("tetrahedron is manifold");
            MeshTopology::from_manifold(&witness)
        };
        (mesh, topology)
    }

    #[test]
    fn empty_input_returns_no_rings() {
        let (_mesh, topology) = synthetic_topology();
        let rings = stitch_layer(&[], 0.0, &topology).unwrap();
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
        let (_mesh, topology) = synthetic_topology();
        let rings = stitch_layer(&segments, 0.0, &topology).unwrap();
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
        let (_mesh, topology) = synthetic_topology();
        let rings = stitch_layer(&segments, 0.0, &topology).unwrap();
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
        let (_mesh, topology) = synthetic_topology();
        let rings = stitch_layer(&segments, 0.0, &topology).unwrap();
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
        let (_mesh, topology) = synthetic_topology();
        let err = stitch_layer(&segments, 0.42, &topology).unwrap_err();
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
