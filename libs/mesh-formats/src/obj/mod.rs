/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

mod reader;
#[cfg(test)]
mod tests_reader_equivalence;
mod writer;

use std::io::{Read, Seek, Write};

use sc_mesh_core::Scene;

use crate::error::{LoadObjError, WriteSceneError};
use crate::messages::LoadObjWarning;
use crate::ports::{MeshReader, MeshWriter, WriteSceneProgress, sealed};
use crate::scene_writer::FlattenWarning;

pub use reader::read_obj;
pub use writer::{write_obj, write_obj_with_progress};

/// Format marker for the OBJ format. Implements [`MeshReader`] via
/// static dispatch; the dispatcher in [`crate::loader`] invokes
/// `Obj::read(reader)`.
pub struct Obj;

impl sealed::Sealed for Obj {}

impl MeshReader for Obj {
    type Error = LoadObjError;
    type Warning = LoadObjWarning;

    fn read<R: Read + Seek>(reader: &mut R) -> Result<(Scene, Vec<LoadObjWarning>), LoadObjError> {
        read_obj(reader)
    }
}

impl MeshWriter for Obj {
    type Error = WriteSceneError;
    type Warning = FlattenWarning;

    fn write_with_progress<W: Write, F: FnMut(WriteSceneProgress)>(
        scene: &Scene,
        writer: &mut W,
        progress: F,
    ) -> Result<Vec<FlattenWarning>, WriteSceneError> {
        write_obj_with_progress(scene, writer, progress)?;
        Ok(Vec::new())
    }
}
