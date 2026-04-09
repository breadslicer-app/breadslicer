/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_core::errors::MeshFix;
use sc_mesh_core::scene::SceneObjectContent;
use sc_mesh_formats::filetypes::{MeshFileType, infer_mesh_file_format_from_extension};

use crate::commands::diagnose::show_vertex_errors;
use crate::commands::mesh_loader::{display_warnings, load_mesh};
use crate::commands::mesh_writer::write_scene;
use crate::filesystem::{validate_sourcefile_path, validate_targetfile_path};

pub(crate) fn fix(
    source: OsString,
    source_format: Option<MeshFileType>,
    target: OsString,
    target_format: Option<MeshFileType>,
    fix_zero_length_edges: bool,
    fix_flipped_faces: bool,
    fix_face_normals: bool,
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

    let mut warnings = vec![];
    let mut scene = load_mesh(&source_path, source_format, &mut warnings)?;

    display_warnings(&warnings);

    let mut all_fixes: Vec<MeshFix> = Vec::new();

    for mesh in &mut scene.resources {
        fix_mesh(
            mesh,
            &mut all_fixes,
            fix_zero_length_edges,
            fix_flipped_faces,
            fix_face_normals,
        )?;
    }

    for obj in &mut scene.objects {
        let SceneObjectContent::Mesh(ref mut mesh) = obj.content else {
            continue;
        };
        fix_mesh(
            mesh,
            &mut all_fixes,
            fix_zero_length_edges,
            fix_flipped_faces,
            fix_face_normals,
        )?;
    }

    if all_fixes.is_empty() {
        println!("No fixes needed.");
    } else {
        show_applied_fixes(&all_fixes);
    }

    write_scene(&scene, &target_path, target_format)
}

fn fix_mesh(
    mesh: &mut sc_mesh_core::IndexedMesh,
    all_fixes: &mut Vec<MeshFix>,
    fix_zero_length_edges: bool,
    fix_flipped_faces: bool,
    fix_face_normals: bool,
) -> Result<()> {
    if let Err(errors) = mesh.validate_vertices() {
        show_vertex_errors(&errors);
        return Err(anyhow!("Unable to fix mesh, there are invalid vertices."));
    }

    if fix_zero_length_edges && let Err(errors) = mesh.validate_faces() {
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
        let fixes = mesh.fix_faces_orientation(
            &manifold_errors,
            &normal_vector_errors,
            fix_flipped_faces,
            fix_face_normals,
        )?;
        all_fixes.extend(fixes);
    }

    Ok(())
}

fn show_applied_fixes(fixes: &[MeshFix]) {
    println!("Applied fixes ({}):", fixes.len());
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
            MeshFix::RecomputedNormal {
                face_idx,
                old_normal,
                new_normal,
            } => {
                println!(
                    "\t- Recomputed normal vector for face with ID '{}', from {:?} (with norm {}) to {:?} (with norm {})",
                    face_idx,
                    old_normal.0,
                    old_normal.norm(),
                    new_normal.0,
                    new_normal.norm()
                );
            }
        }
    }
}
