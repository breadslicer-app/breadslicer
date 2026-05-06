/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Object-space cluster AABBs over an [`IndexedMesh`]'s face array.
//!
//! Faces are grouped by their position in [`IndexedMesh::faces`] into
//! fixed-size runs of [`TriangleClusters::CLUSTER_SIZE`]; each run gets
//! one [`Aabb`] covering the union of its triangle vertices. Spatial
//! query consumers (the picker today, slicing later) can slab-test the
//! cluster AABBs first and only iterate triangles inside surviving
//! clusters.
//!
//! The clustering is purely a function of the mesh's `faces` order plus
//! the referenced vertex positions, so the structure invalidates on the
//! same boundary as [`super::IndexedMesh::local_aabb_cache`]: vertex
//! mutation, or any face mutation that changes face order. The
//! validation witnesses
//! ([`super::ValidatedFacesMut`](super::ValidatedFacesMut)) only flip
//! winding / recompute normals, neither of which moves the cluster
//! AABBs, so they leave the cache valid.

use crate::aabb::Aabb;
use crate::primitives::{IndexedTriangle, Vertex};

/// Object-space AABBs over fixed-size runs of an [`IndexedMesh`]'s
/// faces. See the module docs for the build / invalidation contract.
#[derive(Clone, Debug, PartialEq)]
pub struct TriangleClusters {
    face_count: u32,
    aabbs: Vec<Aabb>,
}

impl TriangleClusters {
    /// Number of faces packed into one cluster. Picked to balance
    /// memory (one [`Aabb`] per cluster ≈ 24 B) against query speedup
    /// (a hit cluster pays a linear MT walk over up to this many
    /// faces).
    pub const CLUSTER_SIZE: u32 = 64;

    /// Build clusters for a face / vertex pair. Returns `None` when
    /// `faces` is empty (no clusters to emit) or when any face refers
    /// to an out-of-range vertex index — corrupt-mesh callers (e.g. the
    /// picker) treat this the same as "miss" rather than surfacing the
    /// error. For an in-range mesh, the result has length
    /// `ceil(faces.len() / CLUSTER_SIZE)` and the i-th cluster covers
    /// faces `[i * CLUSTER_SIZE, min((i + 1) * CLUSTER_SIZE, faces.len()))`.
    pub fn build(vertices: &[Vertex], faces: &[IndexedTriangle]) -> Option<Self> {
        if faces.is_empty() {
            return None;
        }
        let cluster_size = Self::CLUSTER_SIZE as usize;
        let num_clusters = faces.len().div_ceil(cluster_size);
        let mut aabbs = Vec::with_capacity(num_clusters);
        for chunk in faces.chunks(cluster_size) {
            let mut aabb = Aabb::empty();
            for face in chunk {
                for vidx in face.vertices {
                    let v = vertices.get(vidx as usize)?;
                    aabb.extend(*v);
                }
            }
            aabbs.push(aabb);
        }
        Some(Self {
            face_count: faces.len() as u32,
            aabbs,
        })
    }

    /// Number of clusters. Always ≥ 1 for a `Some` result and ≤
    /// `ceil(face_count / CLUSTER_SIZE)`.
    pub fn len(&self) -> usize {
        self.aabbs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.aabbs.is_empty()
    }

    /// Total number of faces covered by the clustering. Equal to the
    /// `faces.len()` value passed at build time.
    pub fn face_count(&self) -> u32 {
        self.face_count
    }

    /// AABB of the cluster at index `i`. Panics if `i >= self.len()`.
    pub fn aabb(&self, i: usize) -> Aabb {
        self.aabbs[i]
    }

    /// Iterate `(face_range, aabb)` pairs in cluster order. The range
    /// `start .. end` is half-open and indexes into the original
    /// `faces` slice the clustering was built from.
    pub fn iter(&self) -> impl Iterator<Item = (std::ops::Range<u32>, Aabb)> + '_ {
        let face_count = self.face_count;
        self.aabbs.iter().enumerate().map(move |(i, aabb)| {
            let start = (i as u32) * Self::CLUSTER_SIZE;
            let end = ((i as u32 + 1) * Self::CLUSTER_SIZE).min(face_count);
            (start..end, *aabb)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Normal;

    fn vertex(x: f32, y: f32, z: f32) -> Vertex {
        Vertex::new([x, y, z])
    }

    fn tri(a: u32, b: u32, c: u32) -> IndexedTriangle {
        IndexedTriangle {
            normal: Normal::new([0.0, 0.0, 1.0]),
            vertices: [a, b, c],
        }
    }

    #[test]
    fn build_returns_none_for_empty_faces() {
        let vertices = vec![vertex(0.0, 0.0, 0.0)];
        let faces: Vec<IndexedTriangle> = vec![];
        assert!(TriangleClusters::build(&vertices, &faces).is_none());
    }

    #[test]
    fn build_returns_none_for_out_of_range_face_index() {
        let vertices = vec![vertex(0.0, 0.0, 0.0)];
        let faces = vec![tri(0, 1, 2)];
        assert!(TriangleClusters::build(&vertices, &faces).is_none());
    }

    #[test]
    fn single_face_yields_single_cluster_with_face_aabb() {
        let vertices = vec![
            vertex(-1.0, -2.0, 0.0),
            vertex(3.0, -2.0, 0.0),
            vertex(-1.0, 5.0, 7.0),
        ];
        let faces = vec![tri(0, 1, 2)];
        let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters.face_count(), 1);
        let a = clusters.aabb(0);
        assert_eq!(a.min().coords(), [-1.0, -2.0, 0.0]);
        assert_eq!(a.max().coords(), [3.0, 5.0, 7.0]);
    }

    #[test]
    fn cluster_count_is_ceil_face_count_over_cluster_size() {
        let vertices = vec![vertex(0.0, 0.0, 0.0)];
        let faces = vec![tri(0, 0, 0); (TriangleClusters::CLUSTER_SIZE as usize) + 1];
        let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters.face_count(), faces.len() as u32);
    }

    #[test]
    fn iter_emits_half_open_face_ranges_in_cluster_order() {
        let vertices = vec![vertex(0.0, 0.0, 0.0)];
        let face_count = (TriangleClusters::CLUSTER_SIZE as usize) + 5;
        let faces = vec![tri(0, 0, 0); face_count];
        let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
        let collected: Vec<_> = clusters.iter().map(|(r, _)| r).collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0], 0..TriangleClusters::CLUSTER_SIZE);
        assert_eq!(
            collected[1],
            TriangleClusters::CLUSTER_SIZE..(face_count as u32)
        );
    }

    #[test]
    fn cluster_aabb_only_covers_faces_in_that_cluster() {
        // Two clusters: first cluster faces all sit at z = 0, second
        // cluster faces all sit at z = 100. The cluster AABBs must
        // separate cleanly along z.
        let mut vertices = Vec::new();
        let mut faces = Vec::new();
        let cs = TriangleClusters::CLUSTER_SIZE as usize;
        for _ in 0..cs {
            let base = vertices.len() as u32;
            vertices.push(vertex(0.0, 0.0, 0.0));
            vertices.push(vertex(1.0, 0.0, 0.0));
            vertices.push(vertex(0.0, 1.0, 0.0));
            faces.push(tri(base, base + 1, base + 2));
        }
        for _ in 0..3 {
            let base = vertices.len() as u32;
            vertices.push(vertex(0.0, 0.0, 100.0));
            vertices.push(vertex(1.0, 0.0, 100.0));
            vertices.push(vertex(0.0, 1.0, 100.0));
            faces.push(tri(base, base + 1, base + 2));
        }
        let clusters = TriangleClusters::build(&vertices, &faces).unwrap();
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters.aabb(0).max().coords()[2], 0.0);
        assert_eq!(clusters.aabb(1).min().coords()[2], 100.0);
    }
}
