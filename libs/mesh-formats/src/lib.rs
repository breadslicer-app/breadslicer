/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fmt::Display;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Result, anyhow};
use serde::Deserialize;

#[derive(PartialEq, Eq)]
pub struct InvalidSourceMeshFileType;

impl Display for InvalidSourceMeshFileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid mesh file format type")
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub enum SourceMeshFileType {
    Stl,
    StlAscii,
    StlBin,
    Obj,
    Ply,
    Amf,
    _3mf,
}

impl FromStr for SourceMeshFileType {
    type Err = InvalidSourceMeshFileType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "STL" => Ok(SourceMeshFileType::Stl),
            "STLA" => Ok(SourceMeshFileType::StlAscii),
            "STLB" => Ok(SourceMeshFileType::StlBin),
            "STL_ASCII" => Ok(SourceMeshFileType::StlAscii),
            "STL_BIN" => Ok(SourceMeshFileType::StlBin),
            "OBJ" => Ok(SourceMeshFileType::Obj),
            "PLY" => Ok(SourceMeshFileType::Ply),
            "AMF" => Ok(SourceMeshFileType::Amf),
            "3MF" => Ok(SourceMeshFileType::_3mf),
            _ => Err(InvalidSourceMeshFileType),
        }
    }
}

pub fn infer_mesh_file_format_from_extension(target_path: &Path) -> Result<SourceMeshFileType> {
    let extension = match target_path.extension() {
        Some(ext) => ext.to_str().unwrap_or(""),
        None => "",
    }
    .to_lowercase();

    match extension.as_str() {
        "stl" => Ok(SourceMeshFileType::Stl),
        "stla" => Ok(SourceMeshFileType::StlAscii),
        "stlb" => Ok(SourceMeshFileType::StlBin),
        "obj" => Ok(SourceMeshFileType::Obj),
        "ply" => Ok(SourceMeshFileType::Ply),
        "amf" => Ok(SourceMeshFileType::Amf),
        "3mf" => Ok(SourceMeshFileType::_3mf),
        _ => Err(anyhow!(
            "Unable to infer file format from filename extension for \"{:?}\"",
            target_path
        )),
    }
}
