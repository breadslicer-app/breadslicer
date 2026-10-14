/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fmt;
use std::sync::Arc;

use super::fingerprint::geometry_fingerprint::GeometryFingerprint;
use super::mesh::IndexedMesh;

/// The facet normals that a source file declared for a mesh: the f32 bit
/// patterns of 1 normal per face, in face order, kept as read. They are file
/// data, not geometry, and can be read only while the mesh keeps the geometry
/// they came with, through [`IndexedMesh::declared_facet_normals`].
#[derive(Clone, PartialEq, Eq)]
pub struct DeclaredFacetNormals {
    normals: Arc<[[u32; 3]]>,
    // Fingerprint of the mesh the normals were built for.
    fingerprint: GeometryFingerprint,
}

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

    /// Returns the fingerprint of the geometry the normals were bound to.
    pub fn fingerprint(&self) -> GeometryFingerprint {
        self.fingerprint
    }
}

// Prints the facet count instead of the normals, which can take megabytes.
impl fmt::Debug for DeclaredFacetNormals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeclaredFacetNormals")
            .field("facets", &self.normals.len())
            .field("fingerprint", &self.fingerprint)
            .finish()
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

/// A list of facet normals whose length differs from the face count of the mesh
/// it was given for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacetCountMismatch {
    pub facets: usize,
    pub faces: usize,
}

impl fmt::Display for FacetCountMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "facet normal count {} differs from face count {}",
            self.facets, self.faces
        )
    }
}

impl std::error::Error for FacetCountMismatch {}

/// Why [`DeclaredFacetNormals::restore`] refused stored normals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreRefusal {
    /// The geometry of the mesh differs from the geometry the normals were
    /// stored with.
    GeometryChanged,
    /// The geometry matches, but the number of normals does not.
    FacetCountMismatch(FacetCountMismatch),
}

impl From<FacetCountMismatch> for RestoreRefusal {
    fn from(mismatch: FacetCountMismatch) -> Self {
        Self::FacetCountMismatch(mismatch)
    }
}

impl fmt::Display for RestoreRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GeometryChanged => write!(
                f,
                "the mesh geometry differs from the geometry the facet normals were stored with"
            ),
            Self::FacetCountMismatch(mismatch) => write!(f, "{mismatch}"),
        }
    }
}

impl std::error::Error for RestoreRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GeometryChanged => None,
            Self::FacetCountMismatch(mismatch) => Some(mismatch),
        }
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
