/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `.bread` project file (#145).
//!
//! A `.bread` file is a short header followed by chunks. Each chunk has a
//! four-byte tag, its own version, and a payload compressed with zstd and
//! guarded by a CRC-32. A reader skips the tags it does not know, so a newer
//! file still opens in an older build as far as that build understands it,
//! and a damaged chunk is reported instead of read.
//!
//! The file holds the scene, not what is computed from it: slices, previews
//! and other caches are rebuilt after opening. [`container`] reads and writes
//! chunks without knowing what they hold; [`scene`] and [`mesh`] are the
//! codecs of the scene graph and its meshes, and [`file`] puts them together.

mod bytes;
pub mod container;
pub mod file;
pub mod mesh;
pub mod scene;

#[cfg(test)]
mod tests_container;
#[cfg(test)]
mod tests_file;
#[cfg(test)]
mod tests_mesh;
#[cfg(test)]
mod tests_scene;

pub use container::{BreadReader, BreadWriter, Chunk, ChunkTag, ReadBreadError};
pub use file::{ReadSceneError, WriteSceneError, read_scene, write_scene};
pub use mesh::{DecodeMeshError, MESH_TAG, MESH_VERSION, decode_mesh, encode_mesh};
pub use scene::{
    DecodeSceneError, EncodeSceneError, EncodedScene, SCENE_TAG, SCENE_VERSION, decode_scene,
    encode_scene,
};
