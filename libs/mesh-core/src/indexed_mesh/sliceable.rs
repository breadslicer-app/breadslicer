/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The slicer's gate: whether a mesh can be sliced. The slicer, Repair and the
//! sliceable metric all ask here, so they cannot disagree.
//!
//! A mesh is sliceable when its vertices are finite, its faces valid, and
//! every edge is shared by exactly 2 consistently wound faces, or when the
//! only edges that break that rule are shared by 4 faces of 2 closed solids
//! that touch along them ("2+2"). Those the slicer separates on its own copy
//! with the code Repair uses (radial pairing, then 1 vertex per fan of paired
//! faces), so the user's mesh does not change and needs no Repair. Edges with
//! any other number of faces are not separated.

use crate::primitives::{IndexedTriangle, Vertex};

use super::fix::watertight::separate_touching_solids;
use super::mesh::IndexedMesh;
use super::validate::{MeshFaceError, MeshVertexError};
use super::validated::{MeshEdgeManifoldError, ValidatedEdgeManifoldMesh, ValidatedFaces};

/// How many faces an edge between 2 closed solids that touch along it has.
const TOUCHING_SOLIDS_EDGE_FACES: usize = 4;

/// Which stage of the slicer's gate rejected a mesh. The stages run in order
/// (vertices, then faces, then edges), each requiring the previous to pass, so
/// a rejection carries the errors of exactly 1 stage.
#[derive(Clone, Debug, PartialEq)]
pub enum SliceableError {
    /// A vertex coordinate is NaN or infinite.
    Vertices(Vec<MeshVertexError>),
    /// A face index is out of range or a face has a zero-length edge.
    Faces(Vec<MeshFaceError>),
    /// The mesh is not closed and edge-manifold, and separating solids that
    /// touch along an edge does not make it so. The errors are those of the
    /// mesh as given.
    Manifold(Vec<MeshEdgeManifoldError>),
}

impl IndexedMesh {
    /// Returns whether the slicer accepts this mesh (see the module docs).
    /// Zero-area faces do not block slicing and are not checked. Stops at the
    /// first failing stage. Only a mesh with edges shared by 4 faces is copied,
    /// to try the separation.
    pub fn is_sliceable(&self) -> bool {
        match self.edge_stage() {
            Ok(()) => true,
            Err(SliceableError::Manifold(errors)) => {
                self.touching_solids_separated(&errors).is_some()
            }
            Err(_) => false,
        }
    }

    /// Readies this mesh for slicing and returns the edge-manifold witness the
    /// slicer cuts. When the gate passes as is, nothing changes. When the only
    /// bad edges are 2+2 edges of solids that touch, they are separated in
    /// place: each solid gets its own copy of the shared vertices, at the same
    /// position, and the faces keep their order and winding. Meant for a copy
    /// the caller owns, like the slicer's baked leaf. On an error the mesh is
    /// unchanged.
    ///
    /// # Errors
    ///
    /// The [`SliceableError`] of the 1st stage that fails, with the errors of
    /// the mesh as given.
    pub fn make_sliceable(&mut self) -> Result<ValidatedEdgeManifoldMesh<'_>, SliceableError> {
        if let Err(error) = self.edge_stage() {
            let SliceableError::Manifold(errors) = &error else {
                return Err(error);
            };
            let Some((vertices, faces)) = self.touching_solids_separated(errors) else {
                return Err(error);
            };
            self.vertices = vertices;
            self.faces = faces;
        }
        Ok(ValidatedEdgeManifoldMesh::new_unchecked(self))
    }

    /// Runs the 3 stages of the gate on the mesh as is.
    fn edge_stage(&self) -> Result<(), SliceableError> {
        self.validate_vertices().map_err(SliceableError::Vertices)?;
        self.validated_faces()
            .map_err(SliceableError::Faces)?
            .validate_edge_manifold()
            .map_err(SliceableError::Manifold)
    }

    /// Returns the vertices and faces of this mesh with its 2+2 edges
    /// separated, or `None` when `errors` (the edge errors of this mesh, whose
    /// faces are valid) name anything else or the separated copy still fails
    /// the edge check.
    fn touching_solids_separated(
        &self,
        errors: &[MeshEdgeManifoldError],
    ) -> Option<(Vec<Vertex>, Vec<IndexedTriangle>)> {
        let only_touching_solids = errors.iter().all(|e| {
            matches!(e, MeshEdgeManifoldError::NonManifoldEdge { face_indices, .. }
                if face_indices.len() == TOUCHING_SOLIDS_EDGE_FACES)
        });
        if !only_touching_solids {
            return None;
        }
        let mut vertices = self.vertices.clone();
        let mut faces = self.faces.clone();
        if !separate_touching_solids(&mut vertices, &mut faces) {
            return None;
        }
        let separated = IndexedMesh::new(None, vertices, faces);
        // The separation only adds vertices and repoints corners to them, so
        // the face indices stay in range.
        ValidatedFaces::new_unchecked(&separated)
            .validate_edge_manifold()
            .ok()?;
        Some((separated.vertices, separated.faces))
    }
}
