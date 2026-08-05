/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! 3MF support: adapts between `sc-3mf` documents and [`Scene`]
//! graphs.
//!
//! The `sc-3mf` crate owns the container (OPC/ZIP) and the model XML;
//! this module owns the domain conversion. On read, objects become
//! mesh resources and build items become leaves (or groups, for
//! component objects) under one identity-Group scene root, with
//! document lengths scaled to millimetres and 3MF's row-vector
//! transforms mapped to [`sc_mesh_core::transform::Transform3D`]
//! values. On write, the same mapping runs in reverse, preserving
//! shared meshes and group structure instead of flattening;
//! referenced subtrees serialise once and are placed per
//! occurrence through components and build items.

mod reader;
mod writer;

use std::io::{Read, Seek, Write};

use sc_mesh_core::Scene;

use crate::error::{LoadThreeMfError, WriteSceneError};
use crate::messages::LoadThreeMfWarning;
use crate::ports::{MeshReader, MeshWriter, WriteSceneProgress, sealed};
use crate::scene_writer::FlattenWarning;

pub use reader::{
    MAX_COMPONENT_DEPTH, MAX_EXPANDED_LEAF_COUNT, read_threemf, read_threemf_from_package,
};
pub use writer::{
    mesh_to_model_data, scene_to_model_document, write_threemf, write_threemf_with_progress,
};

/// Format marker for 3MF packages. Implements [`MeshReader`] via
/// static dispatch; the dispatcher in [`crate::loader`] invokes
/// `ThreeMf::read(reader)`.
pub struct ThreeMf;

impl sealed::Sealed for ThreeMf {}

impl MeshReader for ThreeMf {
    type Error = LoadThreeMfError;
    type Warning = LoadThreeMfWarning;

    fn read<R: Read + Seek>(
        reader: &mut R,
    ) -> Result<(Scene, Vec<LoadThreeMfWarning>), LoadThreeMfError> {
        read_threemf(reader)
    }
}

impl MeshWriter for ThreeMf {
    type Error = WriteSceneError;
    type Warning = FlattenWarning;

    fn write_with_progress<W: Write, F: FnMut(WriteSceneProgress)>(
        scene: &Scene,
        writer: &mut W,
        progress: F,
    ) -> Result<Vec<FlattenWarning>, WriteSceneError> {
        write_threemf_with_progress(scene, writer, progress).map_err(WriteSceneError::from)?;
        // Nothing is flattened or dropped: meshes, groups and
        // transforms all survive (names survive cleaned), so there
        // is never a warning.
        Ok(Vec::new())
    }
}
