/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Streaming-triangle bridge from format readers to [`IndexedMesh`].
//!
//! Defines the [`TriangleIterator`] trait that every reader implements,
//! plus the shared [`IndexedMeshBuildError`] surfaced by the default
//! `as_indexed_mesh` implementation when the deduplicated-vertex stream
//! cannot fit into the indexed-mesh representation.

use std::collections::HashMap;
use std::fmt;

use crate::indexed_mesh::{IndexedMesh, MeshMetadata};
use crate::primitives::{IndexedTriangle, Triangle, Vertex};

/// Failure modes intrinsic to building an [`IndexedMesh`] from a stream
/// of triangles, independent of the reader's source format.
///
/// Readers absorb this into their own error type via `From`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedMeshBuildError {
    /// The unique-vertex count exceeded what `IndexedMesh` can index.
    /// `IndexedTriangle` stores indices as `u32`, so the upper bound is
    /// `u32::MAX as usize`.
    TooManyVertices { limit: usize },
}

impl fmt::Display for IndexedMeshBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyVertices { limit } => write!(
                f,
                "indexed-mesh build: unique vertex count exceeds the {limit} index limit",
            ),
        }
    }
}

impl std::error::Error for IndexedMeshBuildError {}

/// Fallible streaming iterator of triangles, used by format readers as
/// the bridge between byte-level parsing and the indexed-mesh aggregate.
///
/// The associated `Error` is owned by the adapter; the domain core only
/// requires that yielded items report a `Result<Triangle, Self::Error>`
/// and that the error type can absorb [`IndexedMeshBuildError`] so the
/// default `as_indexed_mesh` implementation can surface build-time
/// failures without depending on any specific reader's error type.
pub trait TriangleIterator:
    Iterator<Item = Result<Triangle, <Self as TriangleIterator>::Error>>
{
    type Error: From<IndexedMeshBuildError>;

    fn as_indexed_mesh(&mut self, meta: Option<MeshMetadata>) -> Result<IndexedMesh, Self::Error> {
        build_indexed_mesh_with_cap(self, meta, u32::MAX as usize)
    }
}

/// Core of [`TriangleIterator::as_indexed_mesh`], parameterised on the
/// unique-vertex cap so tests can drive the overflow path without
/// allocating `u32::MAX` vertices.
///
/// The cap is inclusive: a stream that produces exactly `max_vertices`
/// unique vertices succeeds; the next distinct vertex returns
/// [`IndexedMeshBuildError::TooManyVertices`].
pub(crate) fn build_indexed_mesh_with_cap<I>(
    iter: &mut I,
    meta: Option<MeshMetadata>,
    max_vertices: usize,
) -> Result<IndexedMesh, I::Error>
where
    I: TriangleIterator + ?Sized,
{
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut triangles: Vec<IndexedTriangle> = Vec::new();
    let mut vertex_to_index: HashMap<[u32; 3], usize> = HashMap::new();

    // Do not reserve memory in those structures based on size_hint, because we might have just
    // read bogus data.
    let mut vertex_indices: [u32; 3] = [0; 3];
    for t in iter {
        let t = t?;
        for (i, vertex) in t.vertices.iter().enumerate() {
            // f32 has no Eq and no Hash, but comparing the bits will do.
            // This has the effect that if any coordinate is NaN (which does not make sense
            // anyway), its NaN payload bits will be used as the identity of the vertex.
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
                    return Err(I::Error::from(IndexedMeshBuildError::TooManyVertices {
                        limit: max_vertices,
                    }));
                }
                vertices.push(*vertex);
            }

            vertex_indices[i] = u32::try_from(index).map_err(|_| {
                I::Error::from(IndexedMeshBuildError::TooManyVertices {
                    limit: max_vertices,
                })
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

#[cfg(test)]
mod triangle_iterator_tests {
    use super::*;
    use crate::primitives::Normal;

    /// Test-only error type that absorbs [`IndexedMeshBuildError`] and
    /// nothing else, so the assertions below can match exactly on the
    /// build-time variant.
    #[derive(Debug, PartialEq, Eq)]
    enum StubError {
        Build(IndexedMeshBuildError),
    }

    impl From<IndexedMeshBuildError> for StubError {
        fn from(e: IndexedMeshBuildError) -> Self {
            Self::Build(e)
        }
    }

    /// Yields a fixed list of unit-normal triangles, each constructed
    /// from three predetermined `Vertex` values. Used to drive the
    /// vertex-cap path deterministically.
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
        Triangle::new(
            Normal::new([0.0, 0.0, 1.0]),
            [Vertex::new(a), Vertex::new(b), Vertex::new(c)],
        )
    }

    #[test]
    fn build_below_cap_produces_indexed_mesh() {
        // 2 triangles sharing one edge, 4 distinct vertices total.
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
        // 2 triangles, 5 distinct vertices total. Cap = 4 rejects the
        // vertex that would land at index 4.
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
        // 4 distinct vertices, cap = 4. Boundary case: the last vertex
        // lands at index 3 < cap, so the build succeeds.
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
