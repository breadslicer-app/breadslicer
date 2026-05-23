/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

mod reader;
mod writer;

use std::io::{Read, Seek};

use sc_mesh_core::Scene;

use crate::error::LoadObjError;
use crate::messages::LoadObjWarning;
use crate::ports::{MeshReader, sealed};

pub use reader::read_obj;
pub use writer::write_obj;

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
