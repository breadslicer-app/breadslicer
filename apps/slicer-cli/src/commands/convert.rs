/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::fs::copy;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_formats::filetypes::{
    MeshFileType, WriteFormat, infer_mesh_file_format_from_extension,
};
use sc_mesh_formats::load_mesh_from_path;

use crate::commands::mesh_loader::display_warnings;
use crate::commands::mesh_writer::write_scene;
use crate::filesystem::{validate_sourcefile_path, validate_targetfile_path};

pub(crate) fn convert(
    source: OsString,
    source_format: Option<MeshFileType>,
    target: OsString,
    target_format: Option<MeshFileType>,
) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let target_path = validate_targetfile_path(PathBuf::from(target))?;

    if source_path == target_path {
        return Err(anyhow!(
            "Source file path and target file path are the same"
        ));
    }

    let source_format = match source_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&source_path)?,
    };
    let target_format = match target_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&target_path)?,
    };

    if source_format == target_format {
        eprintln!("Warning: copying file without performing any format conversion");

        return match copy(&source_path, &target_path) {
            Ok(_) => Ok(()),
            Err(err) => Err(err.into()),
        };
    }

    let target_write_format = WriteFormat::try_from(target_format)?;

    let loaded = load_mesh_from_path(
        &source_path,
        Some(source_format),
        Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES),
    )?;
    display_warnings(&loaded.warnings, &mut std::io::stderr());

    write_scene(&loaded.graph, &target_path, target_write_format)
}
