/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::collections::BTreeMap;

use crate::indexed_mesh::fix::collinear_faces::linked_faces::removal::CollinearFace;
use crate::primitives::FaceIndex;

/// The collinear faces that wait for a try: those of this round and those set
/// aside for the next one. Each is held under its own face, in 1 round only.
pub(super) struct Candidates {
    waiting: BTreeMap<FaceIndex, CollinearFace>,
    set_aside: BTreeMap<FaceIndex, CollinearFace>,
}

impl Candidates {
    /// Takes out the waiting face with the lowest index.
    pub(super) fn pop_waiting(&mut self) -> Option<CollinearFace> {
        self.waiting.pop_first().map(|(_, collinear)| collinear)
    }

    /// Moves `collinear` to the next round.
    pub(super) fn set_aside(&mut self, collinear: CollinearFace) {
        self.waiting.remove(&collinear.face());
        self.set_aside.insert(collinear.face(), collinear);
    }

    /// Takes `face` out of both rounds.
    pub(super) fn remove(&mut self, face: FaceIndex) {
        self.waiting.remove(&face);
        self.set_aside.remove(&face);
    }

    /// Makes the set-aside faces wait again.
    pub(super) fn start_next_round(&mut self) {
        self.waiting.append(&mut self.set_aside);
    }

    #[cfg(test)]
    pub(super) fn contains(&self, face: FaceIndex) -> bool {
        self.waiting.contains_key(&face) || self.set_aside.contains_key(&face)
    }
}

/// All the faces wait for the 1st round.
impl FromIterator<CollinearFace> for Candidates {
    fn from_iter<I: IntoIterator<Item = CollinearFace>>(faces: I) -> Self {
        Self {
            waiting: faces
                .into_iter()
                .map(|collinear| (collinear.face(), collinear))
                .collect(),
            set_aside: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod candidates_tests {
    use super::*;
    use crate::indexed_mesh::fix::collinear_faces::linked_faces::links::{
        LinkedFaces, edge_manifold,
    };
    use crate::indexed_mesh::fix::collinear_faces::test_meshes::{SQUARE_FAN_PYRAMID, fan_pyramid};

    /// Returns the readings of `faces` of the square fan pyramid, in their
    /// order. Any face with a strictly longest edge has one.
    fn readings(faces: &[usize]) -> Vec<CollinearFace> {
        let mesh = fan_pyramid(SQUARE_FAN_PYRAMID);
        let witness = edge_manifold(&mesh);
        let linked = LinkedFaces::new(&witness);
        faces
            .iter()
            .map(|&face| {
                linked
                    .collinear_face(FaceIndex::from(face))
                    .expect("a long edge")
            })
            .collect()
    }

    #[test]
    fn waiting_faces_come_by_ascending_index_with_their_readings() {
        let r = readings(&[1, 3, 5]);
        let mut candidates: Candidates = [r[2], r[0], r[1]].into_iter().collect();
        assert_eq!(candidates.pop_waiting(), Some(r[0]));
        assert_eq!(candidates.pop_waiting(), Some(r[1]));
        assert_eq!(candidates.pop_waiting(), Some(r[2]));
        assert_eq!(candidates.pop_waiting(), None);
    }

    #[test]
    fn a_set_aside_face_waits_for_the_next_round() {
        let r = readings(&[1, 3]);
        let mut candidates: Candidates = r.iter().copied().collect();
        assert_eq!(candidates.pop_waiting(), Some(r[0]));
        candidates.set_aside(r[0]);
        assert_eq!(candidates.pop_waiting(), Some(r[1]));
        assert_eq!(candidates.pop_waiting(), None);
        candidates.start_next_round();
        assert_eq!(candidates.pop_waiting(), Some(r[0]));
        assert_eq!(candidates.pop_waiting(), None);
    }

    #[test]
    fn remove_takes_a_face_out_of_either_round() {
        let r = readings(&[1, 3, 5]);
        let mut candidates: Candidates = r.iter().copied().collect();
        assert_eq!(candidates.pop_waiting(), Some(r[0]));
        candidates.set_aside(r[0]);
        // Face 1 is set aside, and face 5 waits.
        candidates.remove(FaceIndex::from(1_usize));
        candidates.remove(FaceIndex::from(5_usize));
        candidates.start_next_round();
        assert_eq!(candidates.pop_waiting(), Some(r[1]));
        assert_eq!(candidates.pop_waiting(), None);
    }

    #[test]
    fn setting_aside_a_waiting_face_moves_it_to_the_next_round() {
        let r = readings(&[1, 3]);
        let mut candidates: Candidates = r.iter().copied().collect();
        candidates.set_aside(r[1]);
        assert_eq!(candidates.pop_waiting(), Some(r[0]));
        assert_eq!(candidates.pop_waiting(), None);
        candidates.start_next_round();
        assert_eq!(candidates.pop_waiting(), Some(r[1]));
        assert_eq!(candidates.pop_waiting(), None);
    }
}
