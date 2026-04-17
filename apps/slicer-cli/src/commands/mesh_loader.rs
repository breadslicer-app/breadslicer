/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use sc_mesh_formats::messages::LoadMeshWarning;

pub(crate) fn display_warnings(warnings: &[LoadMeshWarning]) {
    for warning in warnings {
        match warning {
            LoadMeshWarning::Obj(obj_warning) => match obj_warning {
                sc_mesh_formats::messages::LoadObjWarning::NoVertices => {
                    eprintln!("WARNING: Loaded OBJ file has no vertices");
                }
                sc_mesh_formats::messages::LoadObjWarning::NoFaces => {
                    eprintln!("WARNING: Loaded OBJ file has no faces");
                }
                sc_mesh_formats::messages::LoadObjWarning::SkippedTextureCoordinates { line } => {
                    eprintln!("WARNING (line {}): skipped texture coordinates", line);
                }
                sc_mesh_formats::messages::LoadObjWarning::SkippedVertexNormal { line } => {
                    eprintln!("WARNING (line {}): skipped vertex normal", line);
                }
                sc_mesh_formats::messages::LoadObjWarning::SkippedGroup { line, name } => {
                    eprintln!("WARNING (line {}): Skipped group '{}'", line, name);
                }
                sc_mesh_formats::messages::LoadObjWarning::SkippedSmoothingGroup { line } => {
                    eprintln!("WARNING (line {}): Skipped smoothing group", line);
                }
                sc_mesh_formats::messages::LoadObjWarning::SkippedMaterialReference {
                    line,
                    material,
                } => {
                    eprintln!(
                        "WARNING (line {}): Skipped material reference '{}'",
                        line, material
                    );
                }
                sc_mesh_formats::messages::LoadObjWarning::SkippedMaterialLibrary {
                    line,
                    path,
                } => {
                    eprintln!(
                        "WARNING (line {}): Skipped material library '{}'",
                        line, path
                    );
                }
                sc_mesh_formats::messages::LoadObjWarning::SkippedComment { line } => {
                    eprintln!("WARNING (line {}): Skipped comment", line);
                }
                sc_mesh_formats::messages::LoadObjWarning::SharedVertices {
                    object_name,
                    shared_indices,
                } => match object_name {
                    Some(name) => eprintln!(
                        "WARNING: Object '{}' shares global vertex indices with previously \
                         loaded objects: {:?}",
                        name, shared_indices
                    ),
                    None => eprintln!(
                        "WARNING: An unnamed object shares global vertex indices with \
                         previously loaded objects: {:?}",
                        shared_indices
                    ),
                },
            },
        }
    }
}
