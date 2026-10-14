/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_scene::Scene;
use sc_sl1cer::contour::ContourStack;

use crate::messages::ReadProjectWarning;
use crate::payload::ProjectPayload;

/// Everything a project read produces: the scene, the non-fatal warnings, the
/// project payload when the file carries one, the persisted slice geometry and
/// the opaque validity token for it when present. A file of a project-capable
/// format without project data (a plain 3MF model, for instance) loads with
/// `payload`, `slice_fingerprint`, and `slices` set to `None`.
#[derive(Debug)]
pub struct LoadedProject {
    pub scene: Scene,
    pub warnings: Vec<ReadProjectWarning>,
    pub payload: Option<ProjectPayload>,
    /// The file's persisted slice geometry, mapped back to a contour stack for
    /// the viewer. `None` when the file carried no slices or its slice part
    /// could not be read. The caller must still revalidate it against
    /// `slice_fingerprint` before trusting it.
    pub slices: Option<ContourStack>,
    /// Opaque validity token for the file's persisted slices, returned
    /// verbatim. `None` when the file carried no such token.
    pub slice_fingerprint: Option<String>,
}
