/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! CLI shim around `sc_mesh_formats::scene_writer::write_scene`.
//!
//! The library returns warnings; the CLI's choice of presentation is
//! `eprintln!` per warning.

use std::path::Path;

use anyhow::Result;
use sc_mesh_core::scene::Scene;
use sc_mesh_formats::filetypes::MeshFileType;
use sc_mesh_formats::scene_writer;

pub(crate) fn write_scene(
    scene: &Scene,
    target_path: &Path,
    target_format: MeshFileType,
) -> Result<()> {
    let warnings = scene_writer::write_scene(scene, target_path, target_format)?;
    for w in &warnings {
        eprintln!("WARNING: {}", w.user_message());
    }
    Ok(())
}
