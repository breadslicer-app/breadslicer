/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 *
 * Property tests: write a generated mesh, read it back, assert it is
 * the same mesh modulo the format's reindexing/dedup behaviour. The
 * goal is to catch encoder/decoder drift that example-based tests
 * structurally can't (e.g. a quirky float that survives one format
 * but not another, or a triangle ordering that changes after a
 * refactor).
 *
 * Bounds:
 * - Vertex coordinates come from a small integer grid so they are
 *   exactly representable in f32 and survive `{:e}` formatting.
 * - Triangles always carry an explicit non-zero normal, so the
 *   readers' "zero-normal → recompute" fallback is not exercised
 *   here (the malformed-input suite covers that).
 * - PROPTEST_CASES is capped at 32 because writing a Vec<u8>,
 *   parsing it back, and comparing meshes is heavier than a pure
 *   algebra property.
 */

use std::io::Cursor;

use proptest::prelude::*;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, SceneGraph, Vertex};
use sc_mesh_formats::stl::{read_stla, read_stlb, write_stla, write_stlb};

const NORMAL: Normal = Normal::new([1.0, 0.0, 0.0]);

fn small_int_f32() -> impl Strategy<Value = f32> {
    (-10i32..=10i32).prop_map(|i| i as f32)
}

fn vertex_strategy() -> impl Strategy<Value = Vertex> {
    (small_int_f32(), small_int_f32(), small_int_f32()).prop_map(|(x, y, z)| Vertex::new([x, y, z]))
}

/// A mesh with 3..=8 distinct vertices on a small integer grid and
/// 1..=5 non-degenerate triangles referencing them. Triangles may
/// share vertices but the three indices in any one triangle are
/// distinct (degenerate-vertex-index triangles are out of scope).
fn indexed_mesh_strategy() -> impl Strategy<Value = IndexedMesh> {
    proptest::collection::vec(vertex_strategy(), 3..=8)
        .prop_flat_map(|vertices| {
            let n = vertices.len();
            let face = (0..n, 0..n, 0..n).prop_filter(
                "triangle vertex indices must be pairwise distinct",
                |(a, b, c)| a != b && b != c && a != c,
            );
            (Just(vertices), proptest::collection::vec(face, 1..=5))
        })
        .prop_map(|(vertices, faces)| {
            let faces = faces
                .into_iter()
                .map(|(a, b, c)| {
                    IndexedTriangle::from_raw_indices(NORMAL, [a as u32, b as u32, c as u32])
                })
                .collect();
            IndexedMesh::new(None, vertices, faces)
        })
}

/// Returns the multiset of triangles a mesh denotes, each triangle
/// expanded to its three vertex coordinates. Order within each
/// triangle is preserved (winding matters); order across triangles
/// is sorted so the comparison is independent of how the writer/reader
/// happens to lay out the face list.
fn triangle_multiset(mesh: &IndexedMesh) -> Vec<[[u32; 3]; 3]> {
    let mut out: Vec<[[u32; 3]; 3]> = mesh
        .faces()
        .iter()
        .map(|face| {
            let v0 = mesh.vertices()[usize::from(face.vertices[0])];
            let v1 = mesh.vertices()[usize::from(face.vertices[1])];
            let v2 = mesh.vertices()[usize::from(face.vertices[2])];
            // Compare by exact f32 bit pattern: the on-grid values we
            // generate are exact, and the writers/readers preserve
            // them bit-for-bit (binary directly, ASCII via `{:e}`
            // shortest round-trip).
            [
                v0.coords().map(f32::to_bits),
                v1.coords().map(f32::to_bits),
                v2.coords().map(f32::to_bits),
            ]
        })
        .collect();
    out.sort();
    out
}

fn unwrap_single_mesh(graph: &SceneGraph) -> &IndexedMesh {
    assert_eq!(
        graph.reachable_leaf_count(),
        1,
        "expected exactly one reachable leaf"
    );
    let id = graph
        .first_leaf_id()
        .expect("reachable_leaf_count == 1 implies a leaf exists");
    graph.mesh_at_leaf(id).expect("leaf resolves to a mesh")
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn stla_round_trip_preserves_triangles(mesh in indexed_mesh_strategy()) {
        let mut buf: Vec<u8> = Vec::new();
        write_stla(&mesh, &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let graph = read_stla(&mut reader).unwrap();
        let recovered = unwrap_single_mesh(&graph);

        prop_assert_eq!(recovered.faces().len(), mesh.faces().len());
        prop_assert_eq!(triangle_multiset(recovered), triangle_multiset(&mesh));
    }

    #[test]
    fn stlb_round_trip_preserves_triangles(mesh in indexed_mesh_strategy()) {
        let mut buf: Vec<u8> = Vec::new();
        write_stlb(&mesh, &mut buf).unwrap();

        let mut reader = Cursor::new(buf);
        let graph = read_stlb(&mut reader).unwrap();
        let recovered = unwrap_single_mesh(&graph);

        prop_assert_eq!(recovered.faces().len(), mesh.faces().len());
        prop_assert_eq!(triangle_multiset(recovered), triangle_multiset(&mesh));
    }

    #[test]
    fn stla_round_trip_dedups_vertices(mesh in indexed_mesh_strategy()) {
        // The reader runs every triangle's vertices through dedup, so
        // the recovered mesh has no more vertices than the source has
        // distinct vertex bit patterns.
        let mut buf: Vec<u8> = Vec::new();
        write_stla(&mesh, &mut buf).unwrap();
        let mut reader = Cursor::new(buf);
        let graph = read_stla(&mut reader).unwrap();
        let recovered = unwrap_single_mesh(&graph);

        let distinct_input: std::collections::HashSet<[u32; 3]> = mesh
            .vertices()
            .iter()
            .map(|v| v.coords().map(f32::to_bits))
            .collect();
        prop_assert!(recovered.vertices().len() <= distinct_input.len());
    }

    #[test]
    fn stlb_round_trip_preserves_normal(mesh in indexed_mesh_strategy()) {
        // Binary STL stores the normal as three f32s; the recovered
        // normal must match bit-for-bit.
        let mut buf: Vec<u8> = Vec::new();
        write_stlb(&mesh, &mut buf).unwrap();
        let mut reader = Cursor::new(buf);
        let graph = read_stlb(&mut reader).unwrap();
        let recovered = unwrap_single_mesh(&graph);

        for face in recovered.faces() {
            prop_assert_eq!(face.normal, NORMAL);
        }
    }
}
