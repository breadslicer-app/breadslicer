/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::NodeId;

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
    /// A directive defined by the OBJ specification (e.g. `l` line segments,
    /// `p` points, free-form `curv` / `surf`) appeared, but this reader does
    /// not process it.  The file is otherwise valid; only that line is
    /// dropped.
    SkippedUnknownDirective {
        line: usize,
        directive: String,
    },
}

/// Warnings emitted by the STL readers (binary and ASCII).
///
/// The enum is currently empty: STL parsing produces no advisory
/// messages. The type exists so the workspace warning channel has a
/// place for future STL-specific notes (e.g., non-zero attribute
/// bytes in the binary header, atypical solid-name capitalisation),
/// and so the dispatcher in `loader.rs` can wrap per-format STL
/// warnings into `LoadMeshWarning::Stl` once any are added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadStlWarning {}

/// One entry in a scene-level `SharedVertices` warning. Pairs the
/// offending leaf's `NodeId` with the leaf's mesh-metadata name so
/// display surfaces can show a human label without re-borrowing the
/// scene.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedVertexLeaf {
    pub node_id: NodeId,
    pub object_name: Option<String>,
}

/// Warnings derived from the assembled scene after a successful read,
/// regardless of source format. These observations live above the
/// per-format parsers because they describe relationships between
/// `SceneObject`s, not syntactic facts about the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneLoadWarning {
    /// Two or more leaves carry coincident vertex coordinates
    /// (bit-exact f32, with `-0.0` normalised to `0.0`). For OBJ
    /// inputs this typically means the source file declared `v`
    /// directives reused by multiple `o` blocks.
    SharedVertices { leaves: Vec<SharedVertexLeaf> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadMeshWarning {
    Obj(LoadObjWarning),
    Stl(LoadStlWarning),
    Scene(SceneLoadWarning),
}
