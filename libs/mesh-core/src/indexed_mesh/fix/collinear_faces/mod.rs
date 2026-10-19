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

mod linked_faces;
#[cfg(test)]
pub(in crate::indexed_mesh) mod test_meshes;

use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::validated::ValidatedEdgeManifoldMesh;
use crate::primitives::{FaceIndex, IndexedTriangle};

use linked_faces::links::LinkedFaces;
use linked_faces::removal::CollinearFaceRemoval;

/// Removes the collinear faces of `mesh`, and returns the records of the
/// removals and the faces after them.
pub(in crate::indexed_mesh) fn fix_collinear_faces_impl(
    mesh: &ValidatedEdgeManifoldMesh<'_>,
) -> (Vec<MeshFix>, Vec<IndexedTriangle>) {
    let mut pass = CollinearFacePass::new(mesh);
    // Each removal takes its collinear face out of the candidates, so a scan
    // that applies a removal leaves fewer candidates.
    while pass.scan() {}
    pass.finish()
}

/// The state of the pass between 2 removals.
struct CollinearFacePass<'a> {
    faces: LinkedFaces<'a>,
    /// The faces that a scan visits, ascending.
    candidates: Vec<FaceIndex>,
    /// 1 flag per face: whether the face is still a candidate.
    is_candidate: Vec<bool>,
    fixes: Vec<MeshFix>,
}

impl<'a> CollinearFacePass<'a> {
    fn new(mesh: &'a ValidatedEdgeManifoldMesh<'_>) -> Self {
        let candidates: Vec<FaceIndex> = mesh
            .validate_face_areas()
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|face| face.face_idx)
            .collect();
        let mut is_candidate = vec![false; mesh.faces().len()];
        for face in &candidates {
            is_candidate[face.get()] = true;
        }
        Self {
            faces: LinkedFaces::new(mesh),
            candidates,
            is_candidate,
            fixes: Vec::new(),
        }
    }

    /// Visits the candidates once, in ascending order, and returns whether it
    /// applied a removal.
    fn scan(&mut self) -> bool {
        let mut applied = false;
        for idx in 0..self.candidates.len() {
            let face = self.candidates[idx];
            if self.is_candidate[face.get()] {
                applied |= self.try_remove(face);
            }
        }
        let is_candidate = &self.is_candidate;
        self.candidates.retain(|face| is_candidate[face.get()]);
        applied
    }

    fn try_remove(&mut self, face: FaceIndex) -> bool {
        let Some(collinear) = self.faces.collinear_face(face) else {
            // Only a removal changes a face, and a removal takes the faces it
            // changes out of the candidates.
            self.is_candidate[face.get()] = false;
            return false;
        };
        let Some(removal) = self.faces.removal_of(collinear) else {
            return false;
        };
        for changed in removal.changed_neighbours().chain([face]) {
            self.is_candidate[changed.get()] = false;
        }
        self.fixes.push(match removal {
            CollinearFaceRemoval::MergeNeighbours(merge) => self.faces.merge_neighbours(merge),
            CollinearFaceRemoval::SplitOpposite(split) => self.faces.split_opposite(split),
        });
        true
    }

    /// Returns the records, and the faces without the removed ones, in their
    /// order.
    fn finish(self) -> (Vec<MeshFix>, Vec<IndexedTriangle>) {
        (self.fixes, self.faces.into_faces())
    }
}
