/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Result, anyhow};
use sc_mesh_core::scene::Scene;
use sc_mesh_formats::filetypes::{MeshFileType, infer_mesh_file_format_from_extension};
use sc_mesh_formats::obj::read_obj;
use sc_mesh_formats::stl::{read_stl, read_stla, read_stlb};

pub fn load_scene(path: &Path) -> Result<Scene> {
    let file_type = infer_mesh_file_format_from_extension(path)?;
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    match file_type {
        MeshFileType::Stl => read_stl(&mut reader),
        MeshFileType::StlAscii => read_stla(&mut reader),
        MeshFileType::StlBin => read_stlb(&mut reader),
        MeshFileType::Obj => {
            let mut warnings = Vec::new();
            read_obj(&mut reader, &mut warnings)
        }
        other => Err(anyhow!("Unsupported mesh file type: {:?}", other)),
    }
}
