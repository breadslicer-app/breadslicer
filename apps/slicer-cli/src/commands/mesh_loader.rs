/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::fs::File;
use std::path::Path;

use anyhow::Result;
use sc_mesh_core::IndexedMesh;
use sc_mesh_formats::filetypes::{SourceMeshFileType, infer_mesh_file_format_from_extension};
use sc_mesh_formats::stl::{read_stl, read_stla, read_stlb};

pub(crate) fn load_mesh(
    source_path: &Path,
    source_format: Option<SourceMeshFileType>,
) -> Result<IndexedMesh> {
    let source_format = match source_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(source_path)?,
    };

    let mut source_file = File::open(source_path)?;

    match source_format {
        SourceMeshFileType::Stl => read_stl(&mut source_file),
        SourceMeshFileType::StlAscii => read_stla(&mut source_file),
        SourceMeshFileType::StlBin => read_stlb(&mut source_file),
        _ => todo!("Not implemented yet!"),
    }
}
