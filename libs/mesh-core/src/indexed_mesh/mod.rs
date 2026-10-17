/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

pub mod declared_facet_normals;
pub mod diagnose;
pub mod error_summary;
pub mod fingerprint;
pub mod fix;
mod half_edges;
mod mesh;
pub mod repair;
pub mod topology;
pub mod validate;
pub mod validated;

#[cfg(test)]
mod tests_declared_facet_normals;
#[cfg(test)]
mod tests_diagnose;
#[cfg(test)]
mod tests_fingerprint;
#[cfg(test)]
mod tests_mesh;
#[cfg(test)]
mod tests_repair;
#[cfg(test)]
mod tests_validated_faces;
#[cfg(test)]
mod tests_volume;

pub use declared_facet_normals::{DeclaredFacetNormals, FacetCountMismatch, RestoreRefusal};
pub use diagnose::{FaceDiagnosis, MeshDiagnosis};
pub use error_summary::{ErrorVariantGroup, MeshErrorReport, group_errors_by_variant};
pub use fingerprint::geometry_fingerprint::GeometryFingerprint;
pub use fix::{MeshFix, MeshFixError};
pub use mesh::{IndexedMesh, MeshMetadata};
pub use repair::{MeshRepairReport, RepairPasses, RepairRefusal};
pub use topology::{EdgeKey, MeshTopology, edge_key};
pub use validate::{MeshFaceError, MeshVertexError};
pub use validated::{
    MeshEdgeManifoldError, ValidatedEdgeManifoldMesh, ValidatedFaces, ValidatedFacesMut,
    ZeroAreaFace,
};

use crate::aabb::Aabb;
use crate::primitives::{IndexedTriangle, Vertex};

impl IndexedMesh {
    pub fn new(
        meta: Option<MeshMetadata>,
        vertices: Vec<Vertex>,
        faces: Vec<IndexedTriangle>,
    ) -> Self {
        Self {
            meta,
            vertices,
            faces,
        }
    }

    pub fn meta(&self) -> &Option<MeshMetadata> {
        &self.meta
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    pub fn faces(&self) -> &[IndexedTriangle] {
        &self.faces
    }

    pub fn set_meta(&mut self, meta: Option<MeshMetadata>) {
        self.meta = meta;
    }

    /// Returns the bounds of `self.vertices` in object space, or `None` for an
    /// empty mesh. Uncached: each call walks the vertex array.
    pub fn local_aabb(&self) -> Option<Aabb> {
        let mut iter = self.vertices.iter();
        let first = *iter.next()?;
        let mut aabb = Aabb::from_min_max(first, first);
        for vertex in iter {
            aabb.extend(*vertex);
        }
        Some(aabb)
    }

    /// Validates face indices and, on success, returns a witness whose methods
    /// can index into `vertices` and `faces` without per-call bounds checks.
    /// Lifetime-tied to `self`: any later mutation of the mesh invalidates the
    /// witness.
    pub fn validated_faces(&self) -> Result<ValidatedFaces<'_>, Vec<MeshFaceError>> {
        self.validate_faces()?;
        Ok(ValidatedFaces::new_unchecked(self))
    }

    /// Validates face indices as [`Self::validated_faces`] does, returning a
    /// witness that also permits mutation. The returned witness exposes only
    /// mutations that preserve the in-range invariant (winding flips);
    /// operations that could invalidate it (for example removing whole faces)
    /// live on `IndexedMesh` itself.
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
        let fixes = fix::fix_zero_length_edges_impl(self, errors);
        // With only `ZeroLengthEdge` errors, every index is in range; removing
        // whole faces changes neither `vertices.len()` nor the indices of the
        // faces that stay.
        let witness = if only_zero_length {
            Ok(ValidatedFacesMut::new_unchecked(self))
        } else {
            self.validated_faces_mut()
        };
        (fixes, witness)
    }
}
