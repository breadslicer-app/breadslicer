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
//! segment list by walking an `EdgeId` adjacency graph. This previews
//! the stitcher's join logic and lets us assert end-to-end topology
//! invariants directly against `segments_at_z`'s output.

use std::collections::{HashMap, HashSet};

use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, ValidatedManifoldMesh, Vertex};

use crate::geom2d::{EdgeId, Segment};
use crate::mesh_slice::{segments_at_z, slice_mesh};
use crate::profile::LayerSchedule;

fn witness(mesh: &IndexedMesh) -> ValidatedManifoldMesh<'_> {
    mesh.validated_faces()
        .expect("in-range faces")
        .validated_manifold()
        .expect("manifold mesh")
}

/// Pin the stitcher invariant: every crossed mesh edge contributes
/// exactly two segment endpoints to the layer's segment list.
fn assert_edge_id_pairing(segments: &[Segment]) {
    let mut counts: HashMap<EdgeId, usize> = HashMap::new();
    for seg in segments {
        *counts.entry(seg.a_edge).or_insert(0) += 1;
        *counts.entry(seg.b_edge).or_insert(0) += 1;
    }
    for (id, count) in &counts {
        assert_eq!(
            *count, 2,
            "edge {id:?} appears in {count} endpoints, expected exactly 2",
        );
    }
}

/// Recover the number of disjoint contour components from a layer of
/// crossing segments by building an `EdgeId` adjacency graph (one
/// edge per segment, two endpoints per edge) and counting connected
/// components via flood fill. Each component is one closed contour
/// the stitcher will eventually produce.
fn count_contour_components(segments: &[Segment]) -> usize {
    let mut adjacency: HashMap<EdgeId, HashSet<EdgeId>> = HashMap::new();
    for seg in segments {
        adjacency.entry(seg.a_edge).or_default().insert(seg.b_edge);
        adjacency.entry(seg.b_edge).or_default().insert(seg.a_edge);
    }
    let mut visited: HashSet<EdgeId> = HashSet::new();
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
// Mesh fixtures.
// =====================================================================

/// Closed unit cube centred at the origin (side 1). Manifold by
/// construction.
fn unit_cube_mesh() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([-0.5, -0.5, -0.5]),
        Vertex::new([0.5, -0.5, -0.5]),
        Vertex::new([0.5, 0.5, -0.5]),
        Vertex::new([-0.5, 0.5, -0.5]),
        Vertex::new([-0.5, -0.5, 0.5]),
        Vertex::new([0.5, -0.5, 0.5]),
        Vertex::new([0.5, 0.5, 0.5]),
        Vertex::new([-0.5, 0.5, 0.5]),
    ];
    let nz = Normal::new([0.0, 0.0, -1.0]);
    let pz = Normal::new([0.0, 0.0, 1.0]);
    let ny = Normal::new([0.0, -1.0, 0.0]);
    let py = Normal::new([0.0, 1.0, 0.0]);
    let nx = Normal::new([-1.0, 0.0, 0.0]);
    let px = Normal::new([1.0, 0.0, 0.0]);
    let f = |n: Normal, abc: [u32; 3]| IndexedTriangle::from_raw_indices(n, abc);
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            f(nz, [0, 3, 2]),
            f(nz, [0, 2, 1]),
            f(pz, [4, 5, 6]),
            f(pz, [4, 6, 7]),
            f(py, [3, 7, 6]),
            f(py, [3, 6, 2]),
            f(ny, [0, 1, 5]),
            f(ny, [0, 5, 4]),
            f(px, [1, 2, 6]),
            f(px, [1, 6, 5]),
            f(nx, [0, 4, 7]),
            f(nx, [0, 7, 3]),
        ],
    }
}

/// Rectangular prism with a vertical rectangular tunnel (axis = Z).
/// Outer box spans x in [-1, 1], y in [-1, 1], z in [0, 2]. The
/// tunnel spans x in [-0.4, 0.4], y in [-0.4, 0.4], full z. 16
/// vertices, 32 triangles, genus 1 (Euler V - E + F = 16 - 48 + 32 = 0).
fn cuboid_with_z_tunnel() -> IndexedMesh {
    let vertices = vec![
        // 0..7 outer box corners.
        Vertex::new([-1.0, -1.0, 0.0]),
        Vertex::new([1.0, -1.0, 0.0]),
        Vertex::new([1.0, 1.0, 0.0]),
        Vertex::new([-1.0, 1.0, 0.0]),
        Vertex::new([-1.0, -1.0, 2.0]),
        Vertex::new([1.0, -1.0, 2.0]),
        Vertex::new([1.0, 1.0, 2.0]),
        Vertex::new([-1.0, 1.0, 2.0]),
        // 8..15 inner tunnel corners.
        Vertex::new([-0.4, -0.4, 0.0]),
        Vertex::new([0.4, -0.4, 0.0]),
        Vertex::new([0.4, 0.4, 0.0]),
        Vertex::new([-0.4, 0.4, 0.0]),
        Vertex::new([-0.4, -0.4, 2.0]),
        Vertex::new([0.4, -0.4, 2.0]),
        Vertex::new([0.4, 0.4, 2.0]),
        Vertex::new([-0.4, 0.4, 2.0]),
    ];
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            // Bottom annulus (outward normal -Z, CCW from below).
            f([0, 3, 11]),
            f([0, 11, 8]),
            f([3, 2, 10]),
            f([3, 10, 11]),
            f([2, 1, 9]),
            f([2, 9, 10]),
            f([1, 0, 8]),
            f([1, 8, 9]),
            // Top annulus (outward normal +Z, CCW from above).
            f([4, 5, 13]),
            f([4, 13, 12]),
            f([5, 6, 14]),
            f([5, 14, 13]),
            f([6, 7, 15]),
            f([6, 15, 14]),
            f([7, 4, 12]),
            f([7, 12, 15]),
            // Outer side faces.
            f([0, 1, 5]),
            f([0, 5, 4]),
            f([1, 2, 6]),
            f([1, 6, 5]),
            f([2, 3, 7]),
            f([2, 7, 6]),
            f([3, 0, 4]),
            f([3, 4, 7]),
            // Inner tunnel side faces (normal points into the tunnel).
            f([8, 13, 9]),
            f([8, 12, 13]),
            f([9, 13, 14]),
            f([9, 14, 10]),
            f([10, 14, 15]),
            f([10, 15, 11]),
            f([11, 15, 12]),
            f([11, 12, 8]),
        ],
    }
}

/// Rectangular prism with a horizontal rectangular tunnel (axis = Y).
/// Outer box spans x in [-1, 1], y in [-1, 1], z in [0, 2]. The
/// tunnel spans x in [-0.4, 0.4], full y, z in [0.6, 1.4]. The y =
/// +/- 1 faces become annuli (rectangle with a rectangular hole);
/// the other four outer faces stay solid. 16 vertices, 32 triangles,
/// genus 1.
fn cuboid_with_y_tunnel() -> IndexedMesh {
    let vertices = vec![
        // 0..7 outer box corners.
        Vertex::new([-1.0, -1.0, 0.0]),
        Vertex::new([1.0, -1.0, 0.0]),
        Vertex::new([1.0, 1.0, 0.0]),
        Vertex::new([-1.0, 1.0, 0.0]),
        Vertex::new([-1.0, -1.0, 2.0]),
        Vertex::new([1.0, -1.0, 2.0]),
        Vertex::new([1.0, 1.0, 2.0]),
        Vertex::new([-1.0, 1.0, 2.0]),
        // 8..11 tunnel corners on the front cap (y = -1).
        Vertex::new([-0.4, -1.0, 0.6]),
        Vertex::new([0.4, -1.0, 0.6]),
        Vertex::new([0.4, -1.0, 1.4]),
        Vertex::new([-0.4, -1.0, 1.4]),
        // 12..15 tunnel corners on the back cap (y = +1).
        Vertex::new([-0.4, 1.0, 0.6]),
        Vertex::new([0.4, 1.0, 0.6]),
        Vertex::new([0.4, 1.0, 1.4]),
        Vertex::new([-0.4, 1.0, 1.4]),
    ];
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            // Bottom (solid, outward -Z).
            f([0, 3, 2]),
            f([0, 2, 1]),
            // Top (solid, outward +Z).
            f([4, 5, 6]),
            f([4, 6, 7]),
            // Left (solid, outward -X).
            f([3, 0, 4]),
            f([3, 4, 7]),
            // Right (solid, outward +X).
            f([1, 2, 6]),
            f([1, 6, 5]),
            // Front annulus (y = -1, outward -Y).
            f([0, 1, 9]),
            f([0, 9, 8]),
            f([1, 5, 10]),
            f([1, 10, 9]),
            f([5, 4, 11]),
            f([5, 11, 10]),
            f([4, 0, 8]),
            f([4, 8, 11]),
            // Back annulus (y = +1, outward +Y).
            f([2, 3, 12]),
            f([2, 12, 13]),
            f([3, 7, 15]),
            f([3, 15, 12]),
            f([7, 6, 14]),
            f([7, 14, 15]),
            f([6, 2, 13]),
            f([6, 13, 14]),
            // Inner tunnel walls.
            // Tunnel top (z = 1.4, outward -Z).
            f([11, 15, 14]),
            f([11, 14, 10]),
            // Tunnel bottom (z = 0.6, outward +Z).
            f([8, 9, 13]),
            f([8, 13, 12]),
            // Tunnel left (x = -0.4, outward +X).
            f([8, 12, 15]),
            f([8, 15, 11]),
            // Tunnel right (x = +0.4, outward -X).
            f([9, 10, 14]),
            f([9, 14, 13]),
        ],
    }
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
        assert_ne!(seg.a_edge, seg.b_edge);
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
fn cube_mid_height_each_edge_id_appears_in_exactly_two_endpoints() {
    let mesh = unit_cube_mesh();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 0.0);
    assert_edge_id_pairing(&segments);
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
fn z_tunnel_mid_height_pairs_every_edge_id() {
    let mesh = cuboid_with_z_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.0);
    assert!(!segments.is_empty());
    assert_edge_id_pairing(&segments);
}

#[test]
fn z_tunnel_mid_height_has_two_contour_components() {
    // Outer rectangle plus inner rectangular hole: 2 disjoint
    // components in the EdgeId adjacency graph.
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
fn y_tunnel_through_tunnel_band_pairs_every_edge_id() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.0);
    assert!(!segments.is_empty());
    assert_edge_id_pairing(&segments);
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
fn y_tunnel_above_tunnel_band_pairs_every_edge_id() {
    let mesh = cuboid_with_y_tunnel();
    let w = witness(&mesh);
    let segments = segments_at_z(&w, 1.6);
    assert!(!segments.is_empty());
    assert_edge_id_pairing(&segments);
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
        assert_edge_id_pairing(segments);
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
