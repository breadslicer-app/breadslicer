/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::{HalfEdge, LinkedFaces};
use crate::indexed_mesh::fix::collinear_faces::linked_faces::merge::Merge;
use crate::indexed_mesh::fix::collinear_faces::linked_faces::split::Split;
use crate::indexed_mesh::half_edges::EdgeSlot;
use crate::primitives::{FaceIndex, Vertex};

/// A collinear face (R, P, Q), read by its long edge P->Q: the edge that is
/// strictly longer than the other 2. R, the apex of the long edge, is the
/// vertex in the middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::indexed_mesh::fix::collinear_faces) struct CollinearFace {
    long_edge: HalfEdge,
}

impl CollinearFace {
    pub(in crate::indexed_mesh::fix::collinear_faces) fn face(self) -> FaceIndex {
        self.long_edge.face
    }
}

/// The removal of a collinear face (R, P, Q). A is the face across R-P, B the
/// face across Q-R, and C the face across P-Q.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::indexed_mesh::fix::collinear_faces) enum CollinearFaceRemoval {
    /// A and B have the same 3rd vertex X, and merge into (P, Q, X).
    MergeNeighbours(Merge),
    /// A and B do not: C, with its 3rd vertex S, splits at R into (R, P, S) and
    /// (Q, R, S).
    SplitOpposite(Split),
}

impl CollinearFaceRemoval {
    /// Returns the faces other than the collinear face that the removal
    /// changes: A and B for a merge, C for a split.
    pub(in crate::indexed_mesh::fix::collinear_faces) fn changed_neighbours(
        self,
    ) -> impl Iterator<Item = FaceIndex> {
        // #[gamma::skip(collection.reverse_array, reason = "None is dropped")]
        let faces = match self {
            Self::MergeNeighbours(merge) => merge.neighbours().map(Some),
            Self::SplitOpposite(split) => [Some(split.opposite()), None],
        };
        faces.into_iter().flatten()
    }
}

impl LinkedFaces<'_> {
    /// Returns `face` read as a collinear face, or `None` when 2 of its edges
    /// tie for the longest, as when 2 of its vertices are at 1 position.
    pub(in crate::indexed_mesh::fix::collinear_faces) fn collinear_face(
        &self,
        face: FaceIndex,
    ) -> Option<CollinearFace> {
        let [a, b, c] = self.positions(self.faces[face.get()]).map(Vertex::to_f64);
        // The squared length of the edge at each slot.
        let [l0, l1, l2] = [a.diff(&b), b.diff(&c), c.diff(&a)].map(|edge| edge.squared_norm());
        let slot = if l0 > l1 && l0 > l2 {
            EdgeSlot::V0V1
        } else if l1 > l2 && l1 > l0 {
            EdgeSlot::V1V2
        } else if l2 > l0 && l2 > l1 {
            EdgeSlot::V2V0
        } else {
            return None;
        };
        Some(CollinearFace {
            long_edge: HalfEdge { face, slot },
        })
    }

    /// Returns the removal of `collinear`, or `None` when the removal would
    /// leave an edge without exactly 2 faces, or make a face that is degenerate
    /// at a corner or that faces the other way.
    pub(in crate::indexed_mesh::fix::collinear_faces) fn removal_of(
        &self,
        collinear: CollinearFace,
    ) -> Option<CollinearFaceRemoval> {
        let long_edge = collinear.long_edge;
        // The 3rd vertices of A, B and C.
        let [x, y, s] = [long_edge.prev(), long_edge.next(), long_edge]
            .map(|half_edge| self.apex(self.twin(half_edge)));
        // C holds R only when the collinear face and C are a closed part of 2
        // faces.
        // #[gamma::skip(cond.always_false, reason = "G1 refuses it too")]
        if s == self.apex(long_edge) {
            return None;
        }
        if x == y {
            self.merge_of(long_edge)
                .map(CollinearFaceRemoval::MergeNeighbours)
        } else {
            self.split_of(long_edge)
                .map(CollinearFaceRemoval::SplitOpposite)
        }
    }
}

#[cfg(test)]
mod collinear_face_tests {
    use super::*;
    use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::edge_manifold;
    use crate::indexed_mesh::fix::collinear_faces::test_meshes::collinear_pair;
    use crate::primitives::IndexedTriangle;

    #[test]
    fn every_rotation_of_a_face_gives_the_same_reading() {
        // Face 0 of the pair is (R, P, Q) = (2, 0, 1).
        for rotation in [[2, 0, 1], [0, 1, 2], [1, 2, 0]] {
            let mut mesh = collinear_pair();
            mesh.faces[0] = IndexedTriangle::from_raw_indices(rotation);
            let witness = edge_manifold(&mesh);
            let faces = LinkedFaces::new(&witness);
            let long_edge = faces
                .collinear_face(FaceIndex::from(0_usize))
                .expect("a long edge")
                .long_edge;
            let r_p_q = [
                faces.apex(long_edge),
                faces.tail(long_edge),
                faces.head(long_edge),
            ];
            assert_eq!(r_p_q.map(u32::from), [2, 0, 1], "{rotation:?}");
        }
    }

    #[test]
    fn a_face_with_2_vertices_at_1_position_has_no_reading() {
        // With R on P, the edges P-Q and Q-R tie. Each rotation puts them at
        // another pair of slots.
        for rotation in [[2, 0, 1], [0, 1, 2], [1, 2, 0]] {
            let mut mesh = collinear_pair();
            mesh.vertices[2] = mesh.vertices[0];
            mesh.faces[0] = IndexedTriangle::from_raw_indices(rotation);
            let witness = edge_manifold(&mesh);
            let faces = LinkedFaces::new(&witness);
            assert_eq!(
                faces.collinear_face(FaceIndex::from(0_usize)),
                None,
                "{rotation:?}"
            );
        }
    }
}

#[cfg(test)]
mod removal_of_tests {
    use super::*;
    use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::edge_manifold;
    use crate::indexed_mesh::fix::collinear_faces::test_meshes::{
        SPLIT_TETRAHEDRON, SQUARE_FAN_PYRAMID, blocked_split_tetrahedron, collinear_pair,
        fan_pyramid, flat_tetrahedron, split_tetrahedron,
    };
    use crate::indexed_mesh::mesh::IndexedMesh;

    fn fi(index: usize) -> FaceIndex {
        FaceIndex::from(index)
    }

    /// Returns the removal of face 0 of `mesh`, which must be edge-manifold.
    fn removal_of_face_0(mesh: &IndexedMesh) -> Option<CollinearFaceRemoval> {
        let witness = edge_manifold(mesh);
        let faces = LinkedFaces::new(&witness);
        let collinear = faces.collinear_face(fi(0)).expect("face 0 has a long edge");
        faces.removal_of(collinear)
    }

    /// Returns [`SQUARE_FAN_PYRAMID`] with R at `r`.
    fn square_fan_pyramid_with_r(r: [f32; 3]) -> IndexedMesh {
        let mut points = SQUARE_FAN_PYRAMID;
        points[2] = r;
        fan_pyramid(points)
    }

    /// Returns a fan pyramid on P (0, 0, 0) and Q (1, 0, 0) in which C is a
    /// thin face with S at `s`, and R is 2^-21 off the line P-Q, at
    /// `r_along_pq` along it.
    fn thin_fan_pyramid(r_along_pq: f32, s: [f32; 3]) -> IndexedMesh {
        let r = [r_along_pq, 2.0_f32.powi(-21), 0.0];
        fan_pyramid([
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            r,
            s,
            [0.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
        ])
    }

    #[test]
    fn faces_beside_the_short_edges_with_1_third_vertex_merge() {
        let removal = removal_of_face_0(&split_tetrahedron(SPLIT_TETRAHEDRON)).expect("a removal");
        assert!(matches!(removal, CollinearFaceRemoval::MergeNeighbours(_)));
        let changed: Vec<FaceIndex> = removal.changed_neighbours().collect();
        assert_eq!(changed, [fi(1), fi(2)]);
    }

    #[test]
    fn faces_beside_the_short_edges_with_2_third_vertices_split_the_opposite_face() {
        let removal = removal_of_face_0(&fan_pyramid(SQUARE_FAN_PYRAMID)).expect("a removal");
        assert!(matches!(removal, CollinearFaceRemoval::SplitOpposite(_)));
        let changed: Vec<FaceIndex> = removal.changed_neighbours().collect();
        assert_eq!(changed, [fi(3)]);
    }

    #[test]
    fn a_closed_pair_has_no_removal() {
        assert_eq!(removal_of_face_0(&collinear_pair()), None);
    }

    #[test]
    fn a_closed_flat_tetrahedron_has_no_removal() {
        assert_eq!(removal_of_face_0(&flat_tetrahedron()), None);
    }

    #[test]
    fn a_split_that_would_repeat_an_edge_is_refused() {
        assert_eq!(removal_of_face_0(&blocked_split_tetrahedron()), None);
    }

    #[test]
    fn a_merge_into_a_sliver_is_refused() {
        // (P, Q, X) has a sine of about 2.4e-7 at P.
        let sliver = split_tetrahedron([
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [4.0, 2.0_f32.powi(-20), 0.0],
            [1.0, 1.0, 1.0],
        ]);
        assert_eq!(removal_of_face_0(&sliver), None);
    }

    #[test]
    fn a_split_into_a_sliver_is_refused_for_each_half() {
        let near_p = [2.0 - 2.0_f32.powi(-20), 0.0, 0.0];
        let near_q = [2.0_f32.powi(-20), 0.0, 0.0];
        for r in [near_p, near_q] {
            assert_eq!(
                removal_of_face_0(&square_fan_pyramid_with_r(r)),
                None,
                "{r:?}"
            );
        }
    }

    #[test]
    fn a_split_that_turns_a_half_over_is_refused_for_each_half() {
        let s_height = 2.0_f32.powi(-13);
        let near_p = (2.0_f32.powi(-17), 0.125);
        let near_q = (1.0 - 2.0_f32.powi(-17), 0.875);
        for (r_along_pq, s_along_pq) in [near_p, near_q] {
            // With S on the side of R, the half at the end near R folds back.
            let folded = thin_fan_pyramid(r_along_pq, [s_along_pq, s_height, 0.0]);
            assert_eq!(removal_of_face_0(&folded), None, "{r_along_pq}");
            // With S on the other side, the 2 halves face the way of C.
            let flat = thin_fan_pyramid(r_along_pq, [s_along_pq, -s_height, 0.0]);
            assert!(
                matches!(
                    removal_of_face_0(&flat),
                    Some(CollinearFaceRemoval::SplitOpposite(_))
                ),
                "{r_along_pq}"
            );
        }
    }

    #[test]
    fn a_split_is_refused_when_a_half_does_not_face_the_way_of_the_replaced_faces() {
        // R is off the plane of C, and C is about as thin as the collinear
        // face, so the 2 faces that the split replaces face another way than C
        // alone. (Q, R, S) faces the way of C, but is edge-on to the 2 faces
        // together with S at x = 1.125, and faces against them at x = 1.5.
        for s_x in [1.125, 1.5] {
            let mesh = fan_pyramid([
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0 - 2.0_f32.powi(-5), 0.0, 2.0_f32.powi(-20)],
                [s_x, 2.0_f32.powi(-19), 0.0],
                [0.0, -1.0, -1.0],
                [1.0, -1.0, -1.0],
            ]);
            assert_eq!(removal_of_face_0(&mesh), None, "{s_x}");
        }
    }
}
