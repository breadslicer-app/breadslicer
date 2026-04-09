/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fmt::Display;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Result, anyhow};
use serde::Deserialize;

#[derive(PartialEq, Eq)]
pub struct InvalidMeshFileType;

impl Display for InvalidMeshFileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid mesh file format type")
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum MeshFileType {
    Stl,
    StlAscii,
    StlBin,
    Obj,
    Ply,
    Amf,
    _3mf,
}

impl FromStr for MeshFileType {
    type Err = InvalidMeshFileType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "STL" => Ok(MeshFileType::Stl),
            "STLA" => Ok(MeshFileType::StlAscii),
            "STLB" => Ok(MeshFileType::StlBin),
            "STL_ASCII" => Ok(MeshFileType::StlAscii),
            "STL_BIN" => Ok(MeshFileType::StlBin),
            "OBJ" => Ok(MeshFileType::Obj),
            "PLY" => Ok(MeshFileType::Ply),
            "AMF" => Ok(MeshFileType::Amf),
            "3MF" => Ok(MeshFileType::_3mf),
            _ => Err(InvalidMeshFileType),
        }
    }
}

pub fn infer_mesh_file_format_from_extension(path: &Path) -> Result<MeshFileType> {
    let extension = match path.extension() {
        Some(ext) => ext.to_str().unwrap_or(""),
        None => "",
    }
    .to_lowercase();

    match extension.as_str() {
        "stl" => Ok(MeshFileType::Stl),
        "stla" => Ok(MeshFileType::StlAscii),
        "stlb" => Ok(MeshFileType::StlBin),
        "obj" => Ok(MeshFileType::Obj),
        "ply" => Ok(MeshFileType::Ply),
        "amf" => Ok(MeshFileType::Amf),
        "3mf" => Ok(MeshFileType::_3mf),
        _ => Err(anyhow!(
            "Unable to infer file format from filename extension for \"{:?}\"",
            path
        )),
    }
}
