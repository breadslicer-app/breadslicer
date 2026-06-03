/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Object-space cluster AABBs over an [`IndexedMesh`]'s face array.
//!
//! Faces are grouped by their position in [`IndexedMesh::faces`] into
//! fixed-size runs of [`TriangleClusters::CLUSTER_SIZE`]; each run
//! gets one [`Aabb`] covering the union of its triangle vertices.
//! Spatial-query consumers (the picker today, slicing later)
//! slab-test the cluster AABBs first and only iterate triangles
//! inside surviving clusters.
//!
//! The clustering is a pure function of the mesh's `faces` order
//! plus the referenced vertex positions. Cache lifetime is managed
//! by the surrounding [`crate::MeshSpatial`] / [`crate::SceneSpatial`]
//! layer; the type itself has no notion of staleness.

use sc_mesh_core::{Aabb, IndexedMesh, IndexedTriangle, Vertex};

/// Object-space AABBs over fixed-size runs of an [`IndexedMesh`]'s
/// faces. See the module docs for the build contract.
#[derive(Clone, Debug, PartialEq)]
pub struct TriangleClusters {
    face_count: u32,
    aabbs: Vec<Aabb>,
}

impl TriangleClusters {
    /// Number of faces packed into one cluster. Balanced between
    /// memory (one [`Aabb`] per cluster ≈ 24 B) and query speedup (a
    /// hit cluster pays a linear MT walk over up to this many
    /// faces).
    pub const CLUSTER_SIZE: u32 = 64;

    /// Build clusters from the vertex / face arrays of a mesh.
    /// Returns `None` when `faces` is empty (no clusters to emit) or
    /// when any face references an out-of-range vertex index;
    /// corrupt-mesh callers (e.g. the picker) treat this the same as
    /// "miss" rather than surfacing the error. For an in-range mesh,
    /// the result has length `ceil(faces.len() / CLUSTER_SIZE)` and
    /// the i-th cluster covers faces
    /// `[i * CLUSTER_SIZE, min((i + 1) * CLUSTER_SIZE, faces.len()))`.
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
                    let v = vertices.get(usize::from(vidx))?;
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

    /// Convenience builder over an entire [`IndexedMesh`].
    pub fn for_mesh(mesh: &IndexedMesh) -> Option<Self> {
        Self::build(&mesh.vertices, &mesh.faces)
    }

    /// Number of clusters. Always ≥ 1 for a `Some` build result and
    /// ≤ `ceil(face_count / CLUSTER_SIZE)`.
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
