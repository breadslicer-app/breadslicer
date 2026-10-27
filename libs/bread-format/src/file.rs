/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A whole `.bread` file: the scene chunk, then one mesh chunk per mesh it
//! names, in its order.

use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;

use sc_scene::Scene;

use crate::container::{BreadReader, BreadWriter, ChunkTag, ReadBreadError};
use crate::mesh::{DecodeMeshError, MESH_TAG, MESH_VERSION, decode_mesh, encode_mesh};
use crate::scene::{
    DecodeSceneError, EncodeSceneError, SCENE_TAG, SCENE_VERSION, decode_scene, encode_scene,
};

/// Why a scene could not be written.
#[derive(Debug)]
pub enum WriteSceneError {
    Io(io::Error),
    Scene(EncodeSceneError),
}

impl fmt::Display for WriteSceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Scene(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for WriteSceneError {}

impl From<io::Error> for WriteSceneError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Why a `.bread` file's scene could not be read.
#[derive(Debug)]
pub enum ReadSceneError {
    File(ReadBreadError),
    /// A chunk the scene needs was written in a newer layout.
    NewerChunk {
        tag: ChunkTag,
        version: u16,
    },
    /// The file has no scene chunk, or more than one.
    SceneChunkCount(usize),
    Mesh(DecodeMeshError),
    Scene(DecodeSceneError),
}

impl fmt::Display for ReadSceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(err) => write!(f, "{err}"),
            Self::NewerChunk { tag, version } => write!(
                f,
                "the {tag} chunk was written by a newer Breadslicer (version {version})"
            ),
            Self::SceneChunkCount(0) => f.write_str("the file holds no scene"),
            Self::SceneChunkCount(n) => write!(f, "the file holds {n} scenes"),
            Self::Mesh(err) => write!(f, "{err}"),
            Self::Scene(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ReadSceneError {}

impl From<ReadBreadError> for ReadSceneError {
    fn from(err: ReadBreadError) -> Self {
        Self::File(err)
    }
}

/// Writes `scene` as a `.bread` file to `out`, handing `out` back.
pub fn write_scene<W: Write>(scene: &Scene, out: W) -> Result<W, WriteSceneError> {
    let encoded = encode_scene(scene).map_err(WriteSceneError::Scene)?;
    let mut writer = BreadWriter::new(out)?;
    writer.chunk(SCENE_TAG, SCENE_VERSION, &encoded.payload)?;
    for mesh in &encoded.meshes {
        writer.chunk(MESH_TAG, MESH_VERSION, &encode_mesh(mesh))?;
    }
    Ok(writer.finish()?)
}

/// Reads the scene of the `.bread` file in `input`. Chunks this build does
/// not know are skipped.
pub fn read_scene<R: Read>(input: R) -> Result<Scene, ReadSceneError> {
    let mut reader = BreadReader::new(input)?;
    let mut scenes = Vec::new();
    let mut meshes = Vec::new();
    while let Some(chunk) = reader.next_chunk()? {
        let supported = match chunk.tag {
            SCENE_TAG => SCENE_VERSION,
            MESH_TAG => MESH_VERSION,
            _ => continue,
        };
        if chunk.version > supported {
            return Err(ReadSceneError::NewerChunk {
                tag: chunk.tag,
                version: chunk.version,
            });
        }
        if chunk.tag == SCENE_TAG {
            scenes.push(chunk.payload);
        } else {
            meshes.push(Arc::new(
                decode_mesh(&chunk.payload).map_err(ReadSceneError::Mesh)?,
            ));
        }
    }
    let [scene] = scenes.as_slice() else {
        return Err(ReadSceneError::SceneChunkCount(scenes.len()));
    };
    decode_scene(scene, meshes).map_err(ReadSceneError::Scene)
}
