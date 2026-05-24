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
    MeshFaceError, MeshManifoldError, MeshNormalVectorError, MeshVertexError,
};
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_formats::filetypes::MeshFileType;
use sc_mesh_formats::load_mesh_from_path;

use crate::commands::mesh_loader::display_warnings;
use crate::filesystem::{MAX_INPUT_FILE_SIZE_BYTES, validate_sourcefile_path};

pub(crate) fn diagnose(
    source: OsString,
    source_format: Option<MeshFileType>,
    out: &mut dyn Write,
) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let loaded = load_mesh_from_path(&source_path, source_format, Some(MAX_INPUT_FILE_SIZE_BYTES))?;
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
    // by `validate_normal_vectors` and `validate_manifold`. If it
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

    if let Err(manifold_errors) = validated.validate_manifold() {
        detected_problems = true;
        show_manifold_errors(&manifold_errors, out);
    }

    detected_problems
}

pub(crate) fn show_vertex_errors(vertex_errors: &[MeshVertexError], out: &mut dyn Write) {
    let _ = writeln!(out, "Vertex errors ({}):", vertex_errors.len());
    for vertex_error in vertex_errors {
        match vertex_error {
            MeshVertexError::InfiniteCoordinate { vertex_idx, coord } => {
                let _ = writeln!(
                    out,
                    "\t- Vertex with ID {vertex_idx} has an infinite value for its coordinate '{coord}'"
                );
            }
            MeshVertexError::NaNCoordinate { vertex_idx, coord } => {
                let _ = writeln!(
                    out,
                    "\t- Vertex with ID {vertex_idx} has an NaN value for its coordinate '{coord}'"
                );
            }
        }
    }
}

pub(crate) fn show_face_errors(face_errors: &[MeshFaceError], out: &mut dyn Write) {
    let _ = writeln!(out, "Face errors ({}):", face_errors.len());
    for face_error in face_errors {
        match face_error {
            MeshFaceError::ZeroLengthEdge {
                face_idx,
                vertex_pos_a,
                vertex_pos_b,
                vertex_idx,
            } => {
                let _ = writeln!(
                    out,
                    "\t- Face with ID {face_idx} has 2 vertices ({vertex_pos_a}, {vertex_pos_b}) with same value (vertex id = {vertex_idx})"
                );
            }
            MeshFaceError::OutOfRangeVertexIndex {
                face_idx,
                vertex_pos,
                vertex_idx,
            } => {
                let _ = writeln!(
                    out,
                    "\t- Face with ID {face_idx} contains a vertex ({vertex_pos}) with an out of range index {vertex_idx}"
                );
            }
        }
    }
}

pub(crate) fn show_normal_vector_errors(
    vector_errors: &[MeshNormalVectorError],
    out: &mut dyn Write,
) {
    let _ = writeln!(out, "Normal vectors' errors ({}):", vector_errors.len());
    for vector_error in vector_errors {
        match vector_error {
            MeshNormalVectorError::ZeroAreaTriangle { face_idx } => {
                let _ = writeln!(out, "\t- Face with ID {face_idx} has area equal to 0");
            }
            MeshNormalVectorError::NonUnitNorm { face_idx, norm } => {
                let _ = writeln!(
                    out,
                    "\t- Face with ID {face_idx} has normal vector with non-unit norm: {norm}"
                );
            }
            MeshNormalVectorError::ExpectationMismatch { face_idx } => {
                let _ = writeln!(
                    out,
                    "\t- Face with ID {face_idx} has a normal vector that does not match our calculations"
                );
            }
        }
    }
}

pub(crate) fn show_manifold_errors(manifold_errors: &[MeshManifoldError], out: &mut dyn Write) {
    let _ = writeln!(out, "Manifold errors:");
    for manifold_error in manifold_errors {
        match manifold_error {
            MeshManifoldError::EmptyMesh => {
                let _ = writeln!(out, "\t- Loaded mesh is empty, it has no faces");
            }
            MeshManifoldError::BoundaryEdge { edge, face_idx } => {
                let _ = writeln!(
                    out,
                    "\t- Face with ID {face_idx} has a 'boundary edge' ({}, {}), so the mesh does not form a manifold",
                    edge.0, edge.1
                );
            }
            MeshManifoldError::NonManifoldEdge { edge, face_indices } => {
                let _ = writeln!(
                    out,
                    "\t- The edge ({}, {}) is shared by more than {} faces (it should be 2)",
                    edge.0,
                    edge.1,
                    face_indices.len()
                );
            }
            MeshManifoldError::InconsistentWindingOrder { edge, face_indices } => {
                let _ = writeln!(
                    out,
                    "\t- The edge ({}, {}) belongs to 2 faces ({} and {}) with inconsistent normal vectors",
                    edge.0, edge.1, face_indices[0], face_indices[1]
                );
            }
        }
    }
}
