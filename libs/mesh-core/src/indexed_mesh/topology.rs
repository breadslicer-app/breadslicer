/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Topological indices over a [`ValidatedManifoldMesh`]:
//!
//! 1. `edge_to_faces`: every interior edge maps to the two faces
//!    incident to it. The manifold witness guarantees exactly two
//!    per edge.
//! 2. `vertex_one_ring`: every vertex maps to its incident faces in
//!    cyclic order, walked via the edge-to-face adjacency. The
//!    direction of the cycle is implementation-defined (the manifold
//!    is orientable, so the walk is well-formed, but the choice of
//!    starting direction is unobservable from the public API). The
//!    only invariant consumers should rely on is "consecutive entries
//!    share an edge through the vertex."
//!
//! Both indices are built once from the witness; lookups are
//! `HashMap` reads.

use std::collections::HashMap;

use crate::primitives::{FaceIndex, VertexIndex};

use super::ValidatedManifoldMesh;

/// Canonical key for an undirected mesh edge: `(lo, hi)` with
/// `lo <= hi`. Constructed via [`edge_key`].
pub type EdgeKey = (VertexIndex, VertexIndex);

/// Canonicalise an unordered vertex pair into an [`EdgeKey`].
#[inline]
pub fn edge_key(a: VertexIndex, b: VertexIndex) -> EdgeKey {
    if a <= b { (a, b) } else { (b, a) }
}

/// Precomputed topology over a manifold mesh.
///
/// Built once via [`MeshTopology::from_manifold`]; lookups against
/// the two indices are constant time. The struct does not borrow the
/// mesh, so the witness it was built from can be dropped.
#[derive(Debug, Clone)]
pub struct MeshTopology {
    edge_to_faces: HashMap<EdgeKey, [FaceIndex; 2]>,
    vertex_one_ring: HashMap<VertexIndex, Vec<FaceIndex>>,
}

impl MeshTopology {
    /// Build both indices from a manifold-validated mesh.
    pub fn from_manifold(mesh: &ValidatedManifoldMesh<'_>) -> Self {
        let edge_to_faces = build_edge_to_faces(mesh);
        let vertex_one_ring = build_vertex_one_ring(mesh, &edge_to_faces);
        Self {
            edge_to_faces,
            vertex_one_ring,
        }
    }

    /// The two faces incident to the mesh edge between `a` and `b`,
    /// or `None` if no such edge exists in the mesh. Order within the
    /// returned pair is implementation-defined; if your code needs a
    /// specific role for each, distinguish them by looking up
    /// `current_face` vs the other.
    #[inline]
    pub fn edge_faces(&self, a: VertexIndex, b: VertexIndex) -> Option<[FaceIndex; 2]> {
        self.edge_to_faces.get(&edge_key(a, b)).copied()
    }

    /// The faces incident to `vertex` in cyclic order, or `None` if
    /// the vertex is not part of any face. Consecutive entries share
    /// a mesh edge through `vertex`; the last entry shares one with
    /// the first.
    #[inline]
    pub fn vertex_one_ring(&self, vertex: VertexIndex) -> Option<&[FaceIndex]> {
        self.vertex_one_ring.get(&vertex).map(Vec::as_slice)
    }

    /// Number of edges in the indexed mesh. Useful for tests; not
    /// part of the slicer's public hot path.
    pub fn edge_count(&self) -> usize {
        self.edge_to_faces.len()
    }
}

fn build_edge_to_faces(mesh: &ValidatedManifoldMesh<'_>) -> HashMap<EdgeKey, [FaceIndex; 2]> {
    let mut acc: HashMap<EdgeKey, Vec<FaceIndex>> = HashMap::new();
    for (idx, face) in mesh.faces().iter().enumerate() {
        let face_idx = FaceIndex::from(idx);
        let [v0, v1, v2] = face.vertices;
        for &(a, b) in &[(v0, v1), (v1, v2), (v2, v0)] {
            acc.entry(edge_key(a, b)).or_default().push(face_idx);
        }
    }
    acc.into_iter()
        .map(|(k, v)| {
            assert_eq!(
                v.len(),
                2,
                "manifold witness violated for edge {k:?}: {} incident faces",
                v.len(),
            );
            (k, [v[0], v[1]])
        })
        .collect()
}

fn build_vertex_one_ring(
    mesh: &ValidatedManifoldMesh<'_>,
    edge_to_faces: &HashMap<EdgeKey, [FaceIndex; 2]>,
) -> HashMap<VertexIndex, Vec<FaceIndex>> {
    let mut incident: HashMap<VertexIndex, Vec<FaceIndex>> = HashMap::new();
    for (idx, face) in mesh.faces().iter().enumerate() {
        let face_idx = FaceIndex::from(idx);
        for &v in &face.vertices {
            incident.entry(v).or_default().push(face_idx);
        }
    }

    let mut result: HashMap<VertexIndex, Vec<FaceIndex>> = HashMap::new();
    for (vertex, incident_faces) in incident {
        let cyclic = walk_cyclic_one_ring(mesh, vertex, &incident_faces, edge_to_faces);
        assert_eq!(
            cyclic.len(),
            incident_faces.len(),
            "cyclic walk around vertex {vertex:?} visited {} of {} incident faces; expected the witness to guarantee a full cycle",
            cyclic.len(),
            incident_faces.len(),
        );
        result.insert(vertex, cyclic);
    }
    result
}

fn walk_cyclic_one_ring(
    mesh: &ValidatedManifoldMesh<'_>,
    vertex: VertexIndex,
    incident: &[FaceIndex],
    edge_to_faces: &HashMap<EdgeKey, [FaceIndex; 2]>,
) -> Vec<FaceIndex> {
    let mut cyclic = Vec::with_capacity(incident.len());
    let start = incident[0];
    let mut current = start;
    let mut prev_edge: Option<EdgeKey> = None;

    loop {
        cyclic.push(current);

        let face = &mesh.faces()[current.get()];
        let [v0, v1, v2] = face.vertices;
        let mut through_vertex: [Option<EdgeKey>; 2] = [None, None];
        let mut written = 0;
        for &(a, b) in &[(v0, v1), (v1, v2), (v2, v0)] {
            if a == vertex || b == vertex {
                through_vertex[written] = Some(edge_key(a, b));
                written += 1;
            }
        }
        debug_assert_eq!(
            written, 2,
            "face {current:?} listed as incident to vertex {vertex:?} but has {written} edges through it",
        );

        let edges = [through_vertex[0].unwrap(), through_vertex[1].unwrap()];
        let next_edge = match prev_edge {
            None => edges[0],
            Some(p) => *edges
                .iter()
                .find(|e| **e != p)
                .expect("no other edge through vertex"),
        };

        let [f1, f2] = edge_to_faces[&next_edge];
        let next = if f1 == current { f2 } else { f1 };

        if next == start {
            break;
        }
        prev_edge = Some(next_edge);
        current = next;

        if cyclic.len() > incident.len() {
            panic!(
                "cyclic walk around vertex {vertex:?} did not terminate within {} steps; topology is not a closed disk fan",
                incident.len(),
            );
        }
    }

    cyclic
}

#[cfg(test)]
mod edge_key_tests {
    use super::*;

    fn vi(i: u32) -> VertexIndex {
        VertexIndex::from(i)
    }

    #[test]
    fn edge_key_canonicalises_to_lo_hi() {
        let forward = edge_key(vi(3), vi(7));
        let reverse = edge_key(vi(7), vi(3));
        assert_eq!(forward, reverse);
        assert_eq!(forward, (vi(3), vi(7)));
    }

    #[test]
    fn edge_key_equal_endpoints_collapses_cleanly() {
        let degenerate = edge_key(vi(5), vi(5));
        assert_eq!(degenerate, (vi(5), vi(5)));
    }
}

#[cfg(test)]
mod mesh_topology_tests {
    use crate::indexed_mesh::IndexedMesh;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};

    use super::*;

    fn vi(i: u32) -> VertexIndex {
        VertexIndex::from(i)
    }

    fn face(v: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), v)
    }

    /// Same tetrahedron used in `indexed_mesh::validated`'s manifold
    /// tests: consistent outward winding.
    ///
    /// Vertices:
    ///   0: (1, 1, 1)
    ///   1: (1, -1, -1)
    ///   2: (-1, 1, -1)
    ///   3: (-1, -1, 1)
    ///
    /// Faces:
    ///   F0: [0, 1, 2] (edges 0-1, 1-2, 0-2)
    ///   F1: [0, 3, 1] (edges 0-3, 1-3, 0-1)
    ///   F2: [0, 2, 3] (edges 0-2, 2-3, 0-3)
    ///   F3: [1, 3, 2] (edges 1-3, 2-3, 1-2)
    fn tetrahedron() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 1.0, 1.0]),
                Vertex::new([1.0, -1.0, -1.0]),
                Vertex::new([-1.0, 1.0, -1.0]),
                Vertex::new([-1.0, -1.0, 1.0]),
            ],
            faces: vec![
                face([0, 1, 2]),
                face([0, 3, 1]),
                face([0, 2, 3]),
                face([1, 3, 2]),
            ],
        }
    }

    fn topology(mesh: &IndexedMesh) -> MeshTopology {
        let witness = mesh
            .validated_faces()
            .expect("tetrahedron has in-range vertex indices")
            .validated_manifold()
            .expect("tetrahedron is a closed manifold");
        MeshTopology::from_manifold(&witness)
    }

    #[test]
    fn edge_count_matches_tetrahedron_edge_count() {
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        assert_eq!(topo.edge_count(), 6);
    }

    #[test]
    fn edge_faces_returns_two_for_each_tetrahedron_edge() {
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        let edges = [(0u32, 1u32), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
        for (a, b) in edges {
            let pair = topo.edge_faces(vi(a), vi(b)).unwrap_or_else(|| {
                panic!("edge ({a}, {b}) missing from edge_to_faces");
            });
            assert_ne!(
                pair[0], pair[1],
                "edge ({a}, {b}) maps to a single face twice"
            );
        }
    }

    #[test]
    fn edge_faces_is_unordered_pair_lookup() {
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        let forward = topo.edge_faces(vi(0), vi(1)).unwrap();
        let reverse = topo.edge_faces(vi(1), vi(0)).unwrap();
        assert!(
            (forward[0] == reverse[0] && forward[1] == reverse[1])
                || (forward[0] == reverse[1] && forward[1] == reverse[0]),
            "edge_faces should yield the same unordered pair regardless of argument order: forward={forward:?}, reverse={reverse:?}",
        );
    }

    #[test]
    fn edge_faces_returns_none_for_non_edges() {
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        assert!(topo.edge_faces(vi(0), vi(99)).is_none());
    }

    #[test]
    fn vertex_one_ring_length_matches_vertex_degree_for_tetrahedron() {
        // Each vertex of a tetrahedron is incident to exactly three faces.
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        for v in 0u32..4 {
            let ring = topo.vertex_one_ring(vi(v)).unwrap_or_else(|| {
                panic!("vertex {v} not present in vertex_one_ring");
            });
            assert_eq!(
                ring.len(),
                3,
                "vertex {v} expected 3 incident faces, got {}",
                ring.len(),
            );
        }
    }

    #[test]
    fn vertex_one_ring_consecutive_faces_share_an_edge_through_the_vertex() {
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        for v in 0u32..4 {
            let ring = topo.vertex_one_ring(vi(v)).unwrap();
            for i in 0..ring.len() {
                let curr = ring[i];
                let next = ring[(i + 1) % ring.len()];
                let shared_edge_exists = mesh.faces[curr.get()].vertices.iter().any(|&w| {
                    w != vi(v) && mesh.faces[next.get()].vertices.contains(&w) && {
                        let pair = topo.edge_faces(vi(v), w).unwrap();
                        (pair[0] == curr && pair[1] == next) || (pair[0] == next && pair[1] == curr)
                    }
                });
                assert!(
                    shared_edge_exists,
                    "ring entries {curr:?} and {next:?} around vertex {v} do not share an edge through it",
                );
            }
        }
    }

    #[test]
    fn vertex_one_ring_visits_every_incident_face_exactly_once() {
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        for v in 0u32..4 {
            let ring = topo.vertex_one_ring(vi(v)).unwrap();
            let mut seen: std::collections::HashSet<FaceIndex> = std::collections::HashSet::new();
            for &f in ring {
                assert!(
                    seen.insert(f),
                    "face {f:?} appears twice in ring of vertex {v}"
                );
            }
        }
    }

    #[test]
    fn vertex_one_ring_returns_none_for_unreferenced_vertex() {
        // Add a 5th vertex with no incident faces.
        let mut mesh = tetrahedron();
        mesh.vertices.push(Vertex::new([0.0, 0.0, 5.0]));
        let topo = topology(&mesh);
        assert!(topo.vertex_one_ring(vi(4)).is_none());
    }
}
