/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

mod binding;
pub(super) mod declared_facet_normals;
mod fingerprint;
pub(super) mod geometry_fingerprint;

use crate::aabb::Aabb;
use crate::primitives::{IndexedTriangle, Vertex};

use declared_facet_normals::DeclaredFacetNormals;

/// What a mesh that is not a model is (#228): set where the mesh is made
/// and when a project opens, never read from its name, so renaming a mesh
/// does not change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeshRole {
    /// Supports Breadslicer generated, drawn from their own data.
    Supports,
    /// Supports that came with a pre-supported file (#258).
    ImportedSupports,
    /// One raft under the supports of a whole plate (#556).
    Raft,
}

impl MeshRole {
    /// The role a mesh named `name` had in a project saved before roles
    /// were kept (#228): those projects told supports and rafts by name
    /// alone. Never used on a mesh imported from a mesh file.
    pub fn from_legacy_name(name: &str) -> Option<Self> {
        match name {
            "Supports" | "Supports (DragonFruit)" => Some(Self::Supports),
            "Imported supports" => Some(Self::ImportedSupports),
            "Raft" => Some(Self::Raft),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshMetadata {
    pub name: Option<String>,
    pub header_comment: Option<String>,
    pub declared_facet_normals: Option<DeclaredFacetNormals>,
    /// What the mesh is when it is not a model; `None` for a model.
    pub role: Option<MeshRole>,
}

/// Indexed triangle mesh. Pure data: no derived state, no caches.
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
}
