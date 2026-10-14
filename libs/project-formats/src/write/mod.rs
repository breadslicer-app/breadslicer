/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Format dispatch for writing project files.

pub(crate) mod progress;

pub use progress::WriteProgress;

use std::io::Write;

use sc_scene::Scene;

use crate::error::WriteProjectError;
use crate::filetypes::ProjectFileType;
use crate::payload::{EmbedProfiles, ProjectPayload};
use crate::slices::PlateSlices;
use crate::threemf::write_threemf_project_with_progress;

/// Writes `scene` and `payload` as a project file of the given format. With
/// [`EmbedProfiles::No`] the payload's embedded documents are left out and only
/// the profile references are written.
///
/// `slices` and `slice_fingerprint` persist a computed slice result: the merged
/// plate stack plus the opaque validity token that is recomputed and checked on
/// load. They belong together, pass both or neither. A format that cannot carry
/// slices drops them.
pub fn write_project<W>(
    scene: &Scene,
    payload: &ProjectPayload,
    embed: EmbedProfiles,
    slices: Option<&PlateSlices>,
    slice_fingerprint: Option<&str>,
    file_type: ProjectFileType,
    writer: &mut W,
) -> Result<(), WriteProjectError>
where
    W: Write,
{
    write_project_with_progress(
        scene,
        payload,
        embed,
        slices,
        slice_fingerprint,
        file_type,
        writer,
        |_| {},
    )
}

/// [`write_project`], reporting serialize and compress progress through
/// `progress`. The heavy work of a large save lives in these 2 phases; a caller
/// running the write off the UI thread polls the reported counts to drive a
/// determinate progress display.
#[allow(clippy::too_many_arguments)]
pub fn write_project_with_progress<W>(
    scene: &Scene,
    payload: &ProjectPayload,
    embed: EmbedProfiles,
    slices: Option<&PlateSlices>,
    slice_fingerprint: Option<&str>,
    file_type: ProjectFileType,
    writer: &mut W,
    progress: impl FnMut(WriteProgress),
) -> Result<(), WriteProjectError>
where
    W: Write,
{
    match file_type {
        ProjectFileType::ThreeMf => write_threemf_project_with_progress(
            scene,
            payload,
            embed,
            slices,
            slice_fingerprint,
            writer,
            progress,
        )
        .map_err(WriteProjectError::ThreeMf),
    }
}
