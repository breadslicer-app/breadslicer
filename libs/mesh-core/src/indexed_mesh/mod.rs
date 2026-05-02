/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

mod fix;
mod validate;

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
}
