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
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::{
    MeshEdgeManifoldError, MeshFaceError, MeshNormalVectorError, MeshVertexError,
};
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::node::{LeafPayload, NodeId, NodeKind};
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

    for (_id, mesh) in loaded.graph.mesh_resources_iter() {
        detected_problems |= diagnose_mesh(mesh, out);
    }

    for_each_inline_leaf_mesh(&loaded.graph, |mesh| {
        detected_problems |= diagnose_mesh(mesh, out);
    });

    if detected_problems {
        return Err(anyhow!("Found problems in diagnosed mesh"));
    }

    Ok(())
}

/// Walk every reachable inline-mesh leaf under `graph.roots()` in
/// pre-order and invoke `f` with the leaf's mesh.
/// Resource-backed leaves are skipped: their meshes live in the
/// resource arena and are iterated through `mesh_resources_iter`.
fn for_each_inline_leaf_mesh<F: FnMut(&IndexedMesh)>(graph: &Scene, mut f: F) {
    let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        match &node.kind {
            NodeKind::Leaf(LeafPayload::Mesh(mesh)) => f(mesh),
            NodeKind::Leaf(LeafPayload::MeshResource(_)) => {}
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Instance { .. } => {}
        }
    }
}

/// Returns `true` if any problems were detected.
fn diagnose_mesh(mesh: &IndexedMesh, out: &mut dyn Write) -> bool {
    let mut detected_problems = false;

    if let Err(vertex_errors) = mesh.validate_vertices() {
        detected_problems = true;
        show_vertex_errors(&vertex_errors, out);
    }

    // `validated_faces` is the gate that produces the witness needed
    // by `validate_normal_vectors` and `validate_edge_manifold`. If it
    // fails, vertex indices are out of range or edges are
    // collapsed; neither downstream check is meaningful in that
    // state, so the function returns early after surfacing the
    // face errors.
    let validated = match mesh.validated_faces() {
        Ok(v) => v,
        Err(face_errors) => {
            show_face_errors(&face_errors, out);
            return true;
        }
    };

    if let Err(vector_errors) = validated.validate_normal_vectors() {
        detected_problems = true;
        show_normal_vector_errors(&vector_errors, out);
    }

    if let Err(manifold_errors) = validated.validate_edge_manifold() {
        detected_problems = true;
        show_manifold_errors(&manifold_errors, out);
    }

    detected_problems
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
