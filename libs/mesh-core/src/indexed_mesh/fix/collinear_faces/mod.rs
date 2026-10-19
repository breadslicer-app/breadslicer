/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The repair pass that removes collinear faces: zero-area faces with 3
//! distinct vertices on 1 line, so that 1 edge is longer than the other 2. For
//! a collinear face (R, P, Q) with its long edge P-Q:
//!
//! - when the faces across R-P and Q-R have the same 3rd vertex X, the pass
//!   removes the collinear face and merges these 2 faces into (P, Q, X);
//! - otherwise, it removes the collinear face and splits the face across P-Q at
//!   R.
//!
//! In the terms of mesh editing, the merge collapses the edge R-Q onto Q, and
//! the split flips the edge P-Q.
//!
//! The 2 removals keep the outline of the faces they replace, so the surface
//! moves by the distance from R to the line P-Q at most. The pass skips a
//! removal that would leave an edge without exactly 2 faces, or make a face
//! that is degenerate at a corner or that faces the other way.

mod candidates;
mod linked_faces;
#[cfg(test)]
pub(in crate::indexed_mesh) mod test_meshes;

use std::mem;

use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::validated::ValidatedEdgeManifoldMesh;
use crate::primitives::IndexedTriangle;

use candidates::Candidates;
use linked_faces::links::LinkedFaces;
use linked_faces::removal::CollinearFaceRemoval;

/// The removals of the collinear faces of an edge-manifold mesh, 1 record per
/// item. The candidates are its zero-area faces that have a long edge. The
/// removals come in rounds: a round tries each waiting face once, by ascending
/// index, and sets aside the faces with no removal for the next round.
pub(in crate::indexed_mesh) struct CollinearFaceRemovals<'a> {
    faces: LinkedFaces<'a>,
    candidates: Candidates,
    /// Whether this round removed a face.
    removed_in_round: bool,
}

impl<'a> CollinearFaceRemovals<'a> {
    pub(in crate::indexed_mesh) fn new(mesh: &'a ValidatedEdgeManifoldMesh<'_>) -> Self {
        let faces = LinkedFaces::new(mesh);
        let candidates = mesh
            .validate_face_areas()
            .err()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|zero_area| faces.collinear_face(zero_area.face_idx))
            .collect();
        Self {
            faces,
            candidates,
            // #[gamma::skip(literal.bool_flip, reason = "1 more round")]
            removed_in_round: false,
        }
    }

    /// Returns the faces after the removals so far, without the removed ones,
    /// in their order.
    pub(in crate::indexed_mesh) fn into_faces(self) -> Vec<IndexedTriangle> {
        self.faces.into_faces()
    }
}

impl Iterator for CollinearFaceRemovals<'_> {
    type Item = MeshFix;

    fn next(&mut self) -> Option<MeshFix> {
        loop {
            let Some(collinear) = self.candidates.pop_waiting() else {
                // A removal can make a set-aside face removable, as in a chain
                // of collinear faces on 1 edge. A round without a removal
                // changes nothing, so it is the last.
                // #[gamma::skip(cond.always_false, reason = "never ends")]
                if !mem::take(&mut self.removed_in_round) {
                    return None;
                }
                self.candidates.start_next_round();
                continue;
            };
            let Some(removal) = self.faces.removal_of(collinear) else {
                self.candidates.set_aside(collinear);
                continue;
            };
            // A zero-area face can be a neighbour that the removal changes.
            for face in removal.changed_neighbours() {
                self.candidates.remove(face);
            }
            self.removed_in_round = true;
            return Some(match removal {
                CollinearFaceRemoval::MergeNeighbours(merge) => self.faces.merge_neighbours(merge),
                CollinearFaceRemoval::SplitOpposite(split) => self.faces.split_opposite(split),
            });
        }
    }
}

#[cfg(test)]
mod collinear_face_removals_tests {
    use super::*;
    use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::edge_manifold;
    use crate::indexed_mesh::fix::collinear_faces::test_meshes::{
        SPLIT_TETRAHEDRON, SQUARE_FAN_PYRAMID, chained_pyramid, collinear_pair, fan_pyramid,
        flat_tetrahedron, split_tetrahedron, split_tetrahedron_with_a_thin_neighbour,
    };
    use crate::primitives::FaceIndex;

    fn fi(index: usize) -> FaceIndex {
        FaceIndex::from(index)
    }

    fn triangle(vertices: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(vertices)
    }

    #[test]
    fn the_removals_come_1_by_1_in_rounds() {
        let mesh = chained_pyramid();
        let witness = edge_manifold(&mesh);
        let mut removals = CollinearFaceRemovals::new(&witness);
        // Face 8 has a removal only after the removal of face 9, in round 2.
        assert_eq!(
            removals.next(),
            Some(MeshFix::RemovedCollinearFaceBySplit {
                collinear: triangle([0, 1, 6]),
                opposite: triangle([1, 0, 4]),
                halves: [[6, 0, 4], [1, 6, 4]].map(triangle),
            })
        );
        assert_eq!(
            removals.next(),
            Some(MeshFix::RemovedCollinearFaceBySplit {
                collinear: triangle([0, 6, 5]),
                opposite: triangle([6, 0, 4]),
                halves: [[5, 0, 4], [6, 5, 4]].map(triangle),
            })
        );
        assert_eq!(removals.next(), None);
        assert_eq!(removals.next(), None);
    }

    #[test]
    fn the_faces_after_the_1st_removal_hold_only_that_removal() {
        let mesh = chained_pyramid();
        let witness = edge_manifold(&mesh);
        let mut removals = CollinearFaceRemovals::new(&witness);
        assert!(removals.next().is_some());
        let mut expected = mesh.faces.clone();
        expected[4] = triangle([1, 6, 4]);
        expected[9] = triangle([6, 0, 4]);
        assert_eq!(removals.into_faces(), expected);
    }

    #[test]
    fn a_merge_and_a_split_each_give_a_1st_record() {
        let merging = split_tetrahedron(SPLIT_TETRAHEDRON);
        let witness = edge_manifold(&merging);
        assert!(matches!(
            CollinearFaceRemovals::new(&witness).next(),
            Some(MeshFix::RemovedCollinearFaceByMerge { .. })
        ));
        let splitting = fan_pyramid(SQUARE_FAN_PYRAMID);
        let witness = edge_manifold(&splitting);
        assert!(matches!(
            CollinearFaceRemovals::new(&witness).next(),
            Some(MeshFix::RemovedCollinearFaceBySplit { .. })
        ));
    }

    #[test]
    fn faces_that_stay_and_meshes_without_zero_area_faces_give_no_record() {
        let staying = flat_tetrahedron();
        let witness = edge_manifold(&staying);
        assert_eq!(CollinearFaceRemovals::new(&witness).next(), None);
        // With R off the line P-Q, no face has zero area.
        let mut points = SPLIT_TETRAHEDRON;
        points[2] = [0.0, 0.0, 0.0];
        let clean = split_tetrahedron(points);
        let witness = edge_manifold(&clean);
        assert_eq!(witness.validate_face_areas(), Ok(()));
        assert_eq!(CollinearFaceRemovals::new(&witness).next(), None);
    }

    #[test]
    fn new_takes_the_zero_area_faces_that_have_a_long_edge() {
        let flat = flat_tetrahedron();
        let witness = edge_manifold(&flat);
        let removals = CollinearFaceRemovals::new(&witness);
        // Only face 0 of the flat tetrahedron has zero area.
        assert!(removals.candidates.contains(fi(0)));
        for face in 1..4 {
            assert!(!removals.candidates.contains(fi(face)), "{face}");
        }
        let mut pair = collinear_pair();
        // With R on P, the 2 faces have zero area and no long edge.
        pair.vertices[2] = pair.vertices[0];
        let witness = edge_manifold(&pair);
        assert_eq!(
            witness.validate_face_areas().map_err(|faces| faces.len()),
            Err(2)
        );
        let removals = CollinearFaceRemovals::new(&witness);
        assert!(!removals.candidates.contains(fi(0)));
        assert!(!removals.candidates.contains(fi(1)));
    }

    #[test]
    fn a_removal_takes_the_neighbours_that_it_changes_out_of_the_candidates() {
        let mesh = split_tetrahedron_with_a_thin_neighbour();
        let witness = edge_manifold(&mesh);
        let mut removals = CollinearFaceRemovals::new(&witness);
        assert!(removals.candidates.contains(fi(1)));
        // The merge of face 0 rewrites A, face 1.
        assert!(matches!(
            removals.next(),
            Some(MeshFix::RemovedCollinearFaceByMerge { .. })
        ));
        assert!(!removals.candidates.contains(fi(1)));
    }
}
