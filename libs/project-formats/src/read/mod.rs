/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Format dispatch for reading project files.

pub(crate) mod loaded_project;

pub use loaded_project::LoadedProject;

use std::io::{Read, Seek};

use sc_3mf::package::DecompressionProgress;

use crate::error::ReadProjectError;
use crate::filetypes::ProjectFileType;
use crate::threemf::read_threemf_project_with_progress;

/// Reads a project file of the given format.
pub fn read_project<R>(
    reader: &mut R,
    file_type: ProjectFileType,
) -> Result<LoadedProject, ReadProjectError>
where
    R: Read + Seek,
{
    read_project_with_progress(reader, file_type, |_| {})
}

/// [`read_project`], reporting container decompression progress. The ticks
/// cover the container extraction only; document parsing after inflation runs
/// without ticking.
pub fn read_project_with_progress<R>(
    reader: &mut R,
    file_type: ProjectFileType,
    progress: impl FnMut(DecompressionProgress),
) -> Result<LoadedProject, ReadProjectError>
where
    R: Read + Seek,
{
    match file_type {
        ProjectFileType::ThreeMf => {
            read_threemf_project_with_progress(reader, progress).map_err(ReadProjectError::ThreeMf)
        }
    }
}
