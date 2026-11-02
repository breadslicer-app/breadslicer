/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A whole `.bread` file: the scene chunk, the project settings, the supports
//! and the hollows when there are any, then one mesh chunk per mesh the scene
//! names, in its order.

use std::collections::HashSet;
use std::fmt;
use std::io::{self, Read, Seek, Write};
use std::sync::Arc;

use sc_scene::{NodeKind, Scene};

use crate::container::{BreadReader, BreadWriter, ChunkTag, ReadBreadError};
use crate::hollows::{
    DecodeHollowsError, HOLLOWS_TAG, HOLLOWS_VERSION, HollowDocument, decode_hollows,
    encode_hollows,
};
use crate::mesh::{DecodeMeshError, MESH_TAG, MESH_VERSION, decode_mesh, encode_mesh};
use crate::scene::{
    DecodeSceneError, EncodeSceneError, SCENE_TAG, SCENE_VERSION, decode_scene,
    encode_scene_emptying,
};
use crate::supports::{
    DecodeSupportsError, SUPPORTS_TAG, SUPPORTS_VERSION, SupportDocument, decode_supports,
    encode_supports,
};

/// The tag of the project settings chunk.
pub const SETTINGS_TAG: ChunkTag = ChunkTag(*b"PROJ");

/// The settings chunk version this build writes and reads. Its payload is
/// the application's own versioned settings document.
pub const SETTINGS_VERSION: u16 = 1;

/// The tag of the thumbnail chunk: a small picture of the file, as a PNG
/// (`SPEC.md`, section 8). Optional; this build reads it and does not write it
/// yet.
pub const THUMBNAIL_TAG: ChunkTag = ChunkTag(*b"THMB");

/// The longest thumbnail payload read: a 512 x 512 PNG, the largest the spec
/// allows, stays well under it.
const MAX_THUMBNAIL_LEN: u64 = 4 * 1024 * 1024;

/// The thumbnail of a `.bread` file, the PNG's bytes as stored, reading the
/// chunks' headers and the thumbnail only, never the meshes. `None` when the
/// file has none, or one too long to be a thumbnail. Decoding the picture is
/// the caller's.
pub fn read_thumbnail<R: Read + Seek>(input: R) -> Result<Option<Vec<u8>>, ReadBreadError> {
    let mut reader = BreadReader::new(input)?;
    Ok(reader
        .find_chunk(THUMBNAIL_TAG, MAX_THUMBNAIL_LEN)?
        .map(|chunk| chunk.payload))
}

/// What a `.bread` file holds.
#[derive(Clone, Debug, Default)]
pub struct BreadProject {
    pub scene: Scene,
    /// The project settings (printer, resin, export format...) as the
    /// application writes them; carried without being read here.
    pub settings: Option<Vec<u8>>,
    /// Supports kept as data; their leaves are stored with an empty mesh.
    pub supports: Vec<SupportDocument>,
    /// How hollowed models were made (#161); their leaves keep the hollow.
    pub hollows: Vec<HollowDocument>,
}

/// Why a project could not be written.
#[derive(Debug)]
pub enum WriteBreadError {
    Io(io::Error),
    Scene(EncodeSceneError),
    /// A support document names a leaf the scene does not show.
    SupportLeafMissing,
    /// A hollow document names a leaf the scene does not show.
    HollowLeafMissing,
}

impl fmt::Display for WriteBreadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Scene(err) => write!(f, "{err}"),
            Self::SupportLeafMissing => {
                f.write_str("the supports belong to a part that is not in the scene")
            }
            Self::HollowLeafMissing => {
                f.write_str("a hollow belongs to a part that is not in the scene")
            }
        }
    }
}

impl std::error::Error for WriteBreadError {}

impl From<io::Error> for WriteBreadError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Why a `.bread` file could not be read.
#[derive(Debug)]
pub enum ReadProjectError {
    File(ReadBreadError),
    /// A chunk the project needs was written in a newer layout.
    NewerChunk {
        tag: ChunkTag,
        version: u16,
    },
    /// The file has no scene chunk, or more than one of a single chunk.
    ChunkCount {
        tag: ChunkTag,
        count: usize,
    },
    Mesh(DecodeMeshError),
    Scene(DecodeSceneError),
    Supports(DecodeSupportsError),
    Hollows(DecodeHollowsError),
}

impl fmt::Display for ReadProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(err) => write!(f, "{err}"),
            Self::NewerChunk { tag, version } => write!(
                f,
                "the {tag} chunk was written by a newer Breadslicer (version {version})"
            ),
            Self::ChunkCount { tag, count } => {
                write!(f, "the file holds {count} {tag} chunks")
            }
            Self::Mesh(err) => write!(f, "{err}"),
            Self::Scene(err) => write!(f, "{err}"),
            Self::Supports(err) => write!(f, "{err}"),
            Self::Hollows(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ReadProjectError {}

impl From<ReadBreadError> for ReadProjectError {
    fn from(err: ReadBreadError) -> Self {
        Self::File(err)
    }
}

/// Writes `project` as a `.bread` file to `out`, handing `out` back.
pub fn write_project<W: Write>(project: &BreadProject, out: W) -> Result<W, WriteBreadError> {
    let emptied: HashSet<_> = project.supports.iter().map(|s| s.leaf).collect();
    let encoded =
        encode_scene_emptying(&project.scene, &emptied).map_err(WriteBreadError::Scene)?;
    let supports = encode_supports(&project.supports, |leaf| encoded.nodes.get(&leaf).copied())
        .ok_or(WriteBreadError::SupportLeafMissing)?;
    let hollows = encode_hollows(&project.hollows, |leaf| encoded.nodes.get(&leaf).copied())
        .ok_or(WriteBreadError::HollowLeafMissing)?;

    let mut writer = BreadWriter::new(out)?;
    writer.chunk(SCENE_TAG, SCENE_VERSION, &encoded.payload)?;
    if let Some(settings) = &project.settings {
        writer.chunk(SETTINGS_TAG, SETTINGS_VERSION, settings)?;
    }
    if !project.supports.is_empty() {
        writer.chunk(SUPPORTS_TAG, SUPPORTS_VERSION, &supports)?;
    }
    if !project.hollows.is_empty() {
        writer.chunk(HOLLOWS_TAG, HOLLOWS_VERSION, &hollows)?;
    }
    for mesh in &encoded.meshes {
        writer.chunk(MESH_TAG, MESH_VERSION, &encode_mesh(mesh))?;
    }
    Ok(writer.finish()?)
}

/// Reads the `.bread` file in `input`. Chunks this build does not know are
/// skipped.
pub fn read_project<R: Read>(input: R) -> Result<BreadProject, ReadProjectError> {
    let mut reader = BreadReader::new(input)?;
    let (mut scenes, mut settings, mut supports) = (Vec::new(), Vec::new(), Vec::new());
    let mut hollows = Vec::new();
    let mut meshes = Vec::new();
    while let Some(chunk) = reader.next_chunk()? {
        let supported = match chunk.tag {
            SCENE_TAG => SCENE_VERSION,
            SETTINGS_TAG => SETTINGS_VERSION,
            SUPPORTS_TAG => SUPPORTS_VERSION,
            HOLLOWS_TAG => HOLLOWS_VERSION,
            MESH_TAG => MESH_VERSION,
            _ => continue,
        };
        if chunk.version > supported {
            return Err(ReadProjectError::NewerChunk {
                tag: chunk.tag,
                version: chunk.version,
            });
        }
        match chunk.tag {
            SCENE_TAG => scenes.push(chunk.payload),
            SETTINGS_TAG => settings.push(chunk.payload),
            SUPPORTS_TAG => supports.push(chunk.payload),
            HOLLOWS_TAG => hollows.push(chunk.payload),
            _ => meshes.push(Arc::new(
                decode_mesh(&chunk.payload).map_err(ReadProjectError::Mesh)?,
            )),
        }
    }
    let scene = single(SCENE_TAG, scenes)?.ok_or(ReadProjectError::ChunkCount {
        tag: SCENE_TAG,
        count: 0,
    })?;
    let settings = single(SETTINGS_TAG, settings)?;
    let supports = single(SUPPORTS_TAG, supports)?;
    let hollows = single(HOLLOWS_TAG, hollows)?;

    let decoded = decode_scene(&scene, meshes).map_err(ReadProjectError::Scene)?;
    let is_leaf = |id| {
        matches!(
            decoded.scene.node(id).map(|node| &node.kind),
            Some(NodeKind::Leaf(_))
        )
    };
    let supports = match supports {
        None => Vec::new(),
        Some(payload) => decode_supports(&payload, &decoded.nodes, is_leaf)
            .map_err(ReadProjectError::Supports)?,
    };
    let hollows = match hollows {
        None => Vec::new(),
        Some(payload) => {
            decode_hollows(&payload, &decoded.nodes, is_leaf).map_err(ReadProjectError::Hollows)?
        }
    };
    Ok(BreadProject {
        scene: decoded.scene,
        settings,
        supports,
        hollows,
    })
}

/// The payload of a chunk that appears at most once.
fn single(tag: ChunkTag, chunks: Vec<Vec<u8>>) -> Result<Option<Vec<u8>>, ReadProjectError> {
    if chunks.len() > 1 {
        return Err(ReadProjectError::ChunkCount {
            tag,
            count: chunks.len(),
        });
    }
    Ok(chunks.into_iter().next())
}
