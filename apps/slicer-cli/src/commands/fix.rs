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
use sc_mesh_core::scene::SceneObjectContent;
use sc_mesh_formats::filetypes::{SourceMeshFileType, infer_mesh_file_format_from_extension};
use sc_mesh_formats::obj::write_obj;
use sc_mesh_formats::stl::{write_stla, write_stlb};

use crate::commands::diagnose::show_vertex_errors;
use crate::commands::mesh_loader::{display_warnings, load_mesh};
use crate::filesystem::{validate_sourcefile_path, validate_targetfile_path};

pub(crate) fn fix(
    source: OsString,
    source_format: Option<SourceMeshFileType>,
    target: OsString,
    target_format: Option<SourceMeshFileType>,
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
                    SceneObjectContent::Mesh(m) => m,
                    SceneObjectContent::Resource(idx) => scene
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
    eprintln!("Applied fixes ({}):", fixes.len());
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
