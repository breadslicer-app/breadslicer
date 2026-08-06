/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_core::indexed_mesh::{
    FaceDiagnosis, MeshDiagnosis, MeshEdgeManifoldError, MeshFaceError, MeshNormalVectorError,
    MeshVertexError,
};
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_formats::filetypes::MeshFileType;
use sc_mesh_formats::load_mesh_from_path;

use crate::commands::mesh_loader::display_warnings;
use crate::filesystem::validate_sourcefile_path;

pub(crate) fn diagnose(
    source: OsString,
    source_format: Option<MeshFileType>,
    out: &mut dyn Write,
) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let loaded = load_mesh_from_path(
        &source_path,
        source_format,
        Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES),
    )?;
    display_warnings(&loaded.warnings, out);

    let mut detected_problems = false;
    for (_target, diagnosis) in loaded.graph.diagnose_meshes() {
        show_diagnosis(&diagnosis, out);
        detected_problems |= diagnosis.has_problems();
    }

    if detected_problems {
        return Err(anyhow!("Found problems in diagnosed mesh"));
    }

    Ok(())
}

/// Renders one mesh's diagnosis with the historical block layout:
/// vertex errors, then either the face errors (whose gate failure
/// left the witness-gated findings absent) or the normal-vector and
/// edge-manifold blocks. Blocks without findings are omitted.
fn show_diagnosis(diagnosis: &MeshDiagnosis, out: &mut dyn Write) {
    if !diagnosis.vertex_errors.is_empty() {
        show_vertex_errors(&diagnosis.vertex_errors, out);
    }
    match &diagnosis.faces {
        FaceDiagnosis::Invalid(face_errors) => show_face_errors(face_errors, out),
        FaceDiagnosis::Valid {
            normal_vector_errors,
            edge_manifold_errors,
        } => {
            if !normal_vector_errors.is_empty() {
                show_normal_vector_errors(normal_vector_errors, out);
            }
            if !edge_manifold_errors.is_empty() {
                show_manifold_errors(edge_manifold_errors, out);
            }
        }
    }
}

pub(crate) fn show_vertex_errors(vertex_errors: &[MeshVertexError], out: &mut dyn Write) {
    let _ = writeln!(out, "Vertex errors ({}):", vertex_errors.len());
    for vertex_error in vertex_errors {
        let _ = writeln!(out, "\t- {vertex_error}");
    }
}

pub(crate) fn show_face_errors(face_errors: &[MeshFaceError], out: &mut dyn Write) {
    let _ = writeln!(out, "Face errors ({}):", face_errors.len());
    for face_error in face_errors {
        let _ = writeln!(out, "\t- {face_error}");
    }
}

pub(crate) fn show_normal_vector_errors(
    vector_errors: &[MeshNormalVectorError],
    out: &mut dyn Write,
) {
    let _ = writeln!(out, "Normal vector errors ({}):", vector_errors.len());
    for vector_error in vector_errors {
        let _ = writeln!(out, "\t- {vector_error}");
    }
}

pub(crate) fn show_manifold_errors(manifold_errors: &[MeshEdgeManifoldError], out: &mut dyn Write) {
    let _ = writeln!(out, "Manifold errors ({}):", manifold_errors.len());
    for manifold_error in manifold_errors {
        let _ = writeln!(out, "\t- {manifold_error}");
    }
}
