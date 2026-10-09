/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 *
 * Property tests: write a generated mesh, read it back, and check that it holds
 * the same triangles, each with its vertex order kept, in any face order. They
 * catch encoder and decoder drift that example-based tests cannot catch (for
 * example a float that survives 1 format but not another, or a winding that
 * changes after a refactor).
 *
 * Bounds:
 * - Vertex coordinates come from a small integer grid, so they are exactly
 *   representable in f32 and survive `{:e}` formatting.
 * - The proptest config runs 32 cases, because writing a `Vec<u8>`, parsing it
 *   back and comparing meshes costs more than a pure algebra property.
 */

use std::io::Cursor;

use proptest::prelude::*;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_mesh_fixtures::with_declared_facet_normals;
use sc_mesh_formats::stl::{read_stla, read_stlb, write_stla, write_stlb};
use sc_scene::Scene;

// The bits of (1, 0, 0).
const NORMAL: [u32; 3] = [0x3f80_0000, 0, 0];

fn small_int_f32() -> impl Strategy<Value = f32> {
    (-10i32..=10i32).prop_map(|i| i as f32)
}

fn vertex_strategy() -> impl Strategy<Value = Vertex> {
    (small_int_f32(), small_int_f32(), small_int_f32()).prop_map(|(x, y, z)| Vertex::new([x, y, z]))
}

/// Returns a strategy of meshes with 3..=8 vertices on a small integer grid and
/// 1..=5 triangles that reference them. The 3 indices of a triangle are
/// distinct, but its vertices can coincide or be collinear.
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
                .map(|(a, b, c)| IndexedTriangle::from_raw_indices([a as u32, b as u32, c as u32]))
                .collect();
            IndexedMesh::new(None, vertices, faces)
        })
}

/// Returns the multiset of triangles a mesh denotes, each triangle expanded to
/// its 3 vertex coordinates. Order within each triangle is preserved (winding
/// matters); order across triangles is sorted so the comparison is independent
/// of how the writer and the reader lay out the face list.
fn triangle_multiset(mesh: &IndexedMesh) -> Vec<[[u32; 3]; 3]> {
    let mut out: Vec<[[u32; 3]; 3]> = mesh
        .faces()
        .iter()
        .map(|face| {
            let v0 = mesh.vertices()[usize::from(face.vertices[0])];
            let v1 = mesh.vertices()[usize::from(face.vertices[1])];
            let v2 = mesh.vertices()[usize::from(face.vertices[2])];
            // Compare by exact f32 bit pattern: the on-grid values we generate
            // are exact, and the writers and readers preserve them bit for bit
            // (binary directly, ASCII through `{:e}` shortest round-trip).
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

fn unwrap_single_mesh(graph: &Scene) -> &IndexedMesh {
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
        let (graph, _) = read_stlb(&mut reader).unwrap();
        let recovered = unwrap_single_mesh(&graph);

        prop_assert_eq!(recovered.faces().len(), mesh.faces().len());
        prop_assert_eq!(triangle_multiset(recovered), triangle_multiset(&mesh));
    }

    #[test]
    fn stla_round_trip_dedups_vertices(mesh in indexed_mesh_strategy()) {
        // The reader runs every triangle's vertices through dedup, so the
        // recovered mesh has no more vertices than the source has distinct
        // vertex bit patterns.
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
        // Binary STL stores the normal as 3 f32s; a declared normal must come
        // back bit-for-bit.
        let declared = vec![NORMAL; mesh.faces().len()];
        let mut buf: Vec<u8> = Vec::new();
        write_stlb(&with_declared_facet_normals(mesh, declared.clone()), &mut buf).unwrap();
        let mut reader = Cursor::new(buf);
        let (graph, _) = read_stlb(&mut reader).unwrap();
        let recovered = unwrap_single_mesh(&graph);

        prop_assert_eq!(recovered.declared_facet_normals(), Some(&declared[..]));
    }
}
