/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! Regenerates the binary STL fixtures consumed by
//! `tests/slice_e2e.rs`. Run with `cargo run --example prep_fixtures
//! -p sc-slicer-cli`. The committed STL artefacts under
//! `tests/fixtures/` are the authoritative inputs at test time; this
//! example exists so the bytes are reproducible from source if the
//! underlying meshes ever need to change.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::Result;
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_mesh_formats::stl::write_stlb;

fn main() -> Result<()> {
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    std::fs::create_dir_all(&fixtures_dir)?;

    // The unit cube fixture is centred at the origin (z in [-0.5, 0.5]).
    // The slicer schedules planes starting at h0 / 2 above the build
    // plate (z = 0), so we translate the cube up by 0.5 mm before
    // writing the STL: this puts it on the build plate (z in [0, 1])
    // and lets the slice command exercise every layer.
    let mut cube = unit_cube_mesh();
    translate_z(&mut cube, 0.5);
    write_binary_stl(&cube, &fixtures_dir.join("cube.stl"))?;

    let z_tunnel = cuboid_with_z_tunnel();
    write_binary_stl(&z_tunnel, &fixtures_dir.join("prism_z_tunnel.stl"))?;

    let y_tunnel = cuboid_with_y_tunnel();
    write_binary_stl(&y_tunnel, &fixtures_dir.join("prism_y_tunnel.stl"))?;

    println!("Wrote 3 STL fixtures to {}", fixtures_dir.display());
    Ok(())
}

fn translate_z(mesh: &mut IndexedMesh, dz: f32) {
    for vertex in &mut mesh.vertices {
        let c = vertex.coords();
        *vertex = Vertex::new([c[0], c[1], c[2] + dz]);
    }
}

fn write_binary_stl(mesh: &IndexedMesh, path: &Path) -> Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    write_stlb(mesh, &mut writer)?;
    Ok(())
}
