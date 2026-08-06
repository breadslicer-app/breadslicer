/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

pub mod error_summary;
pub mod fix;
mod half_edges;
pub mod repair;
pub mod topology;
pub mod validate;
pub mod validated;

#[cfg(test)]
mod tests_repair;

pub use error_summary::{ErrorVariantGroup, MeshErrorReport, group_errors_by_variant};
pub use fix::{MeshFix, MeshFixError};
pub use repair::{MeshRepairReport, RepairPasses, RepairRefusal};
pub use topology::{EdgeKey, MeshTopology, edge_key};
pub use validate::{MeshFaceError, MeshVertexError};
pub use validated::{
    MeshEdgeManifoldError, MeshNormalVectorError, ValidatedEdgeManifoldMesh, ValidatedFaces,
    ValidatedFacesMut,
};

use crate::aabb::Aabb;
use crate::primitives::{IndexedTriangle, Vertex};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshMetadata {
    pub name: Option<String>,
    pub header_comment: Option<String>,
}

/// Which face-orientation repairs `fix_faces_orientation` should apply.
///
/// Both flags can be set independently; they correspond to two passes
/// the routine can take over the mesh:
///
/// - `flipped`: detect inconsistently-wound faces (via manifold errors)
///   and reverse their vertex order.
/// - `normals`: recompute the stored normal vector from the triangle's
///   vertices for faces whose normal was wrong (non-unit-length, or
///   misaligned with the geometric normal).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixFacesOptions {
    pub flipped: bool,
    pub normals: bool,
}

/// Indexed triangle mesh. Pure data: no derived state, no caches.
///
/// Spatial-query acceleration (object-space AABB, triangle clusters,
/// future BVH / half-edge / ...) lives in the `sc-mesh-spatial` crate
/// alongside the consumer that owns the cache lifecycle.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexedMesh {
    pub meta: Option<MeshMetadata>,
    pub vertices: Vec<Vertex>,
    pub faces: Vec<IndexedTriangle>,
}

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

    /// Object-space AABB of `self.vertices`. Uncached: each call
    /// walks the vertex array. Callers that hammer this should hold
    /// a `SceneSpatial` from `sc-mesh-spatial` instead, which caches
    /// the result. Returns `None` for an empty mesh.
    pub fn local_aabb(&self) -> Option<Aabb> {
        let mut iter = self.vertices.iter();
        let first = *iter.next()?;
        let mut aabb = Aabb::from_min_max(first, first);
        for vertex in iter {
            aabb.extend(*vertex);
        }
        Some(aabb)
    }

    /// Validate face indices and, on success, return a witness whose
    /// methods can index into `vertices` / `faces` without per-call
    /// bounds checks. Lifetime-tied to `self`: any later mutation of
    /// the mesh invalidates the witness.
    pub fn validated_faces(&self) -> Result<ValidatedFaces<'_>, Vec<MeshFaceError>> {
        self.validate_faces()?;
        Ok(ValidatedFaces::new_unchecked(self))
    }

    /// Mutable counterpart of [`validated_faces`]. The returned
    /// witness exposes only mutations that preserve the in-range
    /// invariant (winding flips, normal recomputes, etc.); operations
    /// that could invalidate it (e.g. removing whole faces) live on
    /// `IndexedMesh` itself.
    pub fn validated_faces_mut(&mut self) -> Result<ValidatedFacesMut<'_>, Vec<MeshFaceError>> {
        self.validate_faces()?;
        Ok(ValidatedFacesMut::new_unchecked(self))
    }

    /// Repair the `ZeroLengthEdge` errors in `errors` (other variants
    /// are ignored), then return a witness for the post-removal mesh.
    ///
    /// If every input error is a `ZeroLengthEdge`, the resulting
    /// mesh's face indices are guaranteed to remain in range:
    /// removing whole faces does not change `vertices.len()` and does
    /// not alter any survivor's `face.vertices[k]`, so the witness
    /// is minted directly without re-running `validate_faces`.
    /// Otherwise, the post-removal mesh is re-validated and the
    /// validation result is returned alongside the applied fixes.
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
        let witness = if only_zero_length {
            Ok(ValidatedFacesMut::new_unchecked(self))
        } else {
            self.validated_faces_mut()
        };
        (fixes, witness)
    }
}
