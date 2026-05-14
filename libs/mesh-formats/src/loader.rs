/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use sc_mesh_core::SceneGraph;

use crate::error::LoadMeshError;
use crate::filetypes::{MeshFileType, infer_mesh_file_format_from_extension};
use crate::messages::LoadMeshWarning;
use crate::obj::read_obj;
use crate::stl::{read_stl, read_stla, read_stlb};

pub struct LoadedMesh {
    pub graph: SceneGraph,
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
    let mut warnings = Vec::new();
    let graph = match kind {
        MeshFileType::Stl => read_stl(reader)?,
        MeshFileType::StlAscii => read_stla(reader)?,
        MeshFileType::StlBin => read_stlb(reader)?,
        MeshFileType::Obj => read_obj(reader, &mut warnings)?,
        other => return Err(LoadMeshError::UnsupportedFormat(other)),
    };
    Ok(LoadedMesh { graph, warnings })
}
