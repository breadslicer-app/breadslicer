/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Streaming-triangle bridge from format readers to [`IndexedMesh`].
//!
//! Defines the [`TriangleIterator`] trait for streaming readers, plus the
//! shared [`IndexedMeshBuildError`] surfaced by the default `as_indexed_mesh`
//! implementation when a corner has a NaN or infinite coordinate, or when the
//! deduplicated-vertex stream cannot fit into the indexed-mesh representation.

use std::fmt;

use rayon::slice::ParallelSliceMut;

use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
use crate::primitives::{
    CanonicalCoords, FaceIndex, IndexedTriangle, TriVertexSlot, Triangle, Vertex,
};

/// Failure modes intrinsic to building an [`IndexedMesh`] from a stream of
/// triangles, independent of the reader's source format.
///
/// Readers absorb this into their own error type via `From`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedMeshBuildError {
    /// The unique-vertex count exceeded the cap that the caller gave, `limit`.
    /// The readers pass `u32::MAX as usize`, because `IndexedTriangle` stores
    /// indices as `u32`.
    TooManyVertices { limit: usize },
    /// The vertex at corner `vertex_pos` of face `face_idx` has a NaN or
    /// infinite coordinate. `face_idx` is the position of the triangle in the
    /// input. It is the first such corner in input order, by face index and
    /// then by slot.
    NonFiniteVertex {
        face_idx: FaceIndex,
        vertex_pos: TriVertexSlot,
    },
}

impl fmt::Display for IndexedMeshBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyVertices { limit } => write!(
                f,
                "indexed-mesh build: unique vertex count exceeds the {limit} index limit",
            ),
            Self::NonFiniteVertex {
                face_idx,
                vertex_pos,
            } => write!(
                f,
                "indexed-mesh build: the vertex at face #{face_idx} slot {vertex_pos} has a NaN or infinite coordinate",
            ),
        }
    }
}

impl std::error::Error for IndexedMeshBuildError {}

/// Fallible streaming iterator of triangles: the bridge between byte-level
/// parsing and the indexed-mesh aggregate for a reader that yields 1 triangle
/// at a time.
///
/// The associated `Error` is owned by the adapter; the domain core only
/// requires that yielded items report a `Result<Triangle, Self::Error>` and
/// that the error type can absorb [`IndexedMeshBuildError`] so the default
/// `as_indexed_mesh` implementation can surface build-time failures without
/// depending on any specific reader's error type.
pub trait TriangleIterator:
    Iterator<Item = Result<Triangle, <Self as TriangleIterator>::Error>>
{
    type Error: From<IndexedMeshBuildError>;

    fn as_indexed_mesh(&mut self, meta: Option<MeshMetadata>) -> Result<IndexedMesh, Self::Error> {
        build_indexed_mesh_with_cap(self, meta, u32::MAX as usize)
    }
}

/// Implements [`TriangleIterator::as_indexed_mesh`], parameterised on the
/// unique-vertex cap so tests can drive the overflow path without allocating
/// `u32::MAX` vertices.
///
/// The cap is inclusive: a stream that produces exactly `max_vertices` unique
/// vertices succeeds; the next distinct vertex returns
/// [`IndexedMeshBuildError::TooManyVertices`].
///
/// A corner with a NaN or infinite coordinate returns
/// [`IndexedMeshBuildError::NonFiniteVertex`] for the first such corner in
/// stream order, whatever the cap.
pub(crate) fn build_indexed_mesh_with_cap<I>(
    iter: &mut I,
    meta: Option<MeshMetadata>,
    max_vertices: usize,
) -> Result<IndexedMesh, I::Error>
where
    I: TriangleIterator + ?Sized,
{
    // Do not reserve memory based on size_hint, because we might have just read
    // bogus data.
    let mut soup: Vec<Triangle> = Vec::new();
    for t in iter {
        soup.push(t?);
    }
    indexed_mesh_from_soup(meta, soup, max_vertices).map_err(I::Error::from)
}

/// Builds an [`IndexedMesh`] from a triangle soup. Vertices with the same
/// coordinate bits share 1 index, with `-0.0` taken as `0.0`; the vertex list
/// keeps the order of first appearance, so the vertex list and the index values
/// are what a streaming first-occurrence builder would produce.
///
/// This is the natural entry point for readers that already hold all triangles
/// in memory; [`TriangleIterator::as_indexed_mesh`] funnels into it after
/// draining its stream.
///
/// The cap is inclusive: a soup with exactly `max_vertices` unique vertices
/// succeeds; 1 more distinct vertex returns
/// [`IndexedMeshBuildError::TooManyVertices`].
///
/// A corner with a NaN or infinite coordinate returns
/// [`IndexedMeshBuildError::NonFiniteVertex`] for the first such corner in
/// input order, whatever the cap.
pub fn indexed_mesh_from_soup(
    meta: Option<MeshMetadata>,
    soup: Vec<Triangle>,
    max_vertices: usize,
) -> Result<IndexedMesh, IndexedMeshBuildError> {
    let corner_count = soup.len() * 3;

    // 1 record per corner: (canonical coordinates, sequence number = face * 3 +
    // corner). The sequence number fits in u32 because the format readers cap
    // element counts far below 2^32 corners.
    let mut records: Vec<(CanonicalCoords, u32)> = Vec::with_capacity(corner_count);
    for (face, t) in soup.iter().enumerate() {
        for (slot, vertex) in TriVertexSlot::ALL.into_iter().zip(t.vertices) {
            let coords = vertex.canonical_coords().ok_or_else(|| {
                IndexedMeshBuildError::NonFiniteVertex {
                    face_idx: FaceIndex::from(face),
                    vertex_pos: slot,
                }
            })?;
            records.push((coords, (face * 3 + slot.as_index()) as u32));
        }
    }

    // Tuple order sorts by key first, then by sequence number, so every
    // equal-key run starts with that vertex's earliest appearance.
    records.par_sort_unstable();

    // First-appearance sequence number of every distinct vertex, in key-sorted
    // run order.
    let mut run_first_seq: Vec<u32> = Vec::new();
    let mut prev_key: Option<CanonicalCoords> = None;
    for &(key, seq) in &records {
        if prev_key != Some(key) {
            run_first_seq.push(seq);
            prev_key = Some(key);
        }
    }

    if run_first_seq.len() > max_vertices {
        return Err(IndexedMeshBuildError::TooManyVertices {
            limit: max_vertices,
        });
    }

    // Rank the runs by first appearance: that rank is the index a streaming
    // first-occurrence builder would have assigned.
    let mut runs_by_appearance: Vec<u32> = (0..run_first_seq.len() as u32).collect();
    runs_by_appearance.par_sort_unstable_by_key(|&run| run_first_seq[run as usize]);
    let mut index_of_run: Vec<u32> = vec![0; run_first_seq.len()];
    for (index, &run) in runs_by_appearance.iter().enumerate() {
        index_of_run[run as usize] = index as u32;
    }

    // Emit each distinct vertex from its first appearance, keeping the original
    // bits (the key stores -0.0 as 0.0, the stored vertex does not).
    let mut vertices: Vec<Vertex> = Vec::with_capacity(run_first_seq.len());
    for &run in &runs_by_appearance {
        let seq = run_first_seq[run as usize] as usize;
        vertices.push(soup[seq / 3].vertices[seq % 3]);
    }

    // Scatter every corner's final vertex index back into document order.
    let mut corner_indices: Vec<u32> = vec![0; corner_count];
    let mut run: usize = 0;
    let mut prev_key: Option<CanonicalCoords> = None;
    for &(key, seq) in &records {
        if prev_key != Some(key) {
            if prev_key.is_some() {
                run += 1;
            }
            prev_key = Some(key);
        }
        corner_indices[seq as usize] = index_of_run[run];
    }
    drop(records);

    let faces: Vec<IndexedTriangle> = (0..soup.len())
        .map(|face| {
            IndexedTriangle::from_raw_indices([
                corner_indices[3 * face],
                corner_indices[3 * face + 1],
                corner_indices[3 * face + 2],
            ])
        })
        .collect();

    Ok(IndexedMesh {
        meta,
        vertices,
        faces,
    })
}

#[cfg(test)]
mod triangle_iterator_tests {
    use super::*;

    /// Test-only error type that absorbs [`IndexedMeshBuildError`] and nothing
    /// else, so the assertions below can match exactly on the build-time
    /// variant.
    #[derive(Debug, PartialEq, Eq)]
    enum StubError {
        Build(IndexedMeshBuildError),
    }

    impl From<IndexedMeshBuildError> for StubError {
        fn from(e: IndexedMeshBuildError) -> Self {
            Self::Build(e)
        }
    }

    /// Yields a fixed list of triangles, each constructed from 3 predetermined
    /// `Vertex` values. Used to drive the vertex-cap path deterministically.
    struct StubTriangles {
        items: std::vec::IntoIter<Triangle>,
    }

    impl StubTriangles {
        fn new(items: Vec<Triangle>) -> Self {
            Self {
                items: items.into_iter(),
            }
        }
    }

    impl Iterator for StubTriangles {
        type Item = Result<Triangle, StubError>;

        fn next(&mut self) -> Option<Self::Item> {
            self.items.next().map(Ok)
        }
    }

    impl TriangleIterator for StubTriangles {
        type Error = StubError;
    }

    fn make_triangle(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Triangle {
        Triangle::new([Vertex::new(a), Vertex::new(b), Vertex::new(c)])
    }

    #[test]
    fn build_below_cap_produces_indexed_mesh() {
        // 2 triangles sharing 1 edge, 4 distinct vertices total.
        let triangles = vec![
            make_triangle([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            make_triangle([1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        ];
        let mut iter = StubTriangles::new(triangles);

        let mesh = build_indexed_mesh_with_cap(&mut iter, None, 4)
            .expect("4 unique vertices must fit in a cap of 4");

        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.faces.len(), 2);
        for face in &mesh.faces {
            for idx in face.vertices {
                assert!(usize::from(idx) < mesh.vertices.len());
            }
        }
    }

    #[test]
    fn build_above_cap_returns_too_many_vertices() {
        // 2 triangles, 5 distinct vertices total. Cap = 4 rejects the vertex
        // that would land at index 4.
        let triangles = vec![
            make_triangle([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            make_triangle([2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
        ];
        let mut iter = StubTriangles::new(triangles);

        let err = build_indexed_mesh_with_cap(&mut iter, None, 4)
            .expect_err("5 unique vertices must exceed a cap of 4");

        assert_eq!(
            err,
            StubError::Build(IndexedMeshBuildError::TooManyVertices { limit: 4 }),
        );
    }

    #[test]
    fn build_at_exact_cap_succeeds() {
        // 4 distinct vertices, cap = 4. Boundary case: the last vertex lands at
        // index 3 < cap, so the build succeeds.
        let triangles = vec![
            make_triangle([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            make_triangle([1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        ];
        let mut iter = StubTriangles::new(triangles);

        let mesh = build_indexed_mesh_with_cap(&mut iter, None, 4)
            .expect("4 unique vertices, cap of 4 must succeed");
        assert_eq!(mesh.vertices.len(), 4);
    }
}
