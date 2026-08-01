/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Topological indices over a [`ValidatedEdgeManifoldMesh`]:
//!
//! 1. The edge table: every edge maps to the two faces incident to
//!    it. The edge-manifold witness guarantees exactly two per edge.
//! 2. The vertex fans: every vertex maps to its incident faces,
//!    partitioned into cyclic fans walked via the edge-to-face
//!    adjacency. A vertex whose neighbourhood is a disk has exactly
//!    one fan; a pinch vertex (2 or more surface lobes touching at
//!    a point, which the edge-level witness accepts) has one fan
//!    per lobe. The direction of each cycle is
//!    implementation-defined (the surface is orientable, so the
//!    walk is well-formed, but the choice of starting direction is
//!    unobservable from the public API). The only invariants
//!    consumers should rely on are "consecutive entries of a fan
//!    share an edge through the vertex" and "fans partition the
//!    vertex's incident faces."
//!
//! Both indices are built once from the witness, from a single pass
//! over the sorted directed half-edge records shared with the
//! edge-manifold validator: the edge table comes out as a sorted
//! vector (edge lookups binary-search it) and the fans as one flat
//! array sliced per fan, with a per-vertex index over the fans.

use std::collections::HashSet;

use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::primitives::{FaceIndex, IndexedTriangle, VertexIndex};

use super::{ValidatedEdgeManifoldMesh, half_edges};

/// Canonical key for an undirected mesh edge: `(lo, hi)` with
/// `lo <= hi`. Constructed via [`edge_key`].
pub type EdgeKey = (VertexIndex, VertexIndex);

/// Canonicalise an unordered vertex pair into an [`EdgeKey`].
#[inline]
pub fn edge_key(a: VertexIndex, b: VertexIndex) -> EdgeKey {
    if a <= b { (a, b) } else { (b, a) }
}

/// Precomputed topology over an edge-manifold mesh.
///
/// Built once via [`MeshTopology::from_edge_manifold`]; edge lookups
/// binary-search the sorted edge table, fan lookups slice a flat
/// array through a per-vertex fan index. The struct does not borrow
/// the mesh, so the witness it was built from can be dropped.
#[derive(Debug, Clone)]
pub struct MeshTopology {
    /// Sorted by packed canonical edge key; each entry carries the
    /// two incident faces, ascending.
    edges: Vec<(u64, [FaceIndex; 2])>,
    /// Every vertex's incident faces, fan by fan: fan `k` is
    /// `ring_faces[fan_bounds[k]..fan_bounds[k + 1]]`, cyclic.
    ring_faces: Vec<FaceIndex>,
    /// CSR bounds into `ring_faces`, one entry per fan plus a final
    /// sentinel equal to `ring_faces.len()`.
    fan_bounds: Vec<usize>,
    /// CSR bounds into `fan_bounds`: vertex `v` owns the fan indices
    /// `vertex_fan_ranges[v]..vertex_fan_ranges[v + 1]`.
    vertex_fan_ranges: Vec<usize>,
}

impl MeshTopology {
    /// Build both indices from an edge-manifold-validated mesh.
    pub fn from_edge_manifold(mesh: &ValidatedEdgeManifoldMesh<'_>) -> Self {
        let faces = mesh.faces();
        let records = half_edges::sorted_half_edge_records(faces);

        // One scan over the sorted runs yields the edge table and the
        // per-face neighbor across each edge slot; the latter is what
        // the ring walks step through.
        let mut edges: Vec<(u64, [FaceIndex; 2])> = Vec::with_capacity(records.len() / 2);
        let mut face_adj: Vec<[FaceIndex; 3]> = vec![[FaceIndex::from(0_usize); 3]; faces.len()];
        for run in records.chunk_by(|lhs, rhs| lhs.0 == rhs.0) {
            assert_eq!(
                run.len(),
                2,
                "edge-manifold witness violated for edge {:?}: {} incident faces",
                half_edges::key_vertices(run[0].0),
                run.len(),
            );
            let f0 = half_edges::face_of(run[0].1);
            let f1 = half_edges::face_of(run[1].1);
            edges.push((run[0].0, [f0, f1]));
            face_adj[f0.get()][half_edges::slot_of(run[0].1)] = f1;
            face_adj[f1.get()][half_edges::slot_of(run[1].1)] = f0;
        }

        // Incident faces per vertex, CSR, ascending face order (so
        // each ring walk starts from the lowest incident face).
        let vertex_count = mesh.vertices().len();
        let mut ring_offsets = vec![0usize; vertex_count + 1];
        for face in faces {
            for &v in &face.vertices {
                ring_offsets[usize::from(v) + 1] += 1;
            }
        }
        for k in 1..ring_offsets.len() {
            ring_offsets[k] += ring_offsets[k - 1];
        }
        let mut incident = vec![FaceIndex::from(0_usize); ring_offsets[vertex_count]];
        let mut cursors = ring_offsets[..vertex_count].to_vec();
        for (idx, face) in faces.iter().enumerate() {
            for &v in &face.vertices {
                let cursor = &mut cursors[usize::from(v)];
                incident[*cursor] = FaceIndex::from(idx);
                *cursor += 1;
            }
        }

        // Partition every vertex's incident faces into cyclic fans.
        // Each incident face has exactly 2 edges through the vertex
        // and, per the witness, exactly 1 neighbour across each, so
        // the incident faces split into disjoint simple cycles: one
        // fan for a disk neighbourhood, several for a pinch vertex.
        let fans_per_vertex: Vec<Vec<Vec<FaceIndex>>> = (0..vertex_count)
            .into_par_iter()
            .map(|v| {
                let inc = &incident[ring_offsets[v]..ring_offsets[v + 1]];
                walk_cyclic_fans(faces, VertexIndex::from(v as u32), inc, &face_adj)
            })
            .collect();

        let mut ring_faces = Vec::with_capacity(ring_offsets[vertex_count]);
        let mut fan_bounds = vec![0usize];
        let mut vertex_fan_ranges = Vec::with_capacity(vertex_count + 1);
        vertex_fan_ranges.push(0);
        for fans in &fans_per_vertex {
            for fan in fans {
                ring_faces.extend_from_slice(fan);
                fan_bounds.push(ring_faces.len());
            }
            vertex_fan_ranges.push(fan_bounds.len() - 1);
        }

        Self {
            edges,
            ring_faces,
            fan_bounds,
            vertex_fan_ranges,
        }
    }

    /// The two faces incident to the mesh edge between `a` and `b`,
    /// or `None` if no such edge exists in the mesh. Order within the
    /// returned pair is implementation-defined; if your code needs a
    /// specific role for each, distinguish them by looking up
    /// `current_face` vs the other.
    #[inline]
    pub fn edge_faces(&self, a: VertexIndex, b: VertexIndex) -> Option<[FaceIndex; 2]> {
        let key = half_edges::key(a, b);
        self.edges
            .binary_search_by_key(&key, |&(k, _)| k)
            .ok()
            .map(|found| self.edges[found].1)
    }

    /// The faces incident to `vertex`, grouped into cyclic fans, or
    /// `None` if the vertex is not part of any face. Within a fan,
    /// consecutive entries share a mesh edge through `vertex` and
    /// the last entry shares one with the first. A vertex whose
    /// neighbourhood is a disk has exactly 1 fan; a pinch vertex
    /// has 1 fan per surface lobe touching it, and the fans
    /// partition its incident faces.
    #[inline]
    pub fn vertex_fans(
        &self,
        vertex: VertexIndex,
    ) -> Option<impl Iterator<Item = &[FaceIndex]> + '_> {
        let v = usize::from(vertex);
        if v + 1 >= self.vertex_fan_ranges.len() {
            return None;
        }
        let fans = self.vertex_fan_ranges[v]..self.vertex_fan_ranges[v + 1];
        if fans.is_empty() {
            return None;
        }
        Some(fans.map(move |k| &self.ring_faces[self.fan_bounds[k]..self.fan_bounds[k + 1]]))
    }

    /// Number of edges in the indexed mesh. Useful for tests; not
    /// part of the slicer's public hot path.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

/// Partition `vertex`'s incident faces into their cyclic fans by
/// stepping across the shared edge through the vertex: start at the
/// lowest unvisited incident face, leave through its first edge slot
/// touching the vertex, and keep crossing into the neighbor until
/// the walk returns to its start face; repeat until every incident
/// face belongs to a fan. Each incident face has exactly 2 edges
/// through `vertex` and, per the witness, exactly 1 neighbor across
/// each, so the incident faces form disjoint simple cycles and every
/// walk closes at its start.
fn walk_cyclic_fans(
    faces: &[IndexedTriangle],
    vertex: VertexIndex,
    incident: &[FaceIndex],
    face_adj: &[[FaceIndex; 3]],
) -> Vec<Vec<FaceIndex>> {
    let mut fans = Vec::new();
    let mut visited: HashSet<FaceIndex> = HashSet::with_capacity(incident.len());

    for &start in incident {
        if visited.contains(&start) {
            continue;
        }
        let mut fan = Vec::new();
        let mut current = start;
        let mut prev_edge: Option<u64> = None;

        loop {
            fan.push(current);
            visited.insert(current);

            let [v0, v1, v2] = faces[current.get()].vertices;
            let mut through_vertex: [(u64, usize); 2] = [(0, 0); 2];
            let mut written = 0;
            for (slot, &(a, b)) in [(v0, v1), (v1, v2), (v2, v0)].iter().enumerate() {
                if a == vertex || b == vertex {
                    through_vertex[written] = (half_edges::key(a, b), slot);
                    written += 1;
                }
            }
            debug_assert_eq!(
                written, 2,
                "face {current:?} listed as incident to vertex {vertex:?} but has {written} edges through it",
            );

            let (next_edge, next_slot) = match prev_edge {
                None => through_vertex[0],
                Some(p) => {
                    if through_vertex[0].0 != p {
                        through_vertex[0]
                    } else {
                        through_vertex[1]
                    }
                }
            };

            let next = face_adj[current.get()][next_slot];
            if next == start {
                break;
            }
            prev_edge = Some(next_edge);
            current = next;
        }

        fans.push(fan);
    }

    fans
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

    /// Two tetrahedra sharing vertex 0 (a bowtie). The second lobe
    /// is the point reflection of the first through vertex 0, with
    /// winding reversed so both lobes stay consistently wound. The
    /// mesh is edge-manifold (every edge has exactly 2 faces), but
    /// vertex 0's 6 incident faces form 2 disjoint fans.
    fn bowtie() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 1.0, 1.0]),
                Vertex::new([1.0, -1.0, -1.0]),
                Vertex::new([-1.0, 1.0, -1.0]),
                Vertex::new([-1.0, -1.0, 1.0]),
                Vertex::new([1.0, 3.0, 3.0]),
                Vertex::new([3.0, 1.0, 3.0]),
                Vertex::new([3.0, 3.0, 1.0]),
            ],
            faces: vec![
                face([0, 1, 2]),
                face([0, 3, 1]),
                face([0, 2, 3]),
                face([1, 3, 2]),
                face([0, 5, 4]),
                face([0, 4, 6]),
                face([0, 6, 5]),
                face([4, 5, 6]),
            ],
        }
    }

    fn topology(mesh: &IndexedMesh) -> MeshTopology {
        let witness = mesh
            .validated_faces()
            .expect("fixture has in-range vertex indices")
            .validated_edge_manifold()
            .expect("fixture is edge-manifold with consistent winding");
        MeshTopology::from_edge_manifold(&witness)
    }

    #[test]
    fn from_edge_manifold_accepts_the_bowtie() {
        // Regression: the bowtie mints the manifold witness, so the
        // topology build must succeed; the pinch vertex simply has
        // more than one fan.
        let mesh = bowtie();
        let topo = topology(&mesh);
        assert_eq!(topo.edge_count(), 12);
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

    /// Assert the cyclic invariant for every fan of `v`: consecutive
    /// entries (wrapping around) share a mesh edge through `v`.
    fn assert_fans_are_cyclic(mesh: &IndexedMesh, topo: &MeshTopology, v: u32) {
        let fans: Vec<&[FaceIndex]> = topo
            .vertex_fans(vi(v))
            .unwrap_or_else(|| panic!("vertex {v} not present in vertex_fans"))
            .collect();
        for fan in fans {
            for i in 0..fan.len() {
                let curr = fan[i];
                let next = fan[(i + 1) % fan.len()];
                let shared_edge_exists = mesh.faces[curr.get()].vertices.iter().any(|&w| {
                    w != vi(v) && mesh.faces[next.get()].vertices.contains(&w) && {
                        let pair = topo.edge_faces(vi(v), w).unwrap();
                        (pair[0] == curr && pair[1] == next) || (pair[0] == next && pair[1] == curr)
                    }
                });
                assert!(
                    shared_edge_exists,
                    "fan entries {curr:?} and {next:?} around vertex {v} do not share an edge through it",
                );
            }
        }
    }

    #[test]
    fn vertex_fans_yield_a_single_fan_of_three_for_the_tetrahedron() {
        // Each tetrahedron vertex has a disk neighbourhood: exactly
        // one fan holding its three incident faces.
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        for v in 0u32..4 {
            let fans: Vec<&[FaceIndex]> = topo
                .vertex_fans(vi(v))
                .unwrap_or_else(|| panic!("vertex {v} not present in vertex_fans"))
                .collect();
            assert_eq!(fans.len(), 1, "vertex {v} expected a single fan");
            assert_eq!(fans[0].len(), 3, "vertex {v} expected 3 incident faces");
        }
    }

    #[test]
    fn vertex_fans_consecutive_faces_share_an_edge_through_the_vertex() {
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        for v in 0u32..4 {
            assert_fans_are_cyclic(&mesh, &topo, v);
        }
    }

    #[test]
    fn vertex_fans_partition_incident_faces_without_repeats() {
        let mesh = tetrahedron();
        let topo = topology(&mesh);
        for v in 0u32..4 {
            let mut seen: HashSet<FaceIndex> = HashSet::new();
            for fan in topo.vertex_fans(vi(v)).unwrap() {
                for &f in fan {
                    assert!(
                        seen.insert(f),
                        "face {f:?} appears twice across fans of vertex {v}"
                    );
                }
            }
        }
    }

    #[test]
    fn vertex_fans_returns_none_for_unreferenced_vertex() {
        // Add a 5th vertex with no incident faces.
        let mut mesh = tetrahedron();
        mesh.vertices.push(Vertex::new([0.0, 0.0, 5.0]));
        let topo = topology(&mesh);
        assert!(topo.vertex_fans(vi(4)).is_none());
    }

    #[test]
    fn bowtie_pinch_vertex_has_two_lobe_fans_and_others_have_one() {
        let mesh = bowtie();
        let topo = topology(&mesh);
        let fan_sets: Vec<HashSet<usize>> = topo
            .vertex_fans(vi(0))
            .expect("pinch vertex has incident faces")
            .map(|fan| fan.iter().map(|f| f.get()).collect())
            .collect();
        assert_eq!(fan_sets.len(), 2, "pinch vertex expected 2 fans");
        // Faces 0..2 touch vertex 0 in the lower lobe, 4..6 in the
        // upper lobe (faces 3 and 7 are the opposite bases).
        let lower: HashSet<usize> = [0, 1, 2].into();
        let upper: HashSet<usize> = [4, 5, 6].into();
        assert!(
            (fan_sets[0] == lower && fan_sets[1] == upper)
                || (fan_sets[0] == upper && fan_sets[1] == lower),
            "fans {fan_sets:?} do not match the bowtie lobes"
        );
        for v in 0u32..7 {
            assert_fans_are_cyclic(&mesh, &topo, v);
        }
        for v in 1u32..7 {
            let count = topo.vertex_fans(vi(v)).unwrap().count();
            assert_eq!(count, 1, "vertex {v} expected a single fan");
        }
    }
}

#[cfg(test)]
mod sorted_topology_equivalence_tests {
    use std::collections::HashMap;

    use proptest::prelude::*;

    use crate::indexed_mesh::IndexedMesh;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};

    use super::*;

    /// The previous hash-based topology builder, kept verbatim as the
    /// reference the sorted implementation must reproduce bit for bit
    /// (edge pairs including order, rings including order).
    struct ReferenceTopology {
        edge_to_faces: HashMap<EdgeKey, [FaceIndex; 2]>,
        vertex_one_ring: HashMap<VertexIndex, Vec<FaceIndex>>,
    }

    fn reference_topology(mesh: &ValidatedEdgeManifoldMesh<'_>) -> ReferenceTopology {
        let edge_to_faces = reference_edge_to_faces(mesh);
        let vertex_one_ring = reference_vertex_one_ring(mesh, &edge_to_faces);
        ReferenceTopology {
            edge_to_faces,
            vertex_one_ring,
        }
    }

    fn reference_edge_to_faces(
        mesh: &ValidatedEdgeManifoldMesh<'_>,
    ) -> HashMap<EdgeKey, [FaceIndex; 2]> {
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
                assert_eq!(v.len(), 2, "reference: manifold witness violated");
                (k, [v[0], v[1]])
            })
            .collect()
    }

    fn reference_vertex_one_ring(
        mesh: &ValidatedEdgeManifoldMesh<'_>,
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
            let cyclic = reference_walk(mesh, vertex, &incident_faces, edge_to_faces);
            assert_eq!(cyclic.len(), incident_faces.len(), "reference: open fan");
            result.insert(vertex, cyclic);
        }
        result
    }

    fn reference_walk(
        mesh: &ValidatedEdgeManifoldMesh<'_>,
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

            let edges = [through_vertex[0].unwrap(), through_vertex[1].unwrap()];
            let next_edge = match prev_edge {
                None => edges[0],
                Some(p) => *edges.iter().find(|e| **e != p).expect("other edge"),
            };

            let [f1, f2] = edge_to_faces[&next_edge];
            let next = if f1 == current { f2 } else { f1 };

            if next == start {
                break;
            }
            prev_edge = Some(next_edge);
            current = next;
            assert!(cyclic.len() <= incident.len(), "reference: runaway walk");
        }

        cyclic
    }

    fn face(v: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), v)
    }

    /// Closed grid torus, consistent winding; same construction as
    /// the validator's equivalence tests.
    fn torus_mesh(n: usize, m: usize) -> IndexedMesh {
        let mut vertices = Vec::with_capacity(n * m);
        for i in 0..n {
            for j in 0..m {
                vertices.push(Vertex::new([i as f32, j as f32, (i + j) as f32]));
            }
        }
        let idx = |i: usize, j: usize| -> u32 { (i * m + j) as u32 };
        let mut faces = Vec::with_capacity(2 * n * m);
        for i in 0..n {
            let i1 = (i + 1) % n;
            for j in 0..m {
                let j1 = (j + 1) % m;
                faces.push(face([idx(i, j), idx(i1, j), idx(i1, j1)]));
                faces.push(face([idx(i, j), idx(i1, j1), idx(i, j1)]));
            }
        }
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

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

    fn assert_matches_reference(mesh: &IndexedMesh) {
        let witness = mesh
            .validated_faces()
            .expect("in-range faces")
            .validated_edge_manifold()
            .expect("manifold mesh");
        let topo = MeshTopology::from_edge_manifold(&witness);
        let reference = reference_topology(&witness);

        assert_eq!(topo.edge_count(), reference.edge_to_faces.len());
        for (&(a, b), &pair) in &reference.edge_to_faces {
            assert_eq!(
                topo.edge_faces(a, b),
                Some(pair),
                "edge ({a:?}, {b:?}): pair mismatch",
            );
        }
        for v in 0..mesh.vertices.len() as u32 {
            let vertex = VertexIndex::from(v);
            let expected = reference.vertex_one_ring.get(&vertex).map(Vec::as_slice);
            let fans: Option<Vec<&[FaceIndex]>> = topo.vertex_fans(vertex).map(Iterator::collect);
            match (fans, expected) {
                (None, None) => {}
                (Some(fans), Some(ring)) => {
                    // The reference meshes are vertex-manifold, so the
                    // fan partition must be a single fan reproducing
                    // the reference ring bit for bit.
                    assert_eq!(fans.len(), 1, "vertex {v}: expected a single fan");
                    assert_eq!(fans[0], ring, "vertex {v}: fan mismatch");
                }
                (fans, expected) => {
                    panic!("vertex {v}: presence mismatch (got {fans:?}, expected {expected:?})")
                }
            }
        }
    }

    #[test]
    fn matches_the_reference_on_the_tetrahedron() {
        assert_matches_reference(&tetrahedron());
    }

    #[test]
    fn matches_the_reference_on_a_grid_torus() {
        assert_matches_reference(&torus_mesh(5, 7));
    }

    proptest! {
        #[test]
        fn matches_the_reference_on_generated_tori(n in 3usize..8, m in 3usize..8) {
            assert_matches_reference(&torus_mesh(n, m));
        }
    }
}
