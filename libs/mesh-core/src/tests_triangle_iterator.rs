/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Equivalence coverage for the sort-based soup deduplication against the
//! retired streaming hash-based builder, plus pinned behavior for the
//! `indexed_mesh_from_soup` entry point (zero-spelling folding, NaN identity,
//! cap boundaries, metadata passthrough).

use std::collections::HashMap;

use proptest::prelude::*;

use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
use crate::primitives::{IndexedTriangle, Normal, Triangle, Vertex, VertexIndex};
use crate::triangle_iterator::{IndexedMeshBuildError, indexed_mesh_from_soup};

/// Builds the mesh with the retired streaming hash-based builder, kept verbatim
/// as the reference the sort-based implementation must reproduce bit for bit
/// (vertex order, vertex bits, index values, normal bits). Only the iteration
/// wrapper differs: the original drew from a fallible triangle stream, this
/// copy walks an already-collected soup.
fn reference_build(
    meta: Option<MeshMetadata>,
    soup: &[Triangle],
    max_vertices: usize,
) -> Result<IndexedMesh, IndexedMeshBuildError> {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut triangles: Vec<IndexedTriangle> = Vec::new();
    let mut vertex_to_index: HashMap<[u32; 3], usize> = HashMap::new();

    let mut vertex_indices: [u32; 3] = [0; 3];
    for t in soup {
        for (i, vertex) in t.vertices.iter().enumerate() {
            // f32 has no Eq and no Hash, but comparing the bits will do.
            let bitpattern = vertex.coords().map(|f| {
                if f == -0.0 {
                    0.0_f32.to_bits()
                } else {
                    f.to_bits()
                }
            });
            let index = *vertex_to_index
                .entry(bitpattern)
                .or_insert_with(|| vertices.len());

            if index == vertices.len() {
                if vertices.len() >= max_vertices {
                    return Err(IndexedMeshBuildError::TooManyVertices {
                        limit: max_vertices,
                    });
                }
                vertices.push(*vertex);
            }

            vertex_indices[i] =
                u32::try_from(index).map_err(|_| IndexedMeshBuildError::TooManyVertices {
                    limit: max_vertices,
                })?;
        }

        triangles.push(IndexedTriangle::from_raw_indices(t.normal, vertex_indices));
    }

    vertices.shrink_to_fit();
    triangles.shrink_to_fit();

    Ok(IndexedMesh {
        meta,
        vertices,
        faces: triangles,
    })
}

/// Returns a coordinate palette exercising exact-bit dedup: a small set of
/// distinct values so generated soups share vertices often, including both zero
/// spellings. NaN is excluded here because `IndexedMesh` equality goes through
/// `f32` PartialEq; NaN identity has its own pinned test below.
fn coord_strategy() -> impl Strategy<Value = f32> {
    prop_oneof![
        Just(0.0_f32),
        Just(-0.0_f32),
        Just(1.0_f32),
        Just(-1.0_f32),
        Just(0.5_f32),
        Just(-2.25_f32),
        Just(1.0e-7_f32),
        Just(3.5e6_f32),
    ]
}

fn vertex_strategy() -> impl Strategy<Value = Vertex> {
    [coord_strategy(), coord_strategy(), coord_strategy()].prop_map(Vertex::new)
}

fn triangle_strategy() -> impl Strategy<Value = Triangle> {
    (
        prop_oneof![
            Just(Normal::new([0.0, 0.0, 1.0])),
            Just(Normal::new([0.0, -1.0, 0.0])),
            Just(Normal::new([1.0, 0.0, 0.0])),
        ],
        [vertex_strategy(), vertex_strategy(), vertex_strategy()],
    )
        .prop_map(|(normal, vertices)| Triangle::new(normal, vertices))
}

fn soup_strategy() -> impl Strategy<Value = Vec<Triangle>> {
    proptest::collection::vec(triangle_strategy(), 0..40)
}

/// Returns a grid soup of 2 * n * m triangles over a shared-vertex height-field
/// grid, large enough for the parallel sorts to fork for real, with the first
/// quad duplicated so duplicate triangles are represented.
fn grid_soup(n: usize, m: usize) -> Vec<Triangle> {
    let vert =
        |i: usize, j: usize| Vertex::new([i as f32 * 0.5, j as f32 * 0.5, ((i * j) % 7) as f32]);
    let up = Normal::new([0.0, 0.0, 1.0]);
    let mut soup = Vec::with_capacity(2 * n * m + 2);
    for i in 0..n {
        for j in 0..m {
            let (a, b, c, d) = (
                vert(i, j),
                vert(i + 1, j),
                vert(i + 1, j + 1),
                vert(i, j + 1),
            );
            soup.push(Triangle::new(up, [a, b, c]));
            soup.push(Triangle::new(up, [a, c, d]));
        }
    }
    let first_quad = [soup[0], soup[1]];
    soup.extend_from_slice(&first_quad);
    soup
}

fn raw_indices(face: &IndexedTriangle) -> [u32; 3] {
    face.vertices.map(VertexIndex::get)
}

#[test]
fn matches_the_reference_on_a_large_grid() {
    let soup = grid_soup(60, 60);
    let expected =
        reference_build(None, &soup, u32::MAX as usize).expect("reference build must succeed");
    let actual =
        indexed_mesh_from_soup(None, soup, u32::MAX as usize).expect("sort build must succeed");
    assert_eq!(actual, expected);
}

#[test]
fn keeps_the_bits_of_the_first_zero_spelling() {
    // -0.0 seen first: the later +0.0 dedups onto it and the stored vertex
    // keeps the -0.0 bits.
    let soup = vec![Triangle::new(
        Normal::new([0.0, 0.0, 1.0]),
        [
            Vertex::new([-0.0, 1.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([2.0, 0.0, 0.0]),
        ],
    )];
    let mesh = indexed_mesh_from_soup(None, soup, 4).expect("2 unique vertices fit a cap of 4");
    assert_eq!(mesh.vertices.len(), 2);
    assert_eq!(raw_indices(&mesh.faces[0]), [0, 0, 1]);
    assert_eq!(mesh.vertices[0].coords()[0].to_bits(), (-0.0_f32).to_bits());
}

#[test]
fn distinct_nan_payloads_stay_distinct_vertices() {
    let nan_a = f32::from_bits(0x7FC0_0001);
    let nan_b = f32::from_bits(0x7FC0_0002);
    let soup = vec![Triangle::new(
        Normal::new([0.0, 0.0, 1.0]),
        [
            Vertex::new([nan_a, 0.0, 0.0]),
            Vertex::new([nan_b, 0.0, 0.0]),
            Vertex::new([nan_a, 0.0, 0.0]),
        ],
    )];
    let mesh = indexed_mesh_from_soup(None, soup, 4).expect("2 unique vertices fit a cap of 4");
    assert_eq!(mesh.vertices.len(), 2);
    assert_eq!(raw_indices(&mesh.faces[0]), [0, 1, 0]);
    assert_eq!(mesh.vertices[0].coords()[0].to_bits(), 0x7FC0_0001);
    assert_eq!(mesh.vertices[1].coords()[0].to_bits(), 0x7FC0_0002);
}

#[test]
fn soup_at_exact_cap_succeeds() {
    // 4 distinct vertices, inclusive cap of 4.
    let soup = vec![
        Triangle::new(
            Normal::new([0.0, 0.0, 1.0]),
            [
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
        ),
        Triangle::new(
            Normal::new([0.0, 0.0, 1.0]),
            [
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([1.0, 1.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
        ),
    ];
    let mesh = indexed_mesh_from_soup(None, soup, 4).expect("4 unique vertices, cap of 4");
    assert_eq!(mesh.vertices.len(), 4);
}

#[test]
fn soup_above_cap_reports_too_many_vertices() {
    // 5 distinct vertices, cap of 4.
    let soup = vec![
        Triangle::new(
            Normal::new([0.0, 0.0, 1.0]),
            [
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
        ),
        Triangle::new(
            Normal::new([0.0, 0.0, 1.0]),
            [
                Vertex::new([2.0, 0.0, 0.0]),
                Vertex::new([3.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
            ],
        ),
    ];
    let err = indexed_mesh_from_soup(None, soup, 4).expect_err("5 unique vertices exceed 4");
    assert_eq!(err, IndexedMeshBuildError::TooManyVertices { limit: 4 });
}

#[test]
fn passes_metadata_through_an_empty_soup() {
    let meta = MeshMetadata {
        name: Some("torus".to_string()),
        ..Default::default()
    };
    let mesh = indexed_mesh_from_soup(Some(meta.clone()), Vec::new(), 0)
        .expect("an empty soup fits a cap of 0");
    assert_eq!(mesh.meta, Some(meta));
    assert!(mesh.vertices.is_empty());
    assert!(mesh.faces.is_empty());
}

proptest! {
    #[test]
    fn matches_the_reference_on_generated_soups(soup in soup_strategy()) {
        let expected = reference_build(None, &soup, u32::MAX as usize);
        let actual = indexed_mesh_from_soup(None, soup, u32::MAX as usize);
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn matches_the_reference_near_the_cap(soup in proptest::collection::vec(triangle_strategy(), 1..20)) {
        let unique = reference_build(None, &soup, u32::MAX as usize)
            .expect("uncapped reference build must succeed")
            .vertices
            .len();
        for cap in [unique.saturating_sub(1), unique, unique + 1] {
            let expected = reference_build(None, &soup, cap);
            let actual = indexed_mesh_from_soup(None, soup.clone(), cap);
            prop_assert_eq!(actual, expected);
        }
    }

    #[test]
    fn is_deterministic_across_runs(soup in soup_strategy()) {
        let first = indexed_mesh_from_soup(None, soup.clone(), u32::MAX as usize);
        let second = indexed_mesh_from_soup(None, soup, u32::MAX as usize);
        prop_assert_eq!(first, second);
    }
}
