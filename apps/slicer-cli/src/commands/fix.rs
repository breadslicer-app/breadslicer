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
use sc_mesh_core::errors::MeshFix;
use sc_mesh_formats::filetypes::{SourceMeshFileType, infer_mesh_file_format_from_extension};
use sc_mesh_formats::stl::{write_stla, write_stlb};

use crate::commands::diagnose::show_vertex_errors;
use crate::commands::mesh_loader::load_mesh;
use crate::filesystem::{validate_sourcefile_path, validate_targetfile_path};

pub(crate) fn fix(
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

    let target_format = match target_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&target_path)?,
    };

    let mut mesh = load_mesh(&source_path, source_format)?;
    let mut all_fixes: Vec<MeshFix> = Vec::new();

    if let Err(errors) = mesh.validate_vertices() {
        show_vertex_errors(&errors);
        return Err(anyhow!("Unable to fix mesh, there are invalid vertices."));
    }

    if let Err(errors) = mesh.validate_faces() {
        let fixes = mesh.fix_zero_length_edges(&errors);
        // TODO: Fix other cases

        if fixes.len() < errors.len() {
            return Err(anyhow!(
                "Unable to fix mesh. Face errors correction is not implemented (yet)."
            ));
        }
        all_fixes.extend(fixes);
    }

    let normal_vector_errors = match mesh.validate_normal_vectors() {
        Err(v) => v,
        _ => vec![],
    };
    let manifold_errors = match mesh.validate_manifold() {
        Err(v) => v,
        _ => vec![],
    };

    if !normal_vector_errors.is_empty() || !manifold_errors.is_empty() {
        let fixes = mesh.fix_faces_orientation(&manifold_errors, &normal_vector_errors)?;
        all_fixes.extend(fixes);
    }

    if all_fixes.is_empty() {
        println!("No fixes needed.");
    } else {
        show_applied_fixes(&all_fixes);
    }

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

fn show_applied_fixes(fixes: &[MeshFix]) {
    eprintln!("Applied fixes:");
    for fix in fixes {
        match fix {
            MeshFix::RemovedZeroLengthEdgeFace { face_idx } => {
                println!(
                    "\t- Removed face with ID '{}' (had zero-length edge)",
                    face_idx
                );
            }
            MeshFix::FlippedFaceOrientation { face_idx } => {
                println!("\t- Flipped orientation of face with ID '{}'", face_idx);
            }
            MeshFix::RecomputedNormal { face_idx } => {
                println!(
                    "\t- Recomputed normal vector for face with ID '{}'",
                    face_idx
                );
            }
        }
    }
}
