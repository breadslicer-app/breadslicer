/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::path::Path;

use anyhow::{Result, anyhow};
use sc_mesh_core::scene::{Scene, SceneObjectContent};
use sc_mesh_formats::filetypes::MeshFileType;
use sc_mesh_formats::obj::write_obj;
use sc_mesh_formats::stl::{write_stla, write_stlb};

pub(crate) fn write_scene(
    scene: &Scene,
    target_path: &Path,
    target_format: MeshFileType,
) -> Result<()> {
    match target_format {
        MeshFileType::Obj => {
            let mut output_file = std::fs::File::create(target_path)?;
            write_obj(scene, &mut output_file)?;
        }
        MeshFileType::Stl | MeshFileType::StlAscii | MeshFileType::StlBin => {
            let mesh_count = scene.objects.len();
            let merged;
            let mesh = match mesh_count {
                0 => return Err(anyhow!("Cannot write an empty scene to an STL file")),
                1 => match &scene.objects[0].content {
                    SceneObjectContent::Mesh(m) => m,
                    SceneObjectContent::Resource(idx) => scene
                        .resources
                        .get(*idx)
                        .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
                },
                _ => {
                    eprintln!(
                        "WARNING: Merging {} objects into a single mesh for STL output; \
                         object boundaries and names will be lost.",
                        mesh_count
                    );
                    merged = scene
                        .merge_meshes()?
                        .ok_or_else(|| anyhow!("Cannot write an empty scene to an STL file"))?;
                    &merged
                }
            };

            match target_format {
                MeshFileType::Stl | MeshFileType::StlBin => {
                    let mut output_file = std::fs::File::create(target_path)?;
                    write_stlb(mesh, &mut output_file)?;
                }
                MeshFileType::StlAscii => {
                    let mut output_file = std::fs::File::create(target_path)?;
                    write_stla(mesh, &mut output_file)?;
                }
                _ => unreachable!(),
            }
        }
        _ => todo!("Not implemented yet!"),
    }

    Ok(())
}
