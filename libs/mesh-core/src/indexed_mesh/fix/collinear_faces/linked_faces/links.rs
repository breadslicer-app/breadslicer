/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::collections::HashSet;

use crate::indexed_mesh::half_edges::{self, EdgeSlot};
#[cfg(test)]
use crate::indexed_mesh::mesh::IndexedMesh;
use crate::indexed_mesh::validated::ValidatedEdgeManifoldMesh;
use crate::primitives::{EdgeKey, FaceIndex, IndexedTriangle, Vertex, VertexIndex};

/// The edge of a face that starts at the vertex of `slot`, in the winding of
/// the face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HalfEdge {
    pub(super) face: FaceIndex,
    pub(super) slot: EdgeSlot,
}

impl HalfEdge {
    pub(super) fn next(self) -> Self {
        Self {
            slot: self.slot.next(),
            ..self
        }
    }

    pub(super) fn prev(self) -> Self {
        Self {
            slot: self.slot.prev(),
            ..self
        }
    }
}

/// The faces of an edge-manifold mesh, and the twin of each of their
/// half-edges: the half-edge of the other face on the same edge. Only the merge
/// and the split change it, and each sets every link that it changes.
pub(in crate::indexed_mesh::fix::collinear_faces) struct LinkedFaces<'a> {
    pub(super) vertices: &'a [Vertex],
    /// A copy of the faces of the mesh. A removed slot keeps its last triangle,
    /// which no link names.
    pub(super) faces: Vec<IndexedTriangle>,
    /// For each face, the twin of the half-edge at each slot.
    pub(super) twins: Vec<[HalfEdge; 3]>,
    /// The edges of the faces that are not removed.
    pub(super) edges: HashSet<EdgeKey>,
    /// The slots that merges removed.
    pub(super) removed: Vec<FaceIndex>,
}

impl<'a> LinkedFaces<'a> {
    pub(in crate::indexed_mesh::fix::collinear_faces) fn new(
        mesh: &'a ValidatedEdgeManifoldMesh<'_>,
    ) -> Self {
        let faces = mesh.faces().to_vec();
        let records = half_edges::sorted_half_edge_records(&faces);
        // Behind the witness, each edge has 2 records, next to each other.
        let (pairs, _) = records.as_chunks::<2>();
        let edges = pairs
            .iter()
            .map(|&[(key, _), _]| {
                let (a, b) = half_edges::key_vertices(key);
                EdgeKey::new(a, b)
            })
            .collect();
        // Each half-edge is in 1 pair, so the pairs replace every placeholder.
        // #[gamma::skip(literal.int_increment, reason = "replaced below")]
        let placeholder = HalfEdge {
            face: FaceIndex::from(0_usize),
            slot: EdgeSlot::V0V1,
        };
        let mut linked = Self {
            vertices: mesh.vertices(),
            twins: vec![[placeholder; 3]; faces.len()],
            faces,
            edges,
            removed: Vec::new(),
        };
        let half_edge = |value| HalfEdge {
            face: half_edges::face_of(value),
            slot: half_edges::slot_of(value),
        };
        for &[(_, first), (_, second)] in pairs {
            linked.link(half_edge(first), half_edge(second));
        }
        linked
    }

    pub(super) fn twin(&self, half_edge: HalfEdge) -> HalfEdge {
        self.twins[half_edge.face.get()][half_edge.slot.index()]
    }

    /// Returns the vertex where `half_edge` starts.
    pub(super) fn tail(&self, half_edge: HalfEdge) -> VertexIndex {
        self.faces[half_edge.face.get()].vertices[half_edge.slot.index()]
    }

    /// Returns the vertex where `half_edge` ends.
    pub(super) fn head(&self, half_edge: HalfEdge) -> VertexIndex {
        self.tail(half_edge.next())
    }

    /// Returns the vertex of the face of `half_edge` that is not on it.
    pub(super) fn apex(&self, half_edge: HalfEdge) -> VertexIndex {
        self.tail(half_edge.prev())
    }

    pub(super) fn has_edge(&self, a: VertexIndex, b: VertexIndex) -> bool {
        self.edges.contains(&EdgeKey::new(a, b))
    }

    pub(super) fn positions(&self, triangle: IndexedTriangle) -> [Vertex; 3] {
        triangle.vertices.map(|v| self.vertices[usize::from(v)])
    }

    /// Returns the face of `half_edge`, with `vertex` where `half_edge` ends.
    pub(super) fn with_head(&self, half_edge: HalfEdge, vertex: VertexIndex) -> IndexedTriangle {
        let mut triangle = self.faces[half_edge.face.get()];
        triangle.vertices[half_edge.slot.next().index()] = vertex;
        triangle
    }

    /// Makes `a` and `b` twins.
    pub(super) fn link(&mut self, a: HalfEdge, b: HalfEdge) {
        self.twins[a.face.get()][a.slot.index()] = b;
        self.twins[b.face.get()][b.slot.index()] = a;
    }

    /// Returns the faces without the removed slots, in their order.
    pub(in crate::indexed_mesh::fix::collinear_faces) fn into_faces(self) -> Vec<IndexedTriangle> {
        let mut removed = self.removed;
        removed.sort_unstable();
        self.faces
            .into_iter()
            .enumerate()
            .filter(|&(face, _)| removed.binary_search(&FaceIndex::from(face)).is_err())
            .map(|(_, triangle)| triangle)
            .collect()
    }
}

/// Returns the witness of `mesh`, which must be edge-manifold.
#[cfg(test)]
pub(in crate::indexed_mesh::fix::collinear_faces) fn edge_manifold(
    mesh: &IndexedMesh,
) -> ValidatedEdgeManifoldMesh<'_> {
    mesh.validated_faces()
        .expect("in-range indices")
        .validated_edge_manifold()
        .expect("edge-manifold")
}

/// Returns the half-edges of `raw`, each given as its face and the index of its
/// slot.
#[cfg(test)]
pub(super) fn half_edges_of(raw: [(usize, usize); 3]) -> [HalfEdge; 3] {
    raw.map(|(face, slot)| HalfEdge {
        face: FaceIndex::from(face),
        slot: EdgeSlot::ALL[slot],
    })
}

#[cfg(test)]
pub(super) fn edge_keys(raw: &[(u32, u32)]) -> HashSet<EdgeKey> {
    raw.iter()
        .map(|&(a, b)| EdgeKey::new(VertexIndex::from(a), VertexIndex::from(b)))
        .collect()
}

#[cfg(test)]
mod links_tests {
    use super::*;

    /// Returns a tetrahedron whose faces wind outward.
    fn tetrahedron() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: [
                [1.0, 1.0, 1.0],
                [1.0, -1.0, -1.0],
                [-1.0, 1.0, -1.0],
                [-1.0, -1.0, 1.0],
            ]
            .map(Vertex::new)
            .to_vec(),
            faces: [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
                .map(IndexedTriangle::from_raw_indices)
                .to_vec(),
        }
    }

    #[test]
    fn new_links_each_half_edge_of_a_tetrahedron_to_its_twin() {
        let mesh = tetrahedron();
        let witness = edge_manifold(&mesh);
        let faces = LinkedFaces::new(&witness);
        assert_eq!(faces.faces, mesh.faces);
        assert_eq!(
            faces.twins,
            [
                [(1, 2), (3, 2), (2, 0)],
                [(2, 2), (3, 0), (0, 0)],
                [(0, 2), (3, 1), (1, 0)],
                [(1, 1), (2, 1), (0, 1)],
            ]
            .map(half_edges_of)
        );
        assert_eq!(
            faces.edges,
            edge_keys(&[(0, 1), (1, 2), (0, 2), (0, 3), (1, 3), (2, 3)])
        );
        assert!(faces.removed.is_empty());
    }

    #[test]
    fn the_queries_read_the_faces_and_the_links() {
        let mesh = tetrahedron();
        let witness = edge_manifold(&mesh);
        let faces = LinkedFaces::new(&witness);
        let zero_to_one = HalfEdge {
            face: FaceIndex::from(0_usize),
            slot: EdgeSlot::V0V1,
        };
        let one_to_zero = HalfEdge {
            face: FaceIndex::from(1_usize),
            slot: EdgeSlot::V2V0,
        };
        assert_eq!(faces.twin(zero_to_one), one_to_zero);
        let ends = |half_edge| {
            [
                faces.tail(half_edge),
                faces.head(half_edge),
                faces.apex(half_edge),
            ]
            .map(u32::from)
        };
        assert_eq!(ends(zero_to_one), [0, 1, 2]);
        assert_eq!(ends(one_to_zero), [1, 0, 3]);
        assert_eq!(zero_to_one.next().slot, EdgeSlot::V1V2);
        assert_eq!(zero_to_one.prev().slot, EdgeSlot::V2V0);
        assert_eq!(
            faces.with_head(zero_to_one, VertexIndex::from(3_u32)),
            IndexedTriangle::from_raw_indices([0, 3, 2])
        );
        assert!(faces.has_edge(VertexIndex::from(3_u32), VertexIndex::from(2_u32)));
        assert!(!faces.has_edge(VertexIndex::from(0_u32), VertexIndex::from(4_u32)));
        assert_eq!(
            faces.positions(mesh.faces[3]),
            [1, 3, 2].map(|v| mesh.vertices[v])
        );
    }
}
