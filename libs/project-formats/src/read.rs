/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Format dispatch for reading project files.

use std::io::{Read, Seek};

use sc_mesh_core::Scene;

use crate::error::ReadProjectError;
use crate::filetypes::ProjectFileType;
use crate::messages::ReadProjectWarning;
use crate::payload::ProjectPayload;
use crate::threemf::read_threemf_project;

/// Everything a project read produces: the scene, the non-fatal
/// warnings, and the project payload when the file carries one. A
/// file of a project-capable format without project data (a plain
/// 3MF model, for instance) loads with `payload` set to `None`.
#[derive(Debug)]
pub struct LoadedProject {
    pub scene: Scene,
    pub warnings: Vec<ReadProjectWarning>,
    pub payload: Option<ProjectPayload>,
}

/// Reads a project file of the given format.
pub fn read_project<R>(
    reader: &mut R,
    file_type: ProjectFileType,
) -> Result<LoadedProject, ReadProjectError>
where
    R: Read + Seek,
{
    match file_type {
        ProjectFileType::ThreeMf => read_threemf_project(reader).map_err(ReadProjectError::ThreeMf),
    }
}
