/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_core::errors::MeshFix;
use sc_mesh_core::indexed_mesh::FixFacesOptions;
use sc_mesh_core::scene::SceneObjectContent;
use sc_mesh_formats::filetypes::{
    MeshFileType, WriteFormat, infer_mesh_file_format_from_extension,
};
use sc_mesh_formats::load_mesh_from_path;

use crate::commands::diagnose::show_vertex_errors;
use crate::commands::mesh_loader::display_warnings;
use crate::commands::mesh_writer::write_scene;
use crate::filesystem::{
    MAX_INPUT_FILE_SIZE_BYTES, validate_sourcefile_path, validate_targetfile_path,
};

/// Repairs the user requested when running `fix`. Each flag enables an
/// independent pass over the mesh; passes that find nothing to repair
/// are no-ops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FixOptions {
    pub zero_length_edges: bool,
    pub flipped_faces: bool,
    pub face_normals: bool,
}

pub(crate) fn fix(
    source: OsString,
    source_format: Option<MeshFileType>,
    target: OsString,
    target_format: Option<MeshFileType>,
    options: FixOptions,
) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let target_path = validate_targetfile_path(PathBuf::from(target))?;

    if source_path == target_path {
        return Err(anyhow!(
            "Source file path and target file path are the same"
        ));
    }

    let target_format = WriteFormat::try_from(match target_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&target_path)?,
    })?;

    let loaded = load_mesh_from_path(&source_path, source_format, Some(MAX_INPUT_FILE_SIZE_BYTES))?;
    display_warnings(&loaded.warnings);
    let mut scene = loaded.scene;

    let mut all_fixes: Vec<MeshFix> = Vec::new();

    for mesh in &mut scene.resources {
        all_fixes.extend(fix_mesh(mesh, options)?);
    }

    for obj in &mut scene.objects {
        let SceneObjectContent::Mesh(ref mut mesh) = obj.content else {
            continue;
        };
        all_fixes.extend(fix_mesh(mesh, options)?);
    }

    if all_fixes.is_empty() {
        println!("No fixes needed.");
    } else {
        show_applied_fixes(&all_fixes);
    }

    write_scene(&scene, &target_path, target_format)
}

fn fix_mesh(mesh: &mut sc_mesh_core::IndexedMesh, options: FixOptions) -> Result<Vec<MeshFix>> {
    if let Err(errors) = mesh.validate_vertices() {
        show_vertex_errors(&errors);
        return Err(anyhow!("Unable to fix mesh, there are invalid vertices."));
    }

    let mut fixes: Vec<MeshFix> = Vec::new();

    // Try to obtain the witness directly. If the mesh's faces
    // already validate, no further face-level repair is necessary
    // and we move straight on to normal/winding fixes, paying for
    // `validate_faces` exactly once. Otherwise, fall back to
    // repairing the zero-length-edge cases (when enabled) and then
    // re-mint the witness from the post-removal mesh; the helper
    // skips a second `validate_faces` whenever the only input
    // errors were `ZeroLengthEdge`.
    let mut validated = match mesh.validated_faces_mut() {
        Ok(v) => v,
        Err(face_errors) => {
            if !options.zero_length_edges {
                return Err(anyhow!(
                    "Unable to fix mesh. Face errors correction is not implemented (yet)."
                ));
            }
            let (edge_fixes, witness) = mesh.fix_zero_length_edges_into_validated(&face_errors);
            // TODO: Fix other cases
            if edge_fixes.len() < face_errors.len() {
                return Err(anyhow!(
                    "Unable to fix mesh. Face errors correction is not implemented (yet)."
                ));
            }
            fixes.extend(edge_fixes);
            witness.map_err(|_| {
                anyhow!("Unable to fix mesh. Face errors correction is not implemented (yet).")
            })?
        }
    };

    let normal_vector_errors = validated
        .validate_normal_vectors()
        .err()
        .unwrap_or_default();
    let manifold_errors = validated.validate_manifold().err().unwrap_or_default();

    if !normal_vector_errors.is_empty() || !manifold_errors.is_empty() {
        fixes.extend(validated.fix_faces_orientation(
            &manifold_errors,
            &normal_vector_errors,
            FixFacesOptions {
                flipped: options.flipped_faces,
                normals: options.face_normals,
            },
        )?);
    }

    Ok(fixes)
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
                    old_normal.coords(),
                    old_normal.norm(),
                    new_normal.coords(),
                    new_normal.norm()
                );
            }
        }
    }
}
