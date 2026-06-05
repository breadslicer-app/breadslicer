/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Public-API tests for the [`crate::mesh_slice`] module: the
//! single-Z producer [`crate::mesh_slice::segments_at_z`] and the
//! schedule-driven [`crate::mesh_slice::slice_mesh`].
//!
//! Fixture meshes are hand-rolled and manifold by inspection. Tests
//! exercise the cube case (simplest closed manifold) plus two
//! prism-with-rectangular-tunnel cases that introduce non-trivial
//! topology: a Z-axis tunnel that yields an outer-plus-inner pair of
//! contour components when sliced horizontally, and a Y-axis tunnel
//! whose horizontal-slice topology transitions between two disjoint
//! rectangles (slice passes through the tunnel) and one rectangle
//! (slice is above or below the tunnel band).
//!
//! The contour-component count is recovered from the unstitched
//! segment list by walking an `EndpointTag` adjacency graph. This previews
//! the stitcher's join logic and lets us assert end-to-end topology
//! invariants directly against `segments_at_z`'s output.

use std::collections::{HashMap, HashSet};

use sc_mesh_core::indexed_mesh::MeshTopology;
use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex, VertexIndex};
use sc_mesh_core::{IndexedMesh, ValidatedManifoldMesh};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};

use crate::geom2d::{EndpointTag, Segment};
use crate::mesh_slice::{segments_at_z, slice_mesh};
use crate::profile::LayerSchedule;
use crate::stitch::stitch_layer;

fn witness(mesh: &IndexedMesh) -> ValidatedManifoldMesh<'_> {
    mesh.validated_faces()
        .expect("in-range faces")
        .validated_manifold()
        .expect("manifold mesh")
}

/// Pin the stitcher invariant: every crossed mesh edge contributes
/// exactly two segment endpoints to the layer's segment list.
fn assert_endpoint_tag_pairing(segments: &[Segment]) {
    let mut counts: HashMap<EndpointTag, usize> = HashMap::new();
    for seg in segments {
        *counts.entry(seg.a_tag).or_insert(0) += 1;
        *counts.entry(seg.b_tag).or_insert(0) += 1;
    }
    for (id, count) in &counts {
        assert_eq!(
            *count, 2,
            "endpoint tag {id:?} appears in {count} endpoints, expected exactly 2",
        );
    }
}

/// Recover the number of disjoint contour components from a layer of
/// crossing segments by building an `EndpointTag` adjacency graph
/// (one edge per segment, two endpoints per edge) and counting
/// connected components via flood fill. Each component is one closed
/// contour the stitcher will eventually produce.
fn count_contour_components(segments: &[Segment]) -> usize {
    let mut adjacency: HashMap<EndpointTag, HashSet<EndpointTag>> = HashMap::new();
    for seg in segments {
        adjacency.entry(seg.a_tag).or_default().insert(seg.b_tag);
        adjacency.entry(seg.b_tag).or_default().insert(seg.a_tag);
    }
    let mut visited: HashSet<EndpointTag> = HashSet::new();
    let mut components = 0;
    for &start in adjacency.keys() {
        if visited.contains(&start) {
            continue;
        }
        components += 1;
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            if let Some(neighbours) = adjacency.get(&node) {
                for &n in neighbours {
                    if !visited.contains(&n) {
                        stack.push(n);
                    }
                }
            }
        }
    }
    components
}

// =====================================================================
// Cube tests.
// =====================================================================

#[test]
fn cube_above_top_face_returns_empty() {
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    assert!(segments_at_z(&w, 1.0).is_empty());
}

#[test]
fn cube_below_bottom_face_returns_empty() {
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    assert!(segments_at_z(&w, -1.0).is_empty());
}

#[test]
fn cube_at_mid_height_returns_eight_segments() {
    // Centred cube spans Z in [-0.5, +0.5]. At Z = 0 the plane cuts
    // the 4 side faces only; each side face is 2 triangles, so 8
    // triangles produce 8 Crossing segments.
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.0);
    assert_eq!(segments.len(), 8);
    for seg in &segments {
        assert_ne!(seg.a_tag, seg.b_tag);
    }
}

#[test]
fn cube_mid_height_segment_endpoints_sit_at_canonical_points() {
    // Crossing endpoints at Z = 0 fall on either one of the 4
    // vertical cube edges (projected to (+/- 0.5, +/- 0.5)) or on
    // one of the 4 face-internal diagonals (projected to that face's
    // XY midpoint, e.g. (0, +/- 0.5) or (+/- 0.5, 0)).
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.0);
    let canonical_points = [
        (-0.5, -0.5),
        (0.5, -0.5),
        (0.5, 0.5),
        (-0.5, 0.5),
        (0.0, -0.5),
        (0.5, 0.0),
        (0.0, 0.5),
        (-0.5, 0.0),
    ];
    for seg in &segments {
        for endpoint in [seg.a, seg.b] {
            let matches_point = canonical_points
                .iter()
                .any(|(cx, cy)| (endpoint.x - cx).abs() < 1e-9 && (endpoint.y - cy).abs() < 1e-9);
            assert!(
                matches_point,
                "endpoint {endpoint:?} does not match any canonical contour point",
            );
        }
    }
}

#[test]
fn cube_mid_height_each_endpoint_tag_appears_in_exactly_two_endpoints() {
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.0);
    assert_endpoint_tag_pairing(&segments);
}

#[test]
fn cube_mid_height_has_single_contour_component() {
    // The cube contour is one connected square.
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.0);
    assert_eq!(count_contour_components(&segments), 1);
}

// =====================================================================
// Z-tunnel prism tests (hole faces up). Horizontal slice gives an
// outer rectangle with an inner rectangular hole: 2 contour components.
// =====================================================================

#[test]
fn z_tunnel_above_top_returns_empty() {
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    assert!(segments_at_z(&w, 2.5).is_empty());
}

#[test]
fn z_tunnel_below_bottom_returns_empty() {
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    assert!(segments_at_z(&w, -0.5).is_empty());
}

#[test]
fn z_tunnel_mid_height_pairs_every_endpoint_tag() {
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.0);
    assert!(!segments.is_empty());
    assert_endpoint_tag_pairing(&segments);
}

#[test]
fn z_tunnel_mid_height_has_two_contour_components() {
    // Outer rectangle plus inner rectangular hole: 2 disjoint
    // components in the EndpointTag adjacency graph.
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.0);
    assert_eq!(count_contour_components(&segments), 2);
}

// =====================================================================
// Y-tunnel prism tests (hole faces sideways). Topology of the
// horizontal slice depends on whether the slice passes through the
// tunnel band.
// =====================================================================

#[test]
fn y_tunnel_above_box_returns_empty() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    assert!(segments_at_z(&w, 2.5).is_empty());
}

#[test]
fn y_tunnel_below_box_returns_empty() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    assert!(segments_at_z(&w, -0.5).is_empty());
}

#[test]
fn y_tunnel_through_tunnel_band_pairs_every_endpoint_tag() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.0);
    assert!(!segments.is_empty());
    assert_endpoint_tag_pairing(&segments);
}

#[test]
fn y_tunnel_through_tunnel_band_has_two_contour_components() {
    // At z = 1.0 the slicing plane crosses the tunnel's z range
    // [0.6, 1.4]. The solid slice is the outer rectangle minus the
    // tunnel cutout, leaving two disjoint rectangles (left of the
    // cutout and right of the cutout).
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.0);
    assert_eq!(count_contour_components(&segments), 2);
}

#[test]
fn y_tunnel_above_tunnel_band_pairs_every_endpoint_tag() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.6);
    assert!(!segments.is_empty());
    assert_endpoint_tag_pairing(&segments);
}

#[test]
fn y_tunnel_above_tunnel_band_has_one_contour_component() {
    // At z = 1.6 the slicing plane is above the tunnel top (z =
    // 1.4) but still inside the outer box. The solid slice is the
    // full outer rectangle, one contour component.
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.6);
    assert_eq!(count_contour_components(&segments), 1);
}

#[test]
fn y_tunnel_below_tunnel_band_has_one_contour_component() {
    // Symmetric to the above-tunnel case.
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.3);
    assert_eq!(count_contour_components(&segments), 1);
}

// =====================================================================
// slice_mesh: schedule-driven wrapper over segments_at_z.
// =====================================================================

#[test]
fn slice_mesh_yields_paired_z_and_segments_following_schedule() {
    // Centred cube spans Z in [-0.5, +0.5]. Schedule h0 = h = 0.1
    // with 4 layers gives centred Z planes at 0.05, 0.15, 0.25, 0.35,
    // all inside the cube, so every layer cuts the 4 side faces and
    // returns 8 segments per layer.
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let schedule = LayerSchedule::uniform(0.1, 0.1, 4).unwrap();
    let layers = slice_mesh(&w, &schedule);

    assert_eq!(layers.len(), 4);

    let expected_zs: Vec<f64> = schedule.iter_z().collect();
    for ((got_z, segments), expected_z) in layers.iter().zip(expected_zs.iter()) {
        assert_eq!(got_z, expected_z);
        assert_eq!(segments.len(), 8);
        assert_endpoint_tag_pairing(segments);
    }

    // Strict-monotone Z carries through from the schedule.
    for w in layers.windows(2) {
        assert!(w[1].0 > w[0].0);
    }
}

#[test]
fn slice_mesh_returns_empty_layers_for_schedule_above_the_mesh() {
    // Schedule entirely above the cube (cube tops out at z = 0.5):
    // every layer slices an empty plane and yields zero segments,
    // but the (z, segments) pairs are still produced in order.
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let schedule = LayerSchedule::uniform(0.1, 1.0, 3).unwrap();
    let layers = slice_mesh(&w, &schedule);

    assert_eq!(layers.len(), 3);
    for (_, segments) in &layers {
        assert!(segments.is_empty());
    }
}

// =====================================================================
// AOB saddle-vertex tests. A tetrahedron whose vertex V0 sits exactly
// on the slicing plane, with the other three vertices split into one
// below and two above. Around V0 there are two AOB triangles and one
// AAO; together with the bottom-cap straddling triangle they form a
// 3-segment contour that closes only if vertex-tagged endpoints mate
// at the V0 hub.
// =====================================================================

/// Saddle-vertex tetrahedron used by the AOB tests.
///
/// V0 sits at z = 0 (on the slicing plane), V1 and V2 above, V3 below.
/// The face winding mirrors `mesh-core`'s manifold tetrahedron pattern
/// so [`validated_manifold`] accepts the mesh.
fn aob_saddle_tetrahedron() -> IndexedMesh {
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 1.0]),
            Vertex::new([-1.0, 0.0, 1.0]),
            Vertex::new([0.0, 1.0, -1.0]),
        ],
        faces: vec![
            IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 2]),
            IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 3, 1]),
            IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 2, 3]),
            IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [1, 3, 2]),
        ],
    }
}

#[test]
fn aob_saddle_tetrahedron_at_z_zero_emits_three_segments() {
    let mesh = aob_saddle_tetrahedron();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.0);
    assert_eq!(
        segments.len(),
        3,
        "expected 2 through-vertex + 1 crossing = 3 segments, got: {segments:?}",
    );
}

#[test]
fn aob_saddle_tetrahedron_at_z_zero_has_two_vertex_tagged_endpoints_at_v0() {
    let mesh = aob_saddle_tetrahedron();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.0);
    let vertex_tag = EndpointTag::Vertex(VertexIndex::from(0_u32));
    let count = segments
        .iter()
        .flat_map(|s| [s.a_tag, s.b_tag])
        .filter(|t| *t == vertex_tag)
        .count();
    assert_eq!(
        count, 2,
        "expected 2 endpoints tagged Vertex(0), got {count}"
    );
}

#[test]
fn aob_saddle_tetrahedron_stitch_closes_into_a_single_triangle_ring() {
    let mesh = aob_saddle_tetrahedron();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.0);
    let topology = MeshTopology::from_manifold(&w);
    let rings = stitch_layer(&segments, 0.0, &topology).expect("AOB hub closes cleanly");
    assert_eq!(
        rings.len(),
        1,
        "expected one closed ring, got {}",
        rings.len()
    );
    assert_eq!(
        rings[0].vertices().len(),
        3,
        "expected 3-vertex ring, got: {:?}",
        rings[0].vertices(),
    );
}
