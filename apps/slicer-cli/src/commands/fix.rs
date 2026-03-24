/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::fs::File;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_formats::filetypes::{SourceMeshFileType, infer_mesh_file_format_from_extension};
use sc_mesh_formats::stl::{read_stl, read_stla, read_stlb, write_stla, write_stlb};

use crate::filesystem::{validate_sourcefile_path, validate_targetfile_path};

pub fn fix(
    source: OsString,
    source_format: Option<SourceMeshFileType>,
    target: OsString,
    target_format: Option<SourceMeshFileType>,
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

    let mut source_file = File::open(&source_path)?;

    let mut mesh = match source_format {
        SourceMeshFileType::Stl => read_stl(&mut source_file)?,
        SourceMeshFileType::StlAscii => read_stla(&mut source_file)?,
        SourceMeshFileType::StlBin => read_stlb(&mut source_file)?,
        _ => todo!("Not implemented yet!"),
    };
    drop(source_file);

    // TODO: Check & fix "face" problems too!

    let normal_vector_errors = match mesh.validate_normal_vectors() {
        Err(v) => v,
        _ => vec![]
    };
    let manifold_errors = match mesh.validate_manifold() {
        Err(v) => v,
        _ => vec![]
    };

    // TODO: List applied changes
    mesh.fix_faces_orientation(&manifold_errors, &normal_vector_errors)?;

    match target_format {
        SourceMeshFileType::Stl => {
            let mut output_file = File::create(&target_path)?;
            write_stlb(&mesh, &mut output_file)?;
        }
        SourceMeshFileType::StlAscii => {
            let mut output_file = File::create(&target_path)?;
            write_stla(&mesh, &mut output_file)?;
        }
        SourceMeshFileType::StlBin => {
            let mut output_file = File::create(&target_path)?;
            write_stlb(&mesh, &mut output_file)?;
        }
        _ => todo!("Not implemented yet!"),
    }

    Ok(())
}
