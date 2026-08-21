/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Reader / writer ports for mesh file formats.
//!
//! Each format that this crate supports implements [`MeshReader`] (for
//! parsing) and / or [`MeshWriter`] (for serialisation) on its own
//! zero-sized type (e.g., `StlBinReader`, `ObjReader`). The dispatcher
//! in [`crate::loader::load_mesh_from_reader`] consumes them through
//! static dispatch; there is no `dyn` involved.
//!
//! Both traits are sealed: external crates can call the methods but
//! cannot add new impls. Cross-crate format support (planned
//! `sla-formats`) lives in its own crate with its own parallel sealed
//! traits and its own warning enum; the adapter (CLI / GUI) unions
//! the two crates at the file-family level.

use std::io::{Read, Seek, Write};

use sc_scene::Scene;

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// A format-specific mesh parser.
///
/// Implementors are unit structs that capture only the format
/// identity. `read` is an associated function (no `self`), so callers
/// invoke it as `StlBinReader::read(reader)`.
///
/// `Warning` is the per-format warning enum (e.g., `LoadObjWarning`).
/// The dispatcher wraps these into [`crate::messages::LoadMeshWarning`]
/// at the workspace boundary.
pub trait MeshReader: sealed::Sealed {
    type Error: std::error::Error + Send + Sync + 'static;
    type Warning;

    fn read<R: Read + Seek>(reader: &mut R) -> Result<(Scene, Vec<Self::Warning>), Self::Error>;
}

/// Progress reported while a scene is serialised, in the order the
/// phases run. Not every format visits every phase: the STL variants
/// flatten first, 3MF compresses after serialising, OBJ only
/// serialises. Each report carries `(done, total)` in phase-specific
/// units: merge steps for `Flatten`, elements (faces, or vertex and
/// face lines) for the plain-text `Serialize` phases, bytes for the
/// 3MF `Serialize` and `Compress` phases. The 3MF serialise total is
/// an estimate, so a consumer deriving a fraction should clamp it.
/// `Write` brackets the final flush, fsync and rename performed by
/// [`crate::scene_writer::write_scene_with_progress`]; the per-format
/// writers never report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteSceneProgress {
    /// Merging the scene into a single mesh (STL formats only).
    Flatten { done: u64, total: u64 },
    /// Serialising geometry into the output stream.
    Serialize { done: u64, total: u64 },
    /// Compressing the assembled package (3MF only).
    Compress { done: u64, total: u64 },
    /// Flushing, fsyncing and renaming the temp file over the target.
    Write { done: u64, total: u64 },
}

/// A format-specific mesh serialiser.
///
/// Like [`MeshReader`], implementors are unit structs and the methods
/// are associated functions. `Warning` is the per-format warning enum
/// surfaced by the write step (for STL formats this is
/// [`crate::scene_writer::FlattenWarning`], emitted when the scene
/// must be merged before serialisation; OBJ returns an empty vec
/// because it preserves per-leaf object boundaries on disk).
///
/// `Reference` nodes expand: each occurrence of an referenced subtree
/// serialises under its composed call-site transform. Expanding
/// walks bound their occurrence count against
/// [`sc_scene::MAX_EXPANDED_LEAF_COUNT`] before serialising.
pub trait MeshWriter: sealed::Sealed {
    type Error: std::error::Error + Send + Sync + 'static;
    type Warning;

    /// Serialises `scene` into `writer`, reporting the format's phases
    /// through `progress` (see [`WriteSceneProgress`] for the units).
    fn write_with_progress<W: Write, F: FnMut(WriteSceneProgress)>(
        scene: &Scene,
        writer: &mut W,
        progress: F,
    ) -> Result<Vec<Self::Warning>, Self::Error>;

    /// [`MeshWriter::write_with_progress`] without progress reporting.
    fn write<W: Write>(scene: &Scene, writer: &mut W) -> Result<Vec<Self::Warning>, Self::Error> {
        Self::write_with_progress(scene, writer, |_| {})
    }
}
