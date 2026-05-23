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

use sc_mesh_core::Scene;

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

/// A format-specific mesh serialiser.
///
/// Like [`MeshReader`], implementors are unit structs and `write` is
/// an associated function. `Warning` is the per-format warning enum
/// surfaced by the write step (for STL formats this is
/// [`crate::scene_writer::FlattenWarning`], emitted when the scene
/// must be merged before serialisation; OBJ returns an empty vec
/// because it preserves per-leaf object boundaries on disk).
pub trait MeshWriter: sealed::Sealed {
    type Error: std::error::Error + Send + Sync + 'static;
    type Warning;

    fn write<W: Write>(scene: &Scene, writer: &mut W) -> Result<Vec<Self::Warning>, Self::Error>;
}
