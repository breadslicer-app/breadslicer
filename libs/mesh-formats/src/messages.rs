/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_3mf::model::ObjectType;
use sc_scene::NodeId;

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadStlWarning {
    /// The binary reader found content in the 80-byte header that is
    /// not usable text (not valid UTF-8, or carrying control
    /// characters). The header is a comment field, so the document
    /// loads anyway; no mesh name is taken from it.
    HeaderNotText,
}

/// Warnings emitted by the 3MF reader for document content that
/// loads incompletely: the geometry always arrives intact, but a
/// property or object the file carries has no representation in the
/// scene graph yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadThreeMfWarning {
    /// The document defines base-material groups; the scene graph
    /// has no material concept, so their assignments are dropped.
    MaterialsDropped { group_count: usize },
    /// An object resource is never reachable from the build section
    /// (directly or through components), so it loads as nothing.
    UnbuiltObjectSkipped { id: u32, name: Option<String> },
    /// A build item pulled in an object typed support, solidsupport,
    /// or surface; it loads as plain model geometry and the type
    /// distinction is dropped.
    NonModelObjectTypeFlattened { id: u32, object_type: ObjectType },
    /// The document carries slice-extension stacks; the loader takes
    /// the full-resolution meshes and drops the per-layer contours.
    SliceDataIgnored { stack_count: usize },
    /// The build section lists no items; the loaded scene is empty.
    EmptyBuild,
}

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
    ThreeMf(LoadThreeMfWarning),
    Scene(SceneLoadWarning),
}
