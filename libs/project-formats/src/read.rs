/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Format dispatch for reading project files.

use std::io::{Read, Seek};

use sc_3mf::package::DecompressionProgress;
use sc_mesh_core::Scene;
use sc_sl1cer::contour::ContourStack;

use crate::error::ReadProjectError;
use crate::filetypes::ProjectFileType;
use crate::messages::ReadProjectWarning;
use crate::payload::ProjectPayload;
use crate::threemf::read_threemf_project_with_progress;

/// Everything a project read produces: the scene, the non-fatal
/// warnings, the project payload when the file carries one, the
/// persisted slice geometry and the opaque validity token for it when
/// present. A file of a project-capable format without project data (a
/// plain 3MF model, for instance) loads with `payload`,
/// `slice_fingerprint`, and `slices` set to `None`.
#[derive(Debug)]
pub struct LoadedProject {
    pub scene: Scene,
    pub warnings: Vec<ReadProjectWarning>,
    pub payload: Option<ProjectPayload>,
    /// The file's persisted slice geometry, mapped back to a contour
    /// stack for the viewer. `None` when the file carried no slices or
    /// its slice part could not be read (a warning is recorded then).
    /// The caller must still revalidate it against `slice_fingerprint`
    /// before trusting it.
    pub slices: Option<ContourStack>,
    /// Opaque validity token for the file's persisted slices, returned
    /// verbatim. `None` when the file carried no such token.
    pub slice_fingerprint: Option<String>,
}

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

/// [`read_project`], reporting container decompression progress. The
/// ticks cover the container extraction only; document parsing after
/// inflation runs without ticking.
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
