/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::fix::{MeshFix, MeshFixError};

use super::{MeshEdgeManifoldError, ValidatedFacesMut};

impl ValidatedFacesMut<'_> {
    /// Reverses the winding of the faces that `manifold_errors` shows as
    /// flipped. On `Err` the mesh is unchanged. The vertex indices of a face
    /// only change places, so `self` stays a valid witness. `manifold_errors`
    /// must come from the edge-manifold validation of this mesh.
    pub fn fix_faces_orientation(
        &mut self,
        manifold_errors: &[MeshEdgeManifoldError],
    ) -> Result<Vec<MeshFix>, MeshFixError> {
        crate::indexed_mesh::fix::fix_faces_orientation_impl(self.mesh, manifold_errors)
    }
}
