/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::fs::{File, copy};
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_formats::filetypes::{SourceMeshFileType, infer_mesh_file_format_from_extension};
use sc_mesh_formats::stl::{read_stl, read_stla, read_stlb, write_stla, write_stlb};

use crate::filesystem::{validate_sourcefile_path, validate_targetfile_path};

pub(crate) fn convert(
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

    if source_format == target_format {
        eprintln!("Warning: copying file without performing any format conversion");

        return match copy(&source_path, &target_path) {
            Ok(_) => Ok(()),
            Err(err) => Err(err.into()),
        };
    }

    let mut source_file = File::open(&source_path)?;

    let source_mesh = match source_format {
        SourceMeshFileType::Stl => read_stl(&mut source_file)?,
        SourceMeshFileType::StlAscii => read_stla(&mut source_file)?,
        SourceMeshFileType::StlBin => read_stlb(&mut source_file)?,
        _ => todo!("Not implemented yet!"),
    };
    drop(source_file);

    match target_format {
        SourceMeshFileType::Stl => {
            let mut output_file = File::create(&target_path)?;
            write_stlb(&source_mesh, &mut output_file)?;
        }
        SourceMeshFileType::StlAscii => {
            let mut output_file = File::create(&target_path)?;
            write_stla(&source_mesh, &mut output_file)?;
        }
        SourceMeshFileType::StlBin => {
            let mut output_file = File::create(&target_path)?;
            write_stlb(&source_mesh, &mut output_file)?;
        }
        _ => todo!("Not implemented yet!"),
    }

    Ok(())
}
