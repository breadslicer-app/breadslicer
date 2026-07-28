/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Format dispatch for writing project files.

use std::io::Write;

use sc_mesh_core::Scene;

use crate::error::WriteProjectError;
use crate::filetypes::ProjectFileType;
use crate::payload::{EmbedProfiles, ProjectPayload};
use crate::threemf::write_threemf_project;

/// Writes `scene` and `payload` as a project file of the given
/// format. With [`EmbedProfiles::No`] the payload's embedded
/// documents are left out and only the profile references are
/// written.
pub fn write_project<W>(
    scene: &Scene,
    payload: &ProjectPayload,
    embed: EmbedProfiles,
    file_type: ProjectFileType,
    writer: &mut W,
) -> Result<(), WriteProjectError>
where
    W: Write,
{
    match file_type {
        ProjectFileType::ThreeMf => {
            write_threemf_project(scene, payload, embed, None, None, writer)
                .map_err(WriteProjectError::ThreeMf)
        }
    }
}
