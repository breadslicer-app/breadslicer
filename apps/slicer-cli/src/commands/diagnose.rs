/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::fs::File;
use std::{ffi::OsString};
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_formats::filetypes::{SourceMeshFileType, infer_mesh_file_format_from_extension};
use sc_mesh_formats::stl::{read_stl, read_stla, read_stlb};

use crate::filesystem::validate_sourcefile_path;

pub fn diagnose(source: OsString, source_format: Option<SourceMeshFileType>) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let source_format = match source_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&source_path)?,
    };

    let mut source_file = File::open(&source_path)?;

    let mesh = match source_format {
        SourceMeshFileType::Stl => read_stl(&mut source_file)?,
        SourceMeshFileType::StlAscii => read_stla(&mut source_file)?,
        SourceMeshFileType::StlBin => read_stlb(&mut source_file)?,
        _ => todo!("Not implemented yet!"),
    };
    drop(source_file);

    let mut detected_problems = false;
    
    // TODO: List specific faces with problematic normal vectors
    if !mesh.are_normal_vectors_consistent() {
        detected_problems = true;
        eprintln!("Error: diagnosed mesh has faces with inconsistent normal vectors.");
    }

    // TODO: Show specific problems that make the mesh non-manifold
    if !mesh.is_manifold() {
        detected_problems = true;
        eprintln!("Error: diagnosed mesh is not a 'manifold' (it might have 'holes', or be an 'open' shape).");
    }

    if detected_problems {
        return Err(anyhow!("Found problems in diagnosed mesh"));
    }

    Ok(())
}
