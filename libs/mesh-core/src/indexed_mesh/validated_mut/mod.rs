/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Mutable witnesses, with the repairs that keep their claims. Each has the
//! claims of its read-only form in [`validated`](super::validated):
//!
//! 1. [`ValidatedFacesMut`]: the face indices are in range. Minted by
//!    [`IndexedMesh::validated_faces_mut`], or by
//!    [`IndexedMesh::fix_zero_length_edges_into_validated`] for the mesh that
//!    its removals leave.
//! 2. [`ValidatedEdgeManifoldMeshMut`]: in addition, the mesh is edge-manifold.
//!    Minted by [`ValidatedFacesMut::validated_edge_manifold_mut`], which
//!    borrows the in-range witness instead of consuming it.
//!
//! `fix_faces_orientation` reverses the vertex order of faces in place, so it
//! keeps the in-range claim. `fix_collinear_faces` keeps it too (it writes only
//! vertex indices that the faces already hold, and keeps the vertices), but it
//! changes the edges, so it consumes the mutable edge-manifold witness.

mod fix_collinear_faces;
mod fix_faces_orientation;
mod fix_watertight;

use crate::primitives::{IndexedTriangle, Vertex};

use super::fix::fix_zero_length_edges_impl;
use super::fix::records::MeshFix;
use super::mesh::IndexedMesh;
use super::validate::MeshFaceError;
use super::validated::{MeshEdgeManifoldError, ValidatedFaces, ZeroAreaFace};

/// Mutable witness that the underlying `IndexedMesh`'s face indices are all in
/// range. Methods on the witness only perform mutations that preserve that
/// invariant.
#[derive(Debug)]
pub struct ValidatedFacesMut<'a> {
    mesh: &'a mut IndexedMesh,
}

/// Mutable witness that the underlying `IndexedMesh` is edge-manifold, with the
/// claims of [`ValidatedEdgeManifoldMesh`]. Its only method consumes it: the
/// claim holds when the witness is minted, and no check proves it after the
/// method changes the mesh.
///
/// [`ValidatedEdgeManifoldMesh`]: super::validated::ValidatedEdgeManifoldMesh
#[derive(Debug)]
pub struct ValidatedEdgeManifoldMeshMut<'a> {
    mesh: &'a mut IndexedMesh,
}

impl<'a> ValidatedFacesMut<'a> {
    fn new_unchecked(mesh: &'a mut IndexedMesh) -> Self {
        Self { mesh }
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.mesh.vertices
    }

    pub fn faces(&self) -> &[IndexedTriangle] {
        &self.mesh.faces
    }

    pub fn validate_face_areas(&self) -> Result<(), Vec<ZeroAreaFace>> {
        ValidatedFaces::new_unchecked(self.mesh).validate_face_areas()
    }

    pub fn validate_edge_manifold(&self) -> Result<(), Vec<MeshEdgeManifoldError>> {
        ValidatedFaces::new_unchecked(self.mesh).validate_edge_manifold()
    }

    /// Runs the edge-manifold validation and, on success, returns a mutable
    /// witness whose existence proves that the mesh is edge-manifold. The
    /// returned witness borrows `self`, which is usable again once it is gone.
    pub fn validated_edge_manifold_mut(
        &mut self,
    ) -> Result<ValidatedEdgeManifoldMeshMut<'_>, Vec<MeshEdgeManifoldError>> {
        self.validate_edge_manifold()?;
        Ok(ValidatedEdgeManifoldMeshMut {
            mesh: &mut *self.mesh,
        })
    }
}

impl IndexedMesh {
    /// Validates face indices as [`Self::validated_faces`] does, returning a
    /// witness that also permits mutation. The returned witness exposes only
    /// mutations that preserve the in-range invariant: winding flips and,
    /// through [`ValidatedFacesMut::validated_edge_manifold_mut`], the removal
    /// of collinear faces. The removal of faces with a zero-length edge lives
    /// on `IndexedMesh` itself, because it runs on faces that fail validation.
    pub fn validated_faces_mut(&mut self) -> Result<ValidatedFacesMut<'_>, Vec<MeshFaceError>> {
        self.validate_faces()?;
        Ok(ValidatedFacesMut::new_unchecked(self))
    }

    /// Repairs the `ZeroLengthEdge` errors in `errors` (other variants are
    /// ignored), then returns the applied fixes and a witness for the repaired
    /// mesh, or the face errors that remain. `errors` must be the face errors
    /// of this mesh, as `validate_faces` returns them: when they are all
    /// `ZeroLengthEdge`, the witness is created without a new validation.
    pub fn fix_zero_length_edges_into_validated(
        &mut self,
        errors: &[MeshFaceError],
    ) -> (
        Vec<MeshFix>,
        Result<ValidatedFacesMut<'_>, Vec<MeshFaceError>>,
    ) {
        let only_zero_length = errors
            .iter()
            .all(|e| matches!(e, MeshFaceError::ZeroLengthEdge { .. }));
        let fixes = fix_zero_length_edges_impl(self, errors);
        // With only `ZeroLengthEdge` errors, every index is in range; removing
        // whole faces changes neither `vertices.len()` nor the indices of the
        // faces that stay.
        // #[gamma::skip(cond.always_false, reason = "Ok too for a full list")]
        let witness = if only_zero_length {
            Ok(ValidatedFacesMut::new_unchecked(self))
        } else {
            self.validated_faces_mut()
        };
        (fixes, witness)
    }
}
