/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::fingerprint::geometry_fingerprint::GeometryFingerprint;
use crate::indexed_mesh::mesh::IndexedMesh;

use super::{DeclaredFacetNormals, FacetCountMismatch, RestoreRefusal};

impl DeclaredFacetNormals {
    /// Binds `normals` to the geometry of `mesh`. Fails when their count
    /// differs from the face count of `mesh`.
    pub fn capture(mesh: &IndexedMesh, normals: Vec<[u32; 3]>) -> Result<Self, FacetCountMismatch> {
        check_count(mesh, &normals)?;
        Ok(Self {
            normals: normals.into(),
            fingerprint: mesh.geometry_fingerprint(),
        })
    }

    /// Binds `normals`, read back from storage with the fingerprint `stored`,
    /// to the geometry of `mesh`. Fails with `GeometryChanged` when `stored`
    /// differs from the fingerprint of `mesh`, else with `FacetCountMismatch`
    /// when the count of `normals` differs from the face count of `mesh`.
    pub fn restore(
        mesh: &IndexedMesh,
        normals: Vec<[u32; 3]>,
        stored: GeometryFingerprint,
    ) -> Result<Self, RestoreRefusal> {
        if stored != mesh.geometry_fingerprint() {
            return Err(RestoreRefusal::GeometryChanged);
        }
        check_count(mesh, &normals)?;
        Ok(Self {
            normals: normals.into(),
            fingerprint: stored,
        })
    }
}

fn check_count(mesh: &IndexedMesh, normals: &[[u32; 3]]) -> Result<(), FacetCountMismatch> {
    if normals.len() == mesh.faces.len() {
        Ok(())
    } else {
        Err(FacetCountMismatch {
            facets: normals.len(),
            faces: mesh.faces.len(),
        })
    }
}

impl IndexedMesh {
    /// Returns the facet normals the source file declared while the mesh keeps
    /// the geometry they came with, else `None`. Each call hashes the geometry.
    pub fn declared_facet_normals(&self) -> Option<&[[u32; 3]]> {
        let declared = self.meta.as_ref()?.declared_facet_normals.as_ref()?;
        (declared.fingerprint == self.geometry_fingerprint()).then_some(&*declared.normals)
    }
}
