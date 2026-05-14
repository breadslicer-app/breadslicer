/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed errors for `sc-mesh-formats`.
//!
//! Adapters (CLI, GUI) absorb these into their own error currency at
//! the boundary; the library itself never depends on `anyhow`.
//!
//! Hierarchy:
//! - [`LoadMeshError`] aggregates every failure mode of the
//!   `load_mesh_from_*` entry points: I/O, size cap, format inference,
//!   per-format [`LoadStlError`] / [`LoadObjError`].
//! - [`WriteSceneError`] aggregates every failure mode of
//!   `write_scene`: I/O, filesystem atomic-write steps, empty-scene,
//!   the domain's [`SceneError`], and per-format [`WriteStlError`] /
//!   [`WriteObjError`].
//! - [`InferFileTypeError`] is also exposed standalone for callers
//!   that drive `infer_mesh_file_format_from_extension` directly.

use std::fmt;
use std::num::{ParseFloatError, ParseIntError};
use std::path::PathBuf;
use std::str::Utf8Error;

use sc_mesh_core::SceneGraphError;
use sc_mesh_core::scene::SceneError;

use crate::filetypes::MeshFileType;

// --- top-level: load -------------------------------------------------

#[derive(Debug)]
pub enum LoadMeshError {
    Io(std::io::Error),
    FileTooLarge { actual_bytes: u64, limit_bytes: u64 },
    InferType(InferFileTypeError),
    UnsupportedFormat(MeshFileType),
    Stl(LoadStlError),
    Obj(LoadObjError),
}

impl fmt::Display for LoadMeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error while loading mesh: {e}"),
            Self::FileTooLarge {
                actual_bytes,
                limit_bytes,
            } => write!(
                f,
                "file is {actual_bytes} bytes, which exceeds the {limit_bytes} byte limit",
            ),
            Self::InferType(e) => write!(f, "{e}"),
            Self::UnsupportedFormat(kind) => write!(f, "Unsupported mesh file type: {kind:?}"),
            Self::Stl(e) => write!(f, "{e}"),
            Self::Obj(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadMeshError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::InferType(e) => Some(e),
            Self::Stl(e) => Some(e),
            Self::Obj(e) => Some(e),
            Self::FileTooLarge { .. } | Self::UnsupportedFormat(_) => None,
        }
    }
}

impl From<std::io::Error> for LoadMeshError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<InferFileTypeError> for LoadMeshError {
    fn from(e: InferFileTypeError) -> Self {
        Self::InferType(e)
    }
}

impl From<LoadStlError> for LoadMeshError {
    fn from(e: LoadStlError) -> Self {
        Self::Stl(e)
    }
}

impl From<LoadObjError> for LoadMeshError {
    fn from(e: LoadObjError) -> Self {
        Self::Obj(e)
    }
}

// --- top-level: write ------------------------------------------------

#[derive(Debug)]
pub enum WriteSceneError {
    Io(std::io::Error),
    EmptyScene,
    PathHasNoFileName(PathBuf),
    CreateTempFile {
        path: PathBuf,
        source: std::io::Error,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    Scene(SceneError),
    Graph(SceneGraphError),
    Stl(WriteStlError),
    Obj(WriteObjError),
}

impl fmt::Display for WriteSceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error while writing scene: {e}"),
            Self::EmptyScene => write!(f, "Cannot flatten an empty scene to a single mesh"),
            Self::PathHasNoFileName(p) => {
                write!(f, "Target path {} has no file name", p.display())
            }
            Self::CreateTempFile { path, source } => {
                write!(f, "Failed to create temp file {}: {source}", path.display())
            }
            Self::Rename { from, to, source } => write!(
                f,
                "Failed to rename {} to {}: {source}",
                from.display(),
                to.display(),
            ),
            Self::Scene(e) => write!(f, "{e}"),
            Self::Graph(e) => write!(f, "{e}"),
            Self::Stl(e) => write!(f, "{e}"),
            Self::Obj(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WriteSceneError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::CreateTempFile { source, .. } | Self::Rename { source, .. } => Some(source),
            Self::Scene(e) => Some(e),
            Self::Graph(e) => Some(e),
            Self::Stl(e) => Some(e),
            Self::Obj(e) => Some(e),
            Self::EmptyScene | Self::PathHasNoFileName(_) => None,
        }
    }
}

impl From<std::io::Error> for WriteSceneError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<SceneError> for WriteSceneError {
    fn from(e: SceneError) -> Self {
        Self::Scene(e)
    }
}

impl From<SceneGraphError> for WriteSceneError {
    fn from(e: SceneGraphError) -> Self {
        Self::Graph(e)
    }
}

impl From<WriteStlError> for WriteSceneError {
    fn from(e: WriteStlError) -> Self {
        Self::Stl(e)
    }
}

impl From<WriteObjError> for WriteSceneError {
    fn from(e: WriteObjError) -> Self {
        Self::Obj(e)
    }
}

// --- file-type inference ---------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferFileTypeError {
    UnknownExtension { path: PathBuf },
}

impl fmt::Display for InferFileTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownExtension { path } => write!(
                f,
                "Unable to infer file format from filename extension for \"{}\"",
                path.display(),
            ),
        }
    }
}

impl std::error::Error for InferFileTypeError {}

// --- per-format: STL load --------------------------------------------

#[derive(Debug)]
pub enum LoadStlError {
    Io(std::io::Error),
    Scene(SceneError),
    Truncated {
        what: &'static str,
    },
    DeclaredTriangleCountExceedsCap {
        declared: usize,
        cap: usize,
    },
    HeaderNotAscii,
    HeaderNotNulTerminated,
    HeaderNotUtf8(Utf8Error),
    AsciiHeaderMissing,
    AsciiEmptyInput,
    AsciiInvalidFacetHeader {
        tokens: Vec<String>,
    },
    AsciiInvalidVertexLine {
        tokens: Vec<String>,
    },
    AsciiUnexpected {
        expected: &'static str,
        got: Vec<String>,
    },
    AsciiEofWhileExpecting {
        what: &'static str,
    },
    InvalidFloat {
        token: String,
        source: ParseFloatError,
    },
    NonFiniteFloat {
        what: &'static str,
        value: f32,
    },
}

impl fmt::Display for LoadStlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "STL I/O error: {e}"),
            Self::Scene(e) => write!(f, "STL reader: {e}"),
            Self::Truncated { what } => write!(
                f,
                "The STL file is truncated or malformed: it ended before {what} could be read",
            ),
            Self::DeclaredTriangleCountExceedsCap { declared, cap } => write!(
                f,
                "declared triangle count {declared} exceeds the {cap} maximum supported by this reader",
            ),
            Self::HeaderNotAscii => write!(f, "Mesh name is not ASCII"),
            Self::HeaderNotNulTerminated => write!(f, "Mesh name is not NUL-terminated"),
            Self::HeaderNotUtf8(e) => write!(f, "Mesh name is not valid UTF-8: {e}"),
            Self::AsciiHeaderMissing => write!(f, "Ascii STL does not start with \"solid \""),
            Self::AsciiEmptyInput => write!(f, "File seems to be empty"),
            Self::AsciiInvalidFacetHeader { tokens } => {
                write!(f, "invalid facet header: {tokens:?}")
            }
            Self::AsciiInvalidVertexLine { tokens } => {
                write!(f, "Expected vertex f32 f32 f32, got {tokens:?}")
            }
            Self::AsciiUnexpected { expected, got } => {
                write!(f, "expected {expected:?}, got {got:?}")
            }
            Self::AsciiEofWhileExpecting { what } => {
                write!(f, "EOF while expecting {what}")
            }
            Self::InvalidFloat { token, source } => {
                write!(f, "invalid float '{token}': {source}")
            }
            Self::NonFiniteFloat { what, value } => write!(
                f,
                "expected finite f32 for {what}, got {value} which is {:?}",
                value.classify(),
            ),
        }
    }
}

impl std::error::Error for LoadStlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Scene(e) => Some(e),
            Self::HeaderNotUtf8(e) => Some(e),
            Self::InvalidFloat { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LoadStlError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<SceneError> for LoadStlError {
    fn from(e: SceneError) -> Self {
        Self::Scene(e)
    }
}

// --- per-format: OBJ load --------------------------------------------

#[derive(Debug)]
pub enum LoadObjError {
    Io(std::io::Error),
    Scene(SceneError),
    InvalidFloat {
        line: usize,
        token: String,
        source: ParseFloatError,
    },
    NonFiniteFloat {
        line: usize,
        value: f32,
    },
    VertexCoordCountInvalid {
        line: usize,
        got: usize,
    },
    VertexWZero {
        line: usize,
    },
    VertexCountExceedsLimit {
        line: usize,
        limit: usize,
    },
    OutOfMemoryVertices {
        line: usize,
    },
    OutOfMemoryFaces {
        line: usize,
    },
    FaceTooFewVertices {
        line: usize,
        got: usize,
    },
    FaceExceedsTokenLimit {
        line: usize,
        limit: usize,
    },
    FaceIndexZero {
        line: usize,
    },
    InvalidFaceIndex {
        line: usize,
        token: String,
        source: ParseIntError,
    },
    NegativeFaceIndexOutOfRange {
        line: usize,
        index: i64,
    },
    FaceIndexOutOfRange {
        line: usize,
        index: i64,
        vertex_count: usize,
    },
    UnrecognizedDirective {
        line: usize,
        directive: String,
    },
}

impl fmt::Display for LoadObjError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "OBJ I/O error: {e}"),
            Self::Scene(e) => write!(f, "OBJ reader: {e}"),
            Self::InvalidFloat {
                line,
                token,
                source,
            } => write!(f, "line {line}: invalid float '{token}': {source}"),
            Self::NonFiniteFloat { line, value } => {
                write!(f, "line {line}: expected finite float, got {value}")
            }
            Self::VertexCoordCountInvalid { line, got } => write!(
                f,
                "line {line}: vertex requires 3 or 4 coordinates, got {got}",
            ),
            Self::VertexWZero { line } => {
                write!(f, "line {line}: vertex w coordinate must not be zero")
            }
            Self::VertexCountExceedsLimit { line, limit } => {
                write!(f, "line {line}: file declares more than {limit} vertices",)
            }
            Self::OutOfMemoryVertices { line } => {
                write!(f, "line {line}: out of memory while parsing OBJ vertices")
            }
            Self::OutOfMemoryFaces { line } => write!(
                f,
                "line {line}: out of memory while expanding face into triangles",
            ),
            Self::FaceTooFewVertices { line, got } => write!(
                f,
                "line {line}: face requires at least 3 vertices, got {got}",
            ),
            Self::FaceExceedsTokenLimit { line, limit } => {
                write!(f, "line {line}: face exceeds the {limit}-vertex limit")
            }
            Self::FaceIndexZero { line } => {
                write!(f, "line {line}: face index must not be zero")
            }
            Self::InvalidFaceIndex {
                line,
                token,
                source,
            } => write!(f, "line {line}: invalid face index '{token}': {source}"),
            Self::NegativeFaceIndexOutOfRange { line, index } => {
                write!(f, "line {line}: negative face index {index} out of range")
            }
            Self::FaceIndexOutOfRange {
                line,
                index,
                vertex_count,
            } => {
                if *index < 0 {
                    write!(
                        f,
                        "line {line}: negative index {index} out of range (only {vertex_count} vertices defined)",
                    )
                } else {
                    write!(
                        f,
                        "line {line}: vertex index {index} out of range (only {vertex_count} vertices defined)",
                    )
                }
            }
            Self::UnrecognizedDirective { line, directive } => {
                write!(f, "line {line}: unrecognized directive '{directive}'")
            }
        }
    }
}

impl std::error::Error for LoadObjError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Scene(e) => Some(e),
            Self::InvalidFloat { source, .. } => Some(source),
            Self::InvalidFaceIndex { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LoadObjError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<SceneError> for LoadObjError {
    fn from(e: SceneError) -> Self {
        Self::Scene(e)
    }
}

// --- per-format: STL write -------------------------------------------

#[derive(Debug)]
pub enum WriteStlError {
    Io(std::io::Error),
    FaceCountExceedsU32 { count: usize },
}

impl fmt::Display for WriteStlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "STL writer I/O error: {e}"),
            Self::FaceCountExceedsU32 { count } => {
                write!(f, "binary STL writer: face count {count} exceeds u32::MAX",)
            }
        }
    }
}

impl std::error::Error for WriteStlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::FaceCountExceedsU32 { .. } => None,
        }
    }
}

impl From<std::io::Error> for WriteStlError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// --- per-format: OBJ write -------------------------------------------

#[derive(Debug)]
pub enum WriteObjError {
    Io(std::io::Error),
    Scene(SceneError),
    Graph(SceneGraphError),
    FaceIndexOverflow { local: u32, offset: u32 },
    VertexCountExceedsU32 { vertex_count: usize },
    VertexOffsetOverflow { offset: u32, added: u32 },
}

impl fmt::Display for WriteObjError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "OBJ writer I/O error: {e}"),
            Self::Scene(e) => write!(f, "OBJ writer: {e}"),
            Self::Graph(e) => write!(f, "OBJ writer: {e}"),
            Self::FaceIndexOverflow { local, offset } => write!(
                f,
                "OBJ writer: face vertex index overflow (local={local}, offset={offset})",
            ),
            Self::VertexCountExceedsU32 { vertex_count } => write!(
                f,
                "OBJ writer: mesh has {vertex_count} vertices, exceeds u32::MAX",
            ),
            Self::VertexOffsetOverflow { offset, added } => write!(
                f,
                "OBJ writer: cumulative vertex offset overflows u32 (offset={offset}, +{added})",
            ),
        }
    }
}

impl std::error::Error for WriteObjError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Scene(e) => Some(e),
            Self::Graph(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WriteObjError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<SceneError> for WriteObjError {
    fn from(e: SceneError) -> Self {
        Self::Scene(e)
    }
}

impl From<SceneGraphError> for WriteObjError {
    fn from(e: SceneGraphError) -> Self {
        Self::Graph(e)
    }
}
