/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;

use anyhow::{Result, anyhow};
use sc_mesh_formats::filetypes::SourceMeshFileType;

pub fn fix(
    _source: OsString,
    _source_format: Option<SourceMeshFileType>,
    _target: OsString,
    _target_format: Option<SourceMeshFileType>,
) -> Result<()> {
    Err(anyhow!("Not yet implemented"))
}
