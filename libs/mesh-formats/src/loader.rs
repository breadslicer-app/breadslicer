/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use sc_mesh_core::Scene;
use sc_mesh_core::scene::node::{LeafPayload, NodeKind};

use crate::error::LoadMeshError;
use crate::filetypes::{MeshFileType, infer_mesh_file_format_from_extension};
use crate::messages::{LoadMeshWarning, SceneLoadWarning, SharedVertexLeaf};
use crate::obj::Obj;
use crate::ports::MeshReader;
use crate::stl::{Stl, StlAscii, StlBin};
use crate::threemf::ThreeMf;

#[derive(Debug)]
pub struct LoadedMesh {
    pub graph: Scene,
    pub warnings: Vec<LoadMeshWarning>,
}

/// Loads a mesh from `path`. When `kind` is `None` the format is inferred
/// from the file extension; otherwise the caller's override is used (handy
/// for `--from-format` CLI flags).
///
/// `max_file_size` is the upper bound on the on-disk byte count. When
/// `Some(cap)` and `std::fs::metadata` succeeds, the function refuses
/// the file before opening it; this is the only memory-exhaustion gate
/// for in-process readers like the OBJ parser whose worst-case
/// expansion factor is several times the on-disk size. When `None`,
/// no precheck runs. A `metadata` failure falls through as `Ok(())`:
/// the subsequent `File::open` produces a more specific I/O error.
pub fn load_mesh_from_path(
    path: &Path,
    kind: Option<MeshFileType>,
    max_file_size: Option<u64>,
) -> Result<LoadedMesh, LoadMeshError> {
    if let Some(cap) = max_file_size
        && let Ok(meta) = std::fs::metadata(path)
        && meta.len() > cap
    {
        return Err(LoadMeshError::FileTooLarge {
            actual_bytes: meta.len(),
            limit_bytes: cap,
        });
    }
    let kind = match kind {
        Some(k) => k,
        None => infer_mesh_file_format_from_extension(path)?,
    };
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    load_mesh_from_reader(kind, &mut reader)
}

/// Loads a mesh from an already-open reader. The caller provides the
/// `MeshFileType` since a raw reader has no path to infer from.
pub fn load_mesh_from_reader<R>(
    kind: MeshFileType,
    reader: &mut R,
) -> Result<LoadedMesh, LoadMeshError>
where
    R: Read + Seek,
{
    let mut warnings: Vec<LoadMeshWarning> = Vec::new();
    let graph = match kind {
        MeshFileType::Stl => {
            let (graph, stl_warnings) = Stl::read(reader)?;
            warnings.extend(stl_warnings.into_iter().map(LoadMeshWarning::Stl));
            graph
        }
        MeshFileType::StlAscii => {
            let (graph, stl_warnings) = StlAscii::read(reader)?;
            warnings.extend(stl_warnings.into_iter().map(LoadMeshWarning::Stl));
            graph
        }
        MeshFileType::StlBin => {
            let (graph, stl_warnings) = StlBin::read(reader)?;
            warnings.extend(stl_warnings.into_iter().map(LoadMeshWarning::Stl));
            graph
        }
        MeshFileType::Obj => {
            let (graph, obj_warnings) = Obj::read(reader)?;
            warnings.extend(obj_warnings.into_iter().map(LoadMeshWarning::Obj));
            graph
        }
        MeshFileType::_3mf => {
            let (graph, threemf_warnings) = ThreeMf::read(reader)?;
            warnings.extend(threemf_warnings.into_iter().map(LoadMeshWarning::ThreeMf));
            graph
        }
        other => return Err(LoadMeshError::UnsupportedFormat(other)),
    };
    if let Some(warning) = scene_shared_vertices_warning(&graph) {
        warnings.push(LoadMeshWarning::Scene(warning));
    }
    Ok(LoadedMesh { graph, warnings })
}

/// Resolves a `Scene::detect_shared_vertices` result into a
/// scene-level warning payload, pairing each offending leaf's
/// `NodeId` with its mesh-metadata name when present. Returns
/// `None` when no leaves share vertices.
fn scene_shared_vertices_warning(graph: &Scene) -> Option<SceneLoadWarning> {
    let ids = graph.detect_shared_vertices();
    if ids.is_empty() {
        return None;
    }
    let leaves = ids
        .into_iter()
        .map(|node_id| {
            let object_name = graph.node(node_id).and_then(|node| match &node.kind {
                NodeKind::Leaf(LeafPayload::Mesh(arc)) => {
                    arc.meta().as_ref().and_then(|m| m.name.clone())
                }
                NodeKind::Leaf(LeafPayload::MeshResource(rid)) => graph
                    .mesh_resource(*rid)
                    .and_then(|mesh| mesh.meta().as_ref().and_then(|m| m.name.clone())),
                _ => None,
            });
            SharedVertexLeaf {
                node_id,
                object_name,
            }
        })
        .collect();
    Some(SceneLoadWarning::SharedVertices { leaves })
}
