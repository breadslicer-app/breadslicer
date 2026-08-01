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
use sc_mesh_core::primitives::VertexIndex;
use sc_mesh_core::{IndexedMesh, ValidatedManifoldMesh, Vertex};
use sc_mesh_fixtures::{
    aob_saddle_tetrahedron, bowtie_tetrahedra, cuboid_with_pinched_z_tunnel, cuboid_with_y_tunnel,
    cuboid_with_z_tunnel, on_edge_crossing_tetrahedron, on_edge_skim_above_tetrahedron,
    unit_cube_mesh,
};

use crate::contour::ClosedRing;
use crate::geom2d::{EndpointTag, Segment};
use crate::mesh_slice::{segments_at_z, segments_at_z_for_faces, slice_mesh};
use crate::plane_intersect::Z_CLASSIFY_EPS_MM;
use crate::profile::LayerSchedule;
use crate::stitch::stitch_layer;
use crate::z_bins::LayerBuckets;

fn witness(mesh: &IndexedMesh) -> ValidatedManifoldMesh<'_> {
    mesh.validated_faces()
        .expect("in-range faces")
        .validated_manifold()
        .expect("manifold mesh")
}

/// Test-only wrapper around [`segments_at_z`] that builds the
/// [`MeshTopology`] inline. Existing fixture-based tests don't need
/// to expose the topology so threading it through every call would
/// be noise; this helper keeps the call sites symmetric with the
/// pre-topology era.
fn slice_at(w: &ValidatedManifoldMesh<'_>, z_mm: f64) -> Vec<Segment> {
    let topology = MeshTopology::from_manifold(w);
    segments_at_z(w, z_mm, &topology)
}

/// Test-only wrapper around [`slice_mesh`] that builds the
/// [`MeshTopology`] inline.
fn slice_schedule<'a>(
    w: &'a ValidatedManifoldMesh<'a>,
    schedule: &'a LayerSchedule,
) -> Vec<(f64, Vec<Segment>)> {
    let topology = MeshTopology::from_manifold(w);
    slice_mesh(w, schedule, &topology)
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
    assert!(slice_at(&w, 1.0).is_empty());
}

#[test]
fn cube_below_bottom_face_returns_empty() {
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    assert!(slice_at(&w, -1.0).is_empty());
}

#[test]
fn cube_at_mid_height_returns_eight_segments() {
    // Centred cube spans Z in [-0.5, +0.5]. At Z = 0 the plane cuts
    // the 4 side faces only; each side face is 2 triangles, so 8
    // triangles produce 8 Crossing segments.
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let segments = slice_at(&w, 0.0);
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
    let segments = slice_at(&w, 0.0);
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
    let segments = slice_at(&w, 0.0);
    assert_endpoint_tag_pairing(&segments);
}

#[test]
fn cube_mid_height_has_single_contour_component() {
    // The cube contour is one connected square.
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let segments = slice_at(&w, 0.0);
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
    assert!(slice_at(&w, 2.5).is_empty());
}

#[test]
fn z_tunnel_below_bottom_returns_empty() {
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    assert!(slice_at(&w, -0.5).is_empty());
}

#[test]
fn z_tunnel_mid_height_pairs_every_endpoint_tag() {
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    let segments = slice_at(&w, 1.0);
    assert!(!segments.is_empty());
    assert_endpoint_tag_pairing(&segments);
}

#[test]
fn z_tunnel_mid_height_has_two_contour_components() {
    // Outer rectangle plus inner rectangular hole: 2 disjoint
    // components in the EndpointTag adjacency graph.
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    let segments = slice_at(&w, 1.0);
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
    assert!(slice_at(&w, 2.5).is_empty());
}

#[test]
fn y_tunnel_below_box_returns_empty() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    assert!(slice_at(&w, -0.5).is_empty());
}

#[test]
fn y_tunnel_through_tunnel_band_pairs_every_endpoint_tag() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = slice_at(&w, 1.0);
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
    let segments = slice_at(&w, 1.0);
    assert_eq!(count_contour_components(&segments), 2);
}

#[test]
fn y_tunnel_above_tunnel_band_pairs_every_endpoint_tag() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = slice_at(&w, 1.6);
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
    let segments = slice_at(&w, 1.6);
    assert_eq!(count_contour_components(&segments), 1);
}

#[test]
fn y_tunnel_below_tunnel_band_has_one_contour_component() {
    // Symmetric to the above-tunnel case.
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = slice_at(&w, 0.3);
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
    let layers = slice_schedule(&w, &schedule);

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
    let layers = slice_schedule(&w, &schedule);

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

#[test]
fn aob_saddle_tetrahedron_at_z_zero_emits_three_segments() {
    let mesh = aob_saddle_tetrahedron();
    let w = witness(&mesh);
    let segments = slice_at(&w, 0.0);
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
    let segments = slice_at(&w, 0.0);
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
    let segments = slice_at(&w, 0.0);
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

// =====================================================================
// Edge-case (two on-vertices) tests. Tetrahedra whose two vertices V0
// and V1 sit on the slicing plane. The mesh edge V0-V1 lies on the
// plane and is shared by two triangles; whether the on-edge actually
// belongs to the contour depends on the third vertex of each
// triangle.
// =====================================================================

#[test]
fn on_edge_crossing_tetrahedron_at_z_zero_emits_three_segments() {
    // Expected segments:
    // - F2 (AOB at V0) -> ThroughVertex from V0 to the crossing on V2-V3.
    // - F3 (AOB at V1) -> ThroughVertex from V1 to the crossing on V2-V3.
    // - On-edge V0-V1: emitted from whichever of F0/F1 has the higher
    //   FaceIndex (F1, idx 1); the lower-indexed face skips.
    let mesh = on_edge_crossing_tetrahedron();
    let w = witness(&mesh);
    let segments = slice_at(&w, 0.0);
    assert_eq!(
        segments.len(),
        3,
        "expected 2 through-vertex + 1 on-edge = 3 segments, got: {segments:?}",
    );
}

#[test]
fn on_edge_crossing_tetrahedron_at_z_zero_has_one_double_vertex_tagged_segment() {
    let mesh = on_edge_crossing_tetrahedron();
    let w = witness(&mesh);
    let segments = slice_at(&w, 0.0);
    let on_edge_seg_count = segments
        .iter()
        .filter(|s| {
            matches!(s.a_tag, EndpointTag::Vertex(_)) && matches!(s.b_tag, EndpointTag::Vertex(_))
        })
        .count();
    assert_eq!(
        on_edge_seg_count, 1,
        "expected exactly one segment with both endpoints vertex-tagged (the on-edge), got {on_edge_seg_count}",
    );
}

#[test]
fn on_edge_crossing_tetrahedron_stitch_closes_into_a_single_triangle_ring() {
    let mesh = on_edge_crossing_tetrahedron();
    let w = witness(&mesh);
    let topology = MeshTopology::from_manifold(&w);
    let segments = slice_at(&w, 0.0);
    let rings = stitch_layer(&segments, 0.0, &topology).expect("on-edge resolution closes cleanly");
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

#[test]
fn on_edge_skim_above_tetrahedron_at_z_zero_emits_no_on_edge_segment() {
    // V0 and V1 are on the plane but BOTH third vertices (V2 and V3)
    // sit above: the surface only skims the plane along V0-V1 and the
    // contour does not pass through it. The slicer must drop the
    // on-edge segment entirely.
    let mesh = on_edge_skim_above_tetrahedron();
    let w = witness(&mesh);
    let segments = slice_at(&w, 0.0);
    let on_edge_seg_count = segments
        .iter()
        .filter(|s| {
            matches!(s.a_tag, EndpointTag::Vertex(_)) && matches!(s.b_tag, EndpointTag::Vertex(_))
        })
        .count();
    assert_eq!(
        on_edge_seg_count, 0,
        "expected no on-edge segment for skim-from-above case, got {on_edge_seg_count}",
    );
}

// =====================================================================
// Bucketed candidate scan: segments_at_z_for_faces fed by LayerBuckets
// must reproduce the full scan bit for bit.
// =====================================================================

/// Probe planes for a mesh: a dense sweep across its Z span (plus a
/// margin on both ends) and, for every distinct vertex Z, the plane
/// through it and the planes one classification epsilon to each side.
/// This hits the Crossing, ThroughVertex, Edge, Coplanar, and Empty
/// paths and the exact edges of the On band.
fn probe_planes(mesh: &IndexedMesh) -> Vec<f64> {
    let mut vertex_zs: Vec<f64> = mesh.vertices.iter().map(|v| f64::from(v[2])).collect();
    vertex_zs.sort_by(f64::total_cmp);
    vertex_zs.dedup();
    let lo = vertex_zs.first().copied().expect("non-empty mesh") - 0.1;
    let hi = vertex_zs.last().copied().expect("non-empty mesh") + 0.1;
    let mut planes: Vec<f64> = (0..=100)
        .map(|k| lo + (hi - lo) * f64::from(k) / 100.0)
        .collect();
    for &z in &vertex_zs {
        planes.extend([z - Z_CLASSIFY_EPS_MM, z, z + Z_CLASSIFY_EPS_MM]);
    }
    planes.sort_by(f64::total_cmp);
    planes.dedup();
    planes
}

fn assert_bucketed_scan_matches_full_scan(mesh: &IndexedMesh) {
    let w = witness(mesh);
    let topology = MeshTopology::from_manifold(&w);
    let planes = probe_planes(mesh);
    let buckets = LayerBuckets::build(&w, &planes);
    for (k, &z_mm) in planes.iter().enumerate() {
        let full = segments_at_z(&w, z_mm, &topology);
        let bucketed = segments_at_z_for_faces(&w, z_mm, &topology, buckets.faces_at(k));
        assert_eq!(bucketed, full, "scan mismatch at plane {z_mm}");
    }
}

#[test]
fn bucketed_scan_matches_the_full_scan_on_every_fixture() {
    assert_bucketed_scan_matches_full_scan(&unit_cube_mesh());
    assert_bucketed_scan_matches_full_scan(&cuboid_with_z_tunnel());
    assert_bucketed_scan_matches_full_scan(&cuboid_with_y_tunnel());
    assert_bucketed_scan_matches_full_scan(&aob_saddle_tetrahedron());
    assert_bucketed_scan_matches_full_scan(&on_edge_crossing_tetrahedron());
    assert_bucketed_scan_matches_full_scan(&on_edge_skim_above_tetrahedron());
}

#[test]
fn bucketed_scan_with_every_face_is_the_full_scan() {
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    let topology = MeshTopology::from_manifold(&w);
    let all_faces: Vec<u32> = (0..w.faces().len() as u32).collect();
    for z_mm in [0.0, 0.5, 1.0, 1.999] {
        assert_eq!(
            segments_at_z_for_faces(&w, z_mm, &topology, &all_faces),
            segments_at_z(&w, z_mm, &topology),
        );
    }
}

// =====================================================================
// Pinched-fixture tests. Both fixtures carry a pinch vertex at
// (0, 0, 1): edge-manifold meshes whose pinch has 2 fans. Each is
// exercised in 2 orientations: upright (the lobes sit above and
// below the through-pinch plane) and quarter-turned about X (the
// through-pinch plane cuts both lobes at once).
// =====================================================================

/// Rotate every vertex a quarter turn about the X axis:
/// (x, y, z) -> (x, z, -y). Exact for the integer-coordinate pinch
/// fixtures, and a proper rotation, so winding stays consistent and
/// the witness still mints. The fixtures' pinch at (0, 0, 1) maps
/// to (0, 1, 0), putting the pinch on the plane `z = 0` with a lobe
/// on each side.
fn quarter_turned_about_x(mut mesh: IndexedMesh) -> IndexedMesh {
    for v in &mut mesh.vertices {
        let [x, y, z] = v.coords();
        *v = Vertex::new([x, z, -y]);
    }
    mesh
}

/// Slice at `z_mm` and stitch the layer into closed rings through
/// the production pairing (the tag-graph component helper above is
/// blind to fans: it would fuse rings that share a pinch vertex
/// tag).
fn stitched_rings(mesh: &IndexedMesh, z_mm: f64) -> Vec<ClosedRing> {
    let w = witness(mesh);
    let topology = MeshTopology::from_manifold(&w);
    let segments = segments_at_z(&w, z_mm, &topology);
    stitch_layer(&segments, z_mm, &topology).expect("pinched layer stitches cleanly")
}

/// Occurrences of the point `(x, y)` among the ring's vertices.
fn point_occurrences(ring: &ClosedRing, x: f64, y: f64) -> usize {
    ring.vertices()
        .iter()
        .filter(|p| (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9)
        .count()
}

#[test]
fn upright_bowtie_below_pinch_yields_one_triangle_ring() {
    let rings = stitched_rings(&bowtie_tetrahedra(), 0.5);
    assert_eq!(rings.len(), 1, "one lobe cross-section expected");
    assert_eq!(rings[0].len(), 3);
}

#[test]
fn upright_bowtie_through_pinch_yields_an_empty_layer() {
    // Both lobes touch the z = 1 plane only at the pinch point: a
    // measure-zero cross-section, so no segments and no rings.
    let mesh = bowtie_tetrahedra();
    let w = witness(&mesh);
    let topology = MeshTopology::from_manifold(&w);
    assert!(segments_at_z(&w, 1.0, &topology).is_empty());
    assert!(stitched_rings(&mesh, 1.0).is_empty());
}

#[test]
fn upright_bowtie_above_pinch_yields_one_triangle_ring() {
    let rings = stitched_rings(&bowtie_tetrahedra(), 1.5);
    assert_eq!(rings.len(), 1, "one lobe cross-section expected");
    assert_eq!(rings[0].len(), 3);
}

#[test]
fn sideways_bowtie_through_pinch_yields_two_rings_sharing_the_pinch_point() {
    // The through-pinch plane cuts both lobes: a figure-8 of 2
    // triangle rings joined at the pinch, which projects to (0, 1).
    // Per-fan hub pairing must keep each lobe's ring closed through
    // its own faces instead of cross-stitching the lobes.
    let rings = stitched_rings(&quarter_turned_about_x(bowtie_tetrahedra()), 0.0);
    assert_eq!(rings.len(), 2, "one ring per lobe expected");
    for ring in &rings {
        assert_eq!(ring.len(), 3);
        assert_eq!(
            point_occurrences(ring, 0.0, 1.0),
            1,
            "each lobe ring passes through the pinch point once"
        );
    }
}

#[test]
fn sideways_bowtie_off_pinch_yields_two_disjoint_rings() {
    let rings = stitched_rings(&quarter_turned_about_x(bowtie_tetrahedra()), 0.5);
    assert_eq!(rings.len(), 2, "one ring per lobe expected");
    for a in rings[0].vertices() {
        for b in rings[1].vertices() {
            assert!(
                (a.x - b.x).abs() > 1e-9 || (a.y - b.y).abs() > 1e-9,
                "off-pinch lobe rings must not share any point"
            );
        }
    }
}

#[test]
fn upright_pinched_tunnel_below_pinch_yields_outer_ring_and_hole_ring() {
    let rings = stitched_rings(&cuboid_with_pinched_z_tunnel(), 0.5);
    let mut lens: Vec<usize> = rings.iter().map(ClosedRing::len).collect();
    lens.sort_unstable();
    assert_eq!(
        lens,
        vec![4, 8],
        "expected the 4-point cavity ring inside the 8-point outer ring"
    );
}

#[test]
fn upright_pinched_tunnel_through_pinch_yields_outer_ring_only() {
    // At z = 1 the cavity has narrowed to the pinch point itself:
    // the hole vanishes and only the outer wall contour remains.
    let rings = stitched_rings(&cuboid_with_pinched_z_tunnel(), 1.0);
    assert_eq!(rings.len(), 1, "the point-sized hole must not emit a ring");
    assert_eq!(rings[0].len(), 8);
    assert_eq!(
        point_occurrences(&rings[0], 0.0, 0.0),
        0,
        "the pinch contributes no contour vertex"
    );
}

#[test]
fn upright_pinched_tunnel_above_pinch_yields_outer_ring_and_hole_ring() {
    let rings = stitched_rings(&cuboid_with_pinched_z_tunnel(), 1.5);
    let mut lens: Vec<usize> = rings.iter().map(ClosedRing::len).collect();
    lens.sort_unstable();
    assert_eq!(lens, vec![4, 8]);
}

#[test]
fn sideways_pinched_tunnel_through_pinch_yields_one_ring_visiting_the_pinch_twice() {
    // Quarter-turned, the tunnel runs along Y and opens at y = 0 and
    // y = 2; the through-pinch plane contains the tunnel axis. The
    // cross-section is the outer rectangle with 2 triangular notches
    // (one per cavity pyramid) meeting at the pinch, which projects
    // to (0, 1): a single boundary ring that passes through the
    // pinch point twice. Cross-fan pairing would instead cut the
    // section in half with 2 rings through solid material.
    let rings = stitched_rings(&quarter_turned_about_x(cuboid_with_pinched_z_tunnel()), 0.0);
    assert_eq!(rings.len(), 1, "expected a single self-touching boundary");
    assert_eq!(
        point_occurrences(&rings[0], 0.0, 1.0),
        2,
        "the boundary visits the pinch point once per notch"
    );
}

#[test]
fn sideways_pinched_tunnel_off_pinch_yields_one_ring_avoiding_the_pinch() {
    // Above the pinch plane but still inside the cavity band: the 2
    // notches shorten and no longer reach the pinch, leaving one
    // simple boundary ring.
    let rings = stitched_rings(&quarter_turned_about_x(cuboid_with_pinched_z_tunnel()), 0.2);
    assert_eq!(rings.len(), 1);
    assert_eq!(point_occurrences(&rings[0], 0.0, 1.0), 0);
}
