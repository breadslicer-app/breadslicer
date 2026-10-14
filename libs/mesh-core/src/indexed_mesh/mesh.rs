/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

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
