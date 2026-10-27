/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A whole `.bread` file: the scene chunk, the project settings and the
//! supports when there are any, then one mesh chunk per mesh the scene names,
//! in its order.

use std::collections::HashSet;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;

use sc_scene::{NodeKind, Scene};

use crate::container::{BreadReader, BreadWriter, ChunkTag, ReadBreadError};
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

/// What a `.bread` file holds.
#[derive(Clone, Debug, Default)]
pub struct BreadProject {
    pub scene: Scene,
    /// The project settings (printer, resin, export format...) as the
    /// application writes them; carried without being read here.
    pub settings: Option<Vec<u8>>,
    /// Supports kept as data; their leaves are stored with an empty mesh.
    pub supports: Vec<SupportDocument>,
}

/// Why a project could not be written.
#[derive(Debug)]
pub enum WriteBreadError {
    Io(io::Error),
    Scene(EncodeSceneError),
    /// A support document names a leaf the scene does not show.
    SupportLeafMissing,
}

impl fmt::Display for WriteBreadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Scene(err) => write!(f, "{err}"),
            Self::SupportLeafMissing => {
                f.write_str("the supports belong to a part that is not in the scene")
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

    let mut writer = BreadWriter::new(out)?;
    writer.chunk(SCENE_TAG, SCENE_VERSION, &encoded.payload)?;
    if let Some(settings) = &project.settings {
        writer.chunk(SETTINGS_TAG, SETTINGS_VERSION, settings)?;
    }
    if !project.supports.is_empty() {
        writer.chunk(SUPPORTS_TAG, SUPPORTS_VERSION, &supports)?;
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
    let mut meshes = Vec::new();
    while let Some(chunk) = reader.next_chunk()? {
        let supported = match chunk.tag {
            SCENE_TAG => SCENE_VERSION,
            SETTINGS_TAG => SETTINGS_VERSION,
            SUPPORTS_TAG => SUPPORTS_VERSION,
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

    let decoded = decode_scene(&scene, meshes).map_err(ReadProjectError::Scene)?;
    let supports = match supports {
        None => Vec::new(),
        Some(payload) => decode_supports(&payload, &decoded.nodes, |id| {
            matches!(
                decoded.scene.node(id).map(|node| &node.kind),
                Some(NodeKind::Leaf(_))
            )
        })
        .map_err(ReadProjectError::Supports)?,
    };
    Ok(BreadProject {
        scene: decoded.scene,
        settings,
        supports,
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
