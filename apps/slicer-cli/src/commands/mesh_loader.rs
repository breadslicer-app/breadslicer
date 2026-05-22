/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::io::Write;

use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning};

pub(crate) fn display_warnings(warnings: &[LoadMeshWarning], out: &mut dyn Write) {
    for warning in warnings {
        let LoadMeshWarning::Obj(obj_warning) = warning;
        match obj_warning {
            LoadObjWarning::NoVertices => {
                let _ = writeln!(out, "WARNING: Loaded OBJ file has no vertices");
            }
            LoadObjWarning::NoFaces => {
                let _ = writeln!(out, "WARNING: Loaded OBJ file has no faces");
            }
            LoadObjWarning::SkippedTextureCoordinates { line } => {
                let _ = writeln!(out, "WARNING (line {line}): skipped texture coordinates");
            }
            LoadObjWarning::SkippedVertexNormal { line } => {
                let _ = writeln!(out, "WARNING (line {line}): skipped vertex normal");
            }
            LoadObjWarning::SkippedGroup { line, name } => {
                let _ = writeln!(out, "WARNING (line {line}): Skipped group '{name}'");
            }
            LoadObjWarning::SkippedSmoothingGroup { line } => {
                let _ = writeln!(out, "WARNING (line {line}): Skipped smoothing group");
            }
            LoadObjWarning::SkippedMaterialReference { line, material } => {
                let _ = writeln!(
                    out,
                    "WARNING (line {line}): Skipped material reference '{material}'"
                );
            }
            LoadObjWarning::SkippedMaterialLibrary { line, path } => {
                let _ = writeln!(
                    out,
                    "WARNING (line {line}): Skipped material library '{path}'"
                );
            }
            LoadObjWarning::SkippedComment { line } => {
                let _ = writeln!(out, "WARNING (line {line}): Skipped comment");
            }
            LoadObjWarning::SkippedUnknownDirective { line, directive } => {
                let _ = writeln!(
                    out,
                    "WARNING (line {line}): Skipped OBJ directive '{directive}'"
                );
            }
            LoadObjWarning::SharedVertices {
                object_name,
                shared_indices,
            } => match object_name {
                Some(name) => {
                    let _ = writeln!(
                        out,
                        "WARNING: Object '{name}' shares global vertex indices with previously \
                         loaded objects: {shared_indices:?}"
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "WARNING: An unnamed object shares global vertex indices with \
                         previously loaded objects: {shared_indices:?}"
                    );
                }
            },
        }
    }
}
