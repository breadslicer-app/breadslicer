/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::geometry::{cross_of_edges_f64, is_degenerate_at_any_corner};
use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::{HalfEdge, LinkedFaces};
use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::half_edges::EdgeSlot;
use crate::primitives::{EdgeKey, FaceIndex, IndexedTriangle};

/// The split of the face C across the long edge of a collinear face (R, P, Q),
/// after its checks: with S the 3rd vertex of C, (R, P, S) takes the place of
/// the collinear face, and C becomes (Q, R, S).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::indexed_mesh::fix::collinear_faces) struct Split {
    /// P->Q in the collinear face.
    long_edge: HalfEdge,
    /// P->R in A.
    a: HalfEdge,
    /// R->Q in B.
    b: HalfEdge,
    /// Q->P in C.
    c: HalfEdge,
}

impl Split {
    /// Returns C.
    pub(super) fn opposite(self) -> FaceIndex {
        self.c.face
    }
}

impl LinkedFaces<'_> {
    /// Returns the split of C for the collinear face of `long_edge`, or `None`
    /// when R-S is an edge already, or when a half is degenerate at a corner or
    /// faces against the faces it replaces.
    pub(super) fn split_of(&self, long_edge: HalfEdge) -> Option<Split> {
        let c = self.twin(long_edge);
        // A 2nd edge R-S would have 4 faces.
        if self.has_edge(self.apex(long_edge), self.apex(c)) {
            return None;
        }
        let replaced_crosses = [long_edge.face, c.face]
            .map(|face| cross_of_edges_f64(&self.positions(self.faces[face.get()])));
        for half in self.halves(long_edge) {
            let corners = self.positions(half);
            let cross = cross_of_edges_f64(&corners);
            // A half can face against the faces it replaces when R is very near
            // P or Q, on the side of S.
            let facing =
                cross.dot_product(&replaced_crosses[0]) + cross.dot_product(&replaced_crosses[1]);
            if is_degenerate_at_any_corner(&corners) || facing <= 0.0 {
                return None;
            }
        }
        Some(Split {
            long_edge,
            a: self.twin(long_edge.prev()),
            b: self.twin(long_edge.next()),
            c,
        })
    }

    /// Writes (R, P, S) into the slot of the collinear face and (Q, R, S) into
    /// the slot of C, and returns the record.
    pub(in crate::indexed_mesh::fix::collinear_faces) fn split_opposite(
        &mut self,
        split: Split,
    ) -> MeshFix {
        let Split { long_edge, a, b, c } = split;
        let [p, q, r, s] = [
            self.tail(long_edge),
            self.head(long_edge),
            self.apex(long_edge),
            self.apex(c),
        ];
        // In the face beyond C on P-S.
        let beyond_c = self.twin(c.next());
        let [collinear, opposite] = [long_edge.face, c.face].map(|face| self.faces[face.get()]);
        let halves = self.halves(long_edge);
        self.faces[long_edge.face.get()] = halves[0];
        self.faces[c.face.get()] = halves[1];
        // The slot of the collinear face now holds R->P, P->S and S->R. `c` is
        // now Q->R and `c.next()` R->S; the link of S->Q does not change.
        let [r_to_p, p_to_s, s_to_r] = EdgeSlot::ALL.map(|slot| HalfEdge {
            face: long_edge.face,
            slot,
        });
        self.link(r_to_p, a);
        self.link(p_to_s, beyond_c);
        self.link(s_to_r, c.next());
        self.link(c, b);
        self.edges.remove(&EdgeKey::new(p, q));
        self.edges.insert(EdgeKey::new(r, s));
        MeshFix::RemovedCollinearFaceBySplit {
            collinear,
            opposite,
            halves,
        }
    }

    /// Returns (R, P, S), R 1st, and C with R in the place of P, in the
    /// rotation of C.
    fn halves(&self, long_edge: HalfEdge) -> [IndexedTriangle; 2] {
        let c = self.twin(long_edge);
        let r = self.apex(long_edge);
        [
            IndexedTriangle {
                vertices: [r, self.tail(long_edge), self.apex(c)],
            },
            self.with_head(c, r),
        ]
    }
}

#[cfg(test)]
mod split_opposite_tests {
    use super::*;
    use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::{
        edge_keys, edge_manifold, half_edges_of,
    };
    use crate::indexed_mesh::fix::collinear_faces::test_meshes::{SQUARE_FAN_PYRAMID, fan_pyramid};

    fn triangle(vertices: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(vertices)
    }

    #[test]
    fn a_split_links_the_halves_to_a_b_and_the_face_beyond_c() {
        let mesh = fan_pyramid(SQUARE_FAN_PYRAMID);
        let witness = edge_manifold(&mesh);
        let mut faces = LinkedFaces::new(&witness);
        // P->Q in the collinear face (0, 1, 2).
        let long_edge = HalfEdge {
            face: FaceIndex::from(0_usize),
            slot: EdgeSlot::V0V1,
        };
        let split = faces.split_of(long_edge).expect("a split");
        assert_eq!(
            faces.split_opposite(split),
            MeshFix::RemovedCollinearFaceBySplit {
                collinear: triangle([0, 1, 2]),
                opposite: triangle([1, 0, 3]),
                halves: [triangle([2, 0, 3]), triangle([1, 2, 3])],
            }
        );
        assert_eq!(
            faces.twins,
            [
                [(1, 2), (5, 2), (3, 1)],
                [(4, 2), (5, 0), (0, 0)],
                [(3, 0), (7, 0), (4, 0)],
                [(2, 0), (0, 2), (7, 1)],
                [(2, 2), (6, 0), (1, 0)],
                [(1, 1), (6, 2), (0, 1)],
                [(4, 1), (7, 2), (5, 1)],
                [(2, 1), (3, 2), (6, 1)],
            ]
            .map(half_edges_of)
        );
        assert_eq!(
            faces.edges,
            edge_keys(&[
                (1, 2),
                (0, 2),
                (2, 4),
                (0, 4),
                (1, 5),
                (2, 5),
                (0, 3),
                (1, 3),
                (4, 5),
                (3, 4),
                (3, 5),
                (2, 3),
            ])
        );
        let mut expected = mesh.faces.clone();
        expected[0] = triangle([2, 0, 3]);
        expected[3] = triangle([1, 2, 3]);
        assert_eq!(faces.into_faces(), expected);
    }
}
