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
//! The 2 removals keep the outline of the faces they replace, so the surface
//! moves by the distance from R to the line P-Q at most. The pass skips a
//! removal that would leave an edge without exactly 2 faces, or make a face
//! that is degenerate at a corner or that faces the other way.

mod edge_faces;
mod removal;
#[cfg(test)]
pub(in crate::indexed_mesh) mod test_meshes;

use crate::indexed_mesh::IndexedMesh;
use crate::indexed_mesh::fix::MeshFix;
use crate::indexed_mesh::validated::ZeroAreaFace;
use crate::primitives::{FaceIndex, IndexedTriangle};

use edge_faces::EdgeFaces;
use removal::{CollinearFace, CollinearFaceRemoval, choose_removal};

/// A face slot and the triangle that a removal writes into it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SlotWrite {
    face: FaceIndex,
    triangle: IndexedTriangle,
}

/// Removes the collinear faces of `mesh`, and returns the records of the
/// removals. `zero_area_faces` must be the zero-area faces of `mesh`, as
/// `validate_face_areas` reports them, and `mesh` must be edge-manifold: when
/// it is not, nothing changes.
pub(in crate::indexed_mesh) fn fix_collinear_faces_impl(
    mesh: &mut IndexedMesh,
    zero_area_faces: &[ZeroAreaFace],
) -> Vec<MeshFix> {
    let Some(edge_faces) = EdgeFaces::build(&mesh.faces) else {
        return Vec::new();
    };
    let mut pass = CollinearFacePass::new(mesh, edge_faces, zero_area_faces);
    // Each removal takes its collinear face out of the candidates, so a scan
    // that applies a removal leaves fewer candidates.
    while pass.scan() {}
    pass.finish()
}

/// The state of the pass between 2 removals. Face indices keep their meaning
/// until [`CollinearFacePass::finish`]: a removed face stays in the faces of
/// the mesh, marked as deleted.
struct CollinearFacePass<'m> {
    mesh: &'m mut IndexedMesh,
    edge_faces: EdgeFaces,
    /// The faces that a scan visits, ascending.
    candidates: Vec<FaceIndex>,
    /// 1 flag per face: whether the face is still a candidate.
    is_candidate: Vec<bool>,
    /// 1 flag per face.
    is_deleted: Vec<bool>,
    fixes: Vec<MeshFix>,
}

impl<'m> CollinearFacePass<'m> {
    fn new(
        mesh: &'m mut IndexedMesh,
        edge_faces: EdgeFaces,
        zero_area_faces: &[ZeroAreaFace],
    ) -> Self {
        let face_count = mesh.faces.len();
        let candidates: Vec<FaceIndex> = zero_area_faces.iter().map(|face| face.face_idx).collect();
        let mut is_candidate = vec![false; face_count];
        for face in &candidates {
            is_candidate[face.get()] = true;
        }
        Self {
            mesh,
            edge_faces,
            candidates,
            is_candidate,
            is_deleted: vec![false; face_count],
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
        let faces = &self.mesh.faces;
        let vertices = &self.mesh.vertices;
        let Some(collinear) = CollinearFace::new(face, faces[face.get()], vertices) else {
            // Only a removal changes a face, and a removal takes the faces it
            // changes out of the candidates.
            self.is_candidate[face.get()] = false;
            return false;
        };
        match choose_removal(&collinear, faces, vertices, &self.edge_faces) {
            Some(removal) => self.apply(removal),
            None => false,
        }
    }

    /// Applies `removal` to the faces and to `edge_faces`, and records it.
    /// Returns `false`, with no change, when `edge_faces` refuses the update,
    /// which the conditions of the choice rule out.
    fn apply(&mut self, removal: CollinearFaceRemoval) -> bool {
        let faces = &mut self.mesh.faces;
        match removal {
            CollinearFaceRemoval::MergeNeighbours {
                collinear,
                absorbed,
                merged,
            } => {
                if !self
                    .edge_faces
                    .update(faces, &[collinear, absorbed], &[merged])
                {
                    return false;
                }
                self.fixes.push(MeshFix::RemovedCollinearFaceByMerge {
                    collinear: faces[collinear.get()],
                    merged_neighbours: [faces[merged.face.get()], faces[absorbed.get()]],
                    merged: merged.triangle,
                });
                faces[merged.face.get()] = merged.triangle;
                for face in [collinear, absorbed, merged.face] {
                    self.is_candidate[face.get()] = false;
                }
                for face in [collinear, absorbed] {
                    self.is_deleted[face.get()] = true;
                }
            }
            CollinearFaceRemoval::SplitOpposite {
                collinear,
                opposite,
            } => {
                if !self.edge_faces.update(faces, &[], &[collinear, opposite]) {
                    return false;
                }
                self.fixes.push(MeshFix::RemovedCollinearFaceBySplit {
                    collinear: faces[collinear.face.get()],
                    opposite: faces[opposite.face.get()],
                    halves: [collinear.triangle, opposite.triangle],
                });
                for write in [collinear, opposite] {
                    faces[write.face.get()] = write.triangle;
                    self.is_candidate[write.face.get()] = false;
                }
            }
        }
        true
    }

    /// Drops the deleted faces, keeping the order of the others, and returns
    /// the records.
    fn finish(self) -> Vec<MeshFix> {
        let Self {
            mesh,
            is_deleted,
            fixes,
            ..
        } = self;
        let mut is_deleted = is_deleted.into_iter();
        mesh.faces.retain(|_| !is_deleted.next().unwrap_or(false));
        fixes
    }
}

#[cfg(test)]
mod fix_collinear_faces_impl_tests {
    use super::test_meshes::collinear_pair;
    use super::*;

    #[test]
    fn a_mesh_that_is_not_edge_manifold_does_not_change() {
        let mut mesh = collinear_pair();
        mesh.faces.truncate(1);
        let original = mesh.clone();
        let zero_area_faces = [ZeroAreaFace {
            face_idx: FaceIndex::from(0_usize),
        }];
        assert_eq!(
            fix_collinear_faces_impl(&mut mesh, &zero_area_faces),
            Vec::new()
        );
        assert_eq!(mesh, original);
    }
}
