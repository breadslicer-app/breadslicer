/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use anyhow::{Result, anyhow};
use sc_mesh_core::scene::Scene;

use crate::filetypes::{MeshFileType, infer_mesh_file_format_from_extension};
use crate::messages::LoadMeshWarning;
use crate::obj::read_obj;
use crate::stl::{read_stl, read_stla, read_stlb};

pub struct LoadedMesh {
    pub scene: Scene,
    pub warnings: Vec<LoadMeshWarning>,
}

/// Loads a mesh from `path`. When `kind` is `None` the format is inferred
/// from the file extension; otherwise the caller's override is used (handy
/// for `--from-format` CLI flags).
pub fn load_mesh_from_path(path: &Path, kind: Option<MeshFileType>) -> Result<LoadedMesh> {
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
pub fn load_mesh_from_reader<R>(kind: MeshFileType, reader: &mut R) -> Result<LoadedMesh>
where
    R: Read + Seek,
{
    let mut warnings = Vec::new();
    let scene = match kind {
        MeshFileType::Stl => read_stl(reader)?,
        MeshFileType::StlAscii => read_stla(reader)?,
        MeshFileType::StlBin => read_stlb(reader)?,
        MeshFileType::Obj => read_obj(reader, &mut warnings)?,
        other => return Err(anyhow!("Unsupported mesh file type: {:?}", other)),
    };
    Ok(LoadedMesh { scene, warnings })
}
