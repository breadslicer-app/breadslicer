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
use sc_mesh_core::{MeshFaceError, MeshManifoldError, MeshNormalVectorError, MeshVertexError};
use sc_mesh_formats::filetypes::{SourceMeshFileType, infer_mesh_file_format_from_extension};
use sc_mesh_formats::stl::{read_stl, read_stla, read_stlb};

use crate::filesystem::validate_sourcefile_path;

pub fn diagnose(source: OsString, source_format: Option<SourceMeshFileType>) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let source_format = match source_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&source_path)?,
    };

    let mut source_file = File::open(&source_path)?;

    let mesh = match source_format {
        SourceMeshFileType::Stl => read_stl(&mut source_file)?,
        SourceMeshFileType::StlAscii => read_stla(&mut source_file)?,
        SourceMeshFileType::StlBin => read_stlb(&mut source_file)?,
        _ => todo!("Not implemented yet!"),
    };
    drop(source_file);

    let mut detected_problems = false;
    let mut critical_problems = false;

    // TODO: warn on "subnormal" vertex coordinates?
    if let Err(vertex_errors) = mesh.validate_vertices() {
        detected_problems = true;
        critical_problems = true;

        eprintln!("Vertex errors:");
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

    if let Err(face_errors) = mesh.validate_faces() {
        detected_problems = true;
        critical_problems = true;

        eprintln!("Face errors:");
        for face_error in face_errors {
            match face_error {
                MeshFaceError::CollapsedEdge {
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

    // If vertices or triangles are broken, it makes no sense to keep checking.
    if !critical_problems {
        if let Err(vector_errors) = mesh.validate_normal_vectors() {
            detected_problems = true;

            eprintln!("Normal vectors' errors:");
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

        if let Err(manifold_errors) = mesh.validate_manifold() {
            detected_problems = true;

            eprintln!("Manifold errors:");
            for manifold_error in manifold_errors {
                match manifold_error {
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
    }

    if detected_problems {
        return Err(anyhow!("Found problems in diagnosed mesh"));
    }

    Ok(())
}
