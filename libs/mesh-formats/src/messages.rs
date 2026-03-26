/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadObjWarning {
    NoVertices,
    NoFaces,
    SkippedTextureCoordinates { line: usize },
    SkippedVertexNormal { line: usize },
    SkippedGroup { line: usize, name: String },
    SkippedSmoothingGroup { line: usize },
    SkippedMaterialReference { line: usize, material: String },
    SkippedMaterialLibrary { line: usize, path: String },
    SkippedComment { line: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadMeshWarning {
    Obj(LoadObjWarning),
}
