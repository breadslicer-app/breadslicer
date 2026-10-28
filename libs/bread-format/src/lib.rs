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
//! and other caches are rebuilt after opening, and supports are kept as data
//! from which their mesh is rebuilt. A hollowed model keeps, besides its
//! hollow, how it was made (#161). [`container`] reads and writes chunks
//! without knowing what they hold; [`scene`], [`mesh`], [`supports`] and
//! [`hollows`] are the codecs of the scene graph, its meshes, its supports
//! and its hollows, and [`file`] puts them together with the project
//! settings.

mod bytes;
pub mod container;
pub mod file;
pub mod hollows;
pub mod mesh;
pub mod scene;
pub mod supports;

#[cfg(test)]
mod tests_container;
#[cfg(test)]
mod tests_file;
#[cfg(test)]
mod tests_hollows;
#[cfg(test)]
mod tests_mesh;
#[cfg(test)]
mod tests_scene;
#[cfg(test)]
mod tests_supports;

pub use container::{BreadReader, BreadWriter, Chunk, ChunkTag, ReadBreadError};
pub use file::{
    BreadProject, ReadProjectError, SETTINGS_TAG, SETTINGS_VERSION, WriteBreadError, read_project,
    write_project,
};
pub use hollows::{DecodeHollowsError, HOLLOWS_TAG, HOLLOWS_VERSION, HollowDocument};
pub use mesh::{DecodeMeshError, MESH_TAG, MESH_VERSION, decode_mesh, encode_mesh};
pub use scene::{
    DecodeSceneError, DecodedScene, EncodeSceneError, EncodedScene, SCENE_TAG, SCENE_VERSION,
    decode_scene, encode_scene, encode_scene_emptying,
};
pub use supports::{DecodeSupportsError, SUPPORTS_TAG, SUPPORTS_VERSION, SupportDocument};
