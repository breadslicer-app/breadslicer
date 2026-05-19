/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Result, anyhow};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::{
    MeshFaceError, MeshManifoldError, MeshNormalVectorError, MeshVertexError,
};
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene_node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_formats::filetypes::MeshFileType;
use sc_mesh_formats::load_mesh_from_path;

use crate::commands::mesh_loader::display_warnings;
use crate::filesystem::{MAX_INPUT_FILE_SIZE_BYTES, validate_sourcefile_path};

pub(crate) fn diagnose(source: OsString, source_format: Option<MeshFileType>) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let loaded = load_mesh_from_path(&source_path, source_format, Some(MAX_INPUT_FILE_SIZE_BYTES))?;
    display_warnings(&loaded.warnings);

    let mut detected_problems = false;

    for (_id, mesh) in loaded.graph.mesh_resources_iter() {
        detected_problems |= diagnose_mesh(mesh);
    }

    for_each_inline_leaf_mesh(&loaded.graph, |mesh| {
        detected_problems |= diagnose_mesh(mesh);
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
fn diagnose_mesh(mesh: &IndexedMesh) -> bool {
    let mut detected_problems = false;

    // TODO: warn on "subnormal" vertex coordinates?
    if let Err(vertex_errors) = mesh.validate_vertices() {
        detected_problems = true;
        show_vertex_errors(&vertex_errors);
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
            show_face_errors(&face_errors);
            return true;
        }
    };

    if let Err(vector_errors) = validated.validate_normal_vectors() {
        detected_problems = true;
        show_normal_vector_errors(&vector_errors);
    }

    if let Err(manifold_errors) = validated.validate_manifold() {
        detected_problems = true;
        show_manifold_errors(&manifold_errors);
    }

    detected_problems
}

pub(crate) fn show_vertex_errors(vertex_errors: &[MeshVertexError]) {
    eprintln!("Vertex errors ({}):", vertex_errors.len());
    for vertex_error in vertex_errors {
        match vertex_error {
            MeshVertexError::InfiniteCoordinate { vertex_idx, coord } => {
                eprintln!(
                    "\t- Vertex with ID {} has an infinite value for its coordinate '{}'",
                    vertex_idx, coord
                );
            }
            MeshVertexError::NaNCoordinate { vertex_idx, coord } => {
                eprintln!(
                    "\t- Vertex with ID {} has an NaN value for its coordinate '{}'",
                    vertex_idx, coord
                );
            }
        }
    }
}

pub(crate) fn show_face_errors(face_errors: &[MeshFaceError]) {
    eprintln!("Face errors ({}):", face_errors.len());
    for face_error in face_errors {
        match face_error {
            MeshFaceError::ZeroLengthEdge {
                face_idx,
                vertex_pos_a,
                vertex_pos_b,
                vertex_idx,
            } => {
                eprintln!(
                    "\t- Face with ID {} has 2 vertices ({}, {}) with same value (vertex id = {})",
                    face_idx, vertex_pos_a, vertex_pos_b, vertex_idx
                );
            }
            MeshFaceError::OutOfRangeVertexIndex {
                face_idx,
                vertex_pos,
                vertex_idx,
            } => {
                eprintln!(
                    "\t- Face with ID {} contains a vertex ({}) with an out of range index {}",
                    face_idx, vertex_pos, vertex_idx
                );
            }
        }
    }
}

fn show_normal_vector_errors(vector_errors: &[MeshNormalVectorError]) {
    eprintln!("Normal vectors' errors ({}):", vector_errors.len());
    for vector_error in vector_errors {
        match vector_error {
            MeshNormalVectorError::ZeroAreaTriangle { face_idx } => {
                eprintln!("\t- Face with ID {} has area equal to 0", face_idx);
            }
            MeshNormalVectorError::NonUnitNorm { face_idx, norm } => {
                eprintln!(
                    "\t- Face with ID {} has normal vector with non-unit norm: {}",
                    face_idx, norm
                );
            }
            MeshNormalVectorError::ExpectationMismatch { face_idx } => {
                eprintln!(
                    "\t- Face with ID {} has a normal vector that does not match our calculations",
                    face_idx
                );
            }
        }
    }
}

fn show_manifold_errors(manifold_errors: &[MeshManifoldError]) {
    eprintln!("Manifold errors:");
    for manifold_error in manifold_errors {
        match manifold_error {
            MeshManifoldError::EmptyMesh => {
                eprintln!("\t- Loaded mesh is empty, it has no faces")
            }
            MeshManifoldError::BoundaryEdge { edge, face_idx } => {
                eprintln!(
                    "\t- Face with ID {} has a 'boundary edge' ({}, {}), so the mesh does not form a manifold",
                    face_idx, edge.0, edge.1
                );
            }
            MeshManifoldError::NonManifoldEdge { edge, face_indices } => {
                eprintln!(
                    "\t- The edge ({}, {}) is shared by more than {} faces (it should be 2)",
                    edge.0,
                    edge.1,
                    face_indices.len()
                )
            }
            MeshManifoldError::InconsistentWindingOrder { edge, face_indices } => {
                eprintln!(
                    "\t- The edge ({}, {}) belongs to 2 faces ({} and {}) with inconsistent normal vectors",
                    edge.0, edge.1, face_indices[0], face_indices[1]
                );
            }
        }
    }
}
