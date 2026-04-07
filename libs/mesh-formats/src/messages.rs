/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadObjWarning {
    NoVertices,
    NoFaces,
    SkippedTextureCoordinates {
        line: usize,
    },
    SkippedVertexNormal {
        line: usize,
    },
    SkippedGroup {
        line: usize,
        name: String,
    },
    SkippedSmoothingGroup {
        line: usize,
    },
    SkippedMaterialReference {
        line: usize,
        material: String,
    },
    SkippedMaterialLibrary {
        line: usize,
        path: String,
    },
    SkippedComment {
        line: usize,
    },
    /// One or more vertices are shared between this object and a previously
    /// loaded object.  The OBJ format allows this, but it usually indicates
    /// that the file was not exported cleanly for use as separate meshes.
    SharedVertices {
        object_name: Option<String>,
        shared_indices: Vec<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadMeshWarning {
    Obj(LoadObjWarning),
}
