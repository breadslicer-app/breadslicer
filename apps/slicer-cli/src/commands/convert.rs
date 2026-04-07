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
use sc_mesh_formats::obj::write_obj;
use sc_mesh_formats::stl::{write_stla, write_stlb};

use crate::commands::mesh_loader::{display_warnings, load_mesh};
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

    let mut warnings = vec![];
    let scene = load_mesh(&source_path, Some(source_format), &mut warnings)?;

    display_warnings(&warnings);

    let mut output_file = File::create(&target_path)?;
    match target_format {
        SourceMeshFileType::Obj => {
            write_obj(&scene, &mut output_file)?;
        }
        SourceMeshFileType::Stl | SourceMeshFileType::StlAscii | SourceMeshFileType::StlBin => {
            let mesh_count = scene.objects.len();
            let merged;
            let mesh = match mesh_count {
                0 => return Err(anyhow!("Cannot write an empty scene to an STL file")),
                1 => match &scene.objects[0].content {
                    sc_mesh_core::scene::SceneObjectContent::Mesh(m) => m,
                    sc_mesh_core::scene::SceneObjectContent::Resource(idx) => scene
                        .resources
                        .get(*idx)
                        .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
                },
                _ => {
                    eprintln!(
                        "WARNING: Merging {} objects into a single mesh for STL output; \
                         object boundaries and names will be lost.",
                        mesh_count
                    );
                    merged = scene
                        .merge_meshes()?
                        .ok_or_else(|| anyhow!("Cannot write an empty scene to an STL file"))?;
                    &merged
                }
            };

            match target_format {
                SourceMeshFileType::Stl | SourceMeshFileType::StlBin => {
                    write_stlb(mesh, &mut output_file)?;
                }
                SourceMeshFileType::StlAscii => {
                    write_stla(mesh, &mut output_file)?;
                }
                _ => unreachable!(),
            }
        }
        _ => todo!("Not implemented yet!"),
    }

    Ok(())
}
