/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

pub mod diagnose;
mod duplicate_faces;
pub mod error_summary;
pub mod fix;
mod half_edges;
mod mesh;
pub mod repair;
pub mod topology;
pub mod validate;
pub mod validated;
pub mod validated_mut;

#[cfg(test)]
mod tests_collinear_faces;
#[cfg(test)]
mod tests_declared_facet_normals;
#[cfg(test)]
mod tests_diagnose;
#[cfg(test)]
mod tests_duplicate_faces;
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

pub use diagnose::{FaceDiagnosis, MeshDiagnosis};
pub use error_summary::{ErrorVariantGroup, MeshErrorReport, group_errors_by_variant};
pub use fix::MeshFixError;
pub use fix::records::MeshFix;
pub use mesh::declared_facet_normals::{DeclaredFacetNormals, FacetCountMismatch, RestoreRefusal};
pub use mesh::geometry_fingerprint::GeometryFingerprint;
pub use mesh::{IndexedMesh, MeshMetadata};
pub use repair::{MeshRepairReport, RepairPasses, RepairRefusal};
pub use topology::MeshTopology;
pub use validate::{MeshFaceError, MeshVertexError};
pub use validated::{
    MeshEdgeManifoldError, ValidatedEdgeManifoldMesh, ValidatedFaces, ZeroAreaFace,
};
pub use validated_mut::ValidatedFacesMut;
