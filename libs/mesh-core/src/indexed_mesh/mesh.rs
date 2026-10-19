/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::aabb::Aabb;
use crate::primitives::{IndexedTriangle, Vertex};

use super::declared_facet_normals::DeclaredFacetNormals;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshMetadata {
    pub name: Option<String>,
    pub header_comment: Option<String>,
    pub declared_facet_normals: Option<DeclaredFacetNormals>,
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
