/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::fix::MeshFix;
use crate::indexed_mesh::fix::collinear_faces::fix_collinear_faces_impl;

use super::{ValidatedEdgeManifoldMeshMut, validate_face_areas_impl};

impl ValidatedEdgeManifoldMeshMut<'_> {
    /// Removes the collinear faces of the mesh, and returns the records of the
    /// removals. A collinear face has zero area, 3 distinct vertices and 1 edge
    /// longer than the other 2. A removal merges the 2 faces beside its short
    /// edges into 1, or splits the face across its long edge in 2.
    ///
    /// A zero-area face stays when 2 of its vertices are at 1 position, or when
    /// its removal would break an edge or make a degenerate or turned-over
    /// face, as for the collinear face of a closed part in 1 plane. The
    /// vertices do not change, so the in-range witness that minted this one
    /// stays valid.
    pub fn fix_collinear_faces(self) -> Vec<MeshFix> {
        let zero_area_faces = validate_face_areas_impl(self.mesh)
            .err()
            .unwrap_or_default();
        fix_collinear_faces_impl(self.mesh, &zero_area_faces)
    }
}
