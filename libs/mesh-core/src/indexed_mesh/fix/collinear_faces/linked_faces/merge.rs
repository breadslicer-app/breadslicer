/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::geometry::is_degenerate_at_any_corner;
use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::{HalfEdge, LinkedFaces};
use crate::indexed_mesh::fix::records::MeshFix;
use crate::primitives::{EdgeKey, FaceIndex, IndexedTriangle};

/// The merge of a collinear face (R, P, Q) whose faces A and B have the same
/// 3rd vertex X, after its checks: A becomes (P, Q, X), and the collinear face
/// and B go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::indexed_mesh::fix::collinear_faces) struct Merge {
    /// P->Q in the collinear face.
    long_edge: HalfEdge,
    /// P->R in A.
    a: HalfEdge,
    /// R->Q in B.
    b: HalfEdge,
}

impl Merge {
    /// Returns A and B.
    pub(super) fn neighbours(self) -> [FaceIndex; 2] {
        [self.a.face, self.b.face]
    }
}

impl LinkedFaces<'_> {
    /// Returns the merge of the collinear face of `long_edge`, whose faces A
    /// and B must have the same 3rd vertex X. Returns `None` when C also has X,
    /// or when (P, Q, X) is degenerate at a corner.
    pub(super) fn merge_of(&self, long_edge: HalfEdge) -> Option<Merge> {
        let a = self.twin(long_edge.prev());
        // With S = X, the 4 faces are a closed part in 1 plane, and (P, Q, X)
        // would be glued to C on all 3 edges.
        if self.apex(self.twin(long_edge)) == self.apex(a) {
            return None;
        }
        // (P, Q, X) has the outline of the 3 faces it replaces, so it faces
        // their way when it is not degenerate.
        if is_degenerate_at_any_corner(&self.positions(self.merged(long_edge))) {
            return None;
        }
        Some(Merge {
            long_edge,
            a,
            b: self.twin(long_edge.next()),
        })
    }

    /// Writes (P, Q, X) into the slot of A, removes the collinear face and B,
    /// and returns the record.
    pub(in crate::indexed_mesh::fix::collinear_faces) fn merge_neighbours(
        &mut self,
        merge: Merge,
    ) -> MeshFix {
        let Merge { long_edge, a, b } = merge;
        let [p, q, r] = [
            self.tail(long_edge),
            self.head(long_edge),
            self.apex(long_edge),
        ];
        let x = self.apex(a);
        let c = self.twin(long_edge);
        // In the face beyond B on Q-X.
        let beyond_b = self.twin(b.next());
        let collinear = self.faces[long_edge.face.get()];
        let merged_neighbours = [a.face, b.face].map(|face| self.faces[face.get()]);
        let merged = self.merged(long_edge);
        self.faces[a.face.get()] = merged;
        // `a` is now P->Q and `a.next()` Q->X; X->P keeps its link.
        self.link(a, c);
        self.link(a.next(), beyond_b);
        for (u, v) in [(r, p), (q, r), (r, x)] {
            self.edges.remove(&EdgeKey::new(u, v));
        }
        self.removed.extend([long_edge.face, b.face]);
        MeshFix::RemovedCollinearFaceByMerge {
            collinear,
            merged_neighbours,
            merged,
        }
    }

    /// Returns A with Q in the place of R: (P, Q, X), in the rotation of A.
    fn merged(&self, long_edge: HalfEdge) -> IndexedTriangle {
        self.with_head(self.twin(long_edge.prev()), self.head(long_edge))
    }
}

#[cfg(test)]
mod merge_neighbours_tests {
    use super::*;
    use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::{
        edge_keys, edge_manifold, half_edges_of,
    };
    use crate::indexed_mesh::fix::collinear_faces::test_meshes::{
        SPLIT_TETRAHEDRON, split_tetrahedron,
    };
    use crate::indexed_mesh::half_edges::EdgeSlot;

    fn triangle(vertices: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(vertices)
    }

    #[test]
    fn a_merge_links_a_to_c_and_to_the_face_beyond_b() {
        let mesh = split_tetrahedron(SPLIT_TETRAHEDRON);
        let witness = edge_manifold(&mesh);
        let mut faces = LinkedFaces::new(&witness);
        // P->Q in the collinear face (0, 1, 2).
        let long_edge = HalfEdge {
            face: FaceIndex::from(0_usize),
            slot: EdgeSlot::V0V1,
        };
        let merge = faces.merge_of(long_edge).expect("a merge");
        assert_eq!(
            faces.merge_neighbours(merge),
            MeshFix::RemovedCollinearFaceByMerge {
                collinear: triangle([0, 1, 2]),
                merged_neighbours: [triangle([0, 2, 3]), triangle([2, 1, 3])],
                merged: triangle([0, 1, 3]),
            }
        );
        // Faces 0 and 2 are removed.
        assert_eq!(
            [1, 3, 4, 5].map(|face| faces.twins[face]),
            [
                [(3, 2), (5, 2), (4, 0)],
                [(4, 2), (5, 0), (1, 0)],
                [(1, 2), (5, 1), (3, 0)],
                [(3, 1), (4, 1), (1, 1)],
            ]
            .map(half_edges_of)
        );
        assert_eq!(
            faces.edges,
            edge_keys(&[(0, 1), (0, 3), (1, 3), (0, 4), (1, 4), (3, 4)])
        );
        assert_eq!(
            faces.into_faces(),
            [[0, 1, 3], [0, 4, 1], [0, 3, 4], [1, 4, 3]].map(triangle)
        );
    }
}
