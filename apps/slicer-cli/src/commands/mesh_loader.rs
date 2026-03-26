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
use sc_mesh_formats::messages::LoadMeshWarning;
use sc_mesh_formats::obj::read_obj;
use sc_mesh_formats::stl::{read_stl, read_stla, read_stlb};

pub(crate) fn load_mesh(
    source_path: &Path,
    source_format: Option<SourceMeshFileType>,
    warnings: &mut Vec<LoadMeshWarning>,
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
        SourceMeshFileType::Obj => read_obj(&mut source_file, warnings),
        _ => todo!("Not implemented yet!"),
    }
}

pub(crate) fn display_warnings(warnings: &Vec<LoadMeshWarning>) {
    for warning in warnings {
        match warning {
            LoadMeshWarning::Obj(obj_warning) => match obj_warning {
                sc_mesh_formats::messages::LoadObjWarning::NoVertices => {
                    eprintln!("WARNING: Loaded OBJ file has no vertices");
                },
                sc_mesh_formats::messages::LoadObjWarning::NoFaces => {
                    eprintln!("WARNING: Loaded OBJ file has no faces");
                },
                sc_mesh_formats::messages::LoadObjWarning::SkippedTextureCoordinates { line } => {
                    eprintln!("WARNING (line {}): skipped texture coordinates", line);
                },
                sc_mesh_formats::messages::LoadObjWarning::SkippedVertexNormal { line } => {
                    eprintln!("WARNING (line {}): skipped vertex normal", line);
                },
                sc_mesh_formats::messages::LoadObjWarning::SkippedGroup { line, name } => {
                    eprintln!("WARNING (line {}): Skipped group '{}'", line, name);
                },
                sc_mesh_formats::messages::LoadObjWarning::SkippedSmoothingGroup { line } => {
                    eprintln!("WARNING (line {}): Skipped smoothing group", line);
                },
                sc_mesh_formats::messages::LoadObjWarning::SkippedMaterialReference { line, material } => {
                    eprintln!("WARNING (line {}): Skipped material reference '{}'", line, material);
                },
                sc_mesh_formats::messages::LoadObjWarning::SkippedMaterialLibrary { line, path } => {
                    eprintln!("WARNING (line {}): Skipped material library '{}'", line, path);
                },
                sc_mesh_formats::messages::LoadObjWarning::SkippedComment { line } => {
                    eprintln!("WARNING (line {}): Skipped comment", line);
                },
            },
        }
    }
}
