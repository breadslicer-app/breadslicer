/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! `support-gen`: the CLI's integration touchpoint for `sc-geom`.
//!
//! Runs the crate's deliberately trivial `TrivialSupport` support
//! algorithm "end to end": support_general_specs resolution, a worker-thread job
//! with live progress and diagnostics, and a reproducible recipe --
//! against a real mesh, so the interface is proven inside the actual
//! product rather than only in `sc-geom`'s own test suite. Not a
//! real support-generation feature: see `sc-geom`'s README for
//! what is implemented.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use sc_geom::{
    BoundaryShape, PrintPlate, SupportStructureSpecs, TrivialSupport, TrivialSupportSpecs,
    run_geom_algorithm,
};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_formats::filetypes::{
    MeshFileType, WriteFormat, infer_mesh_file_format_from_extension,
};
use sc_mesh_formats::load_mesh_from_path;
use sc_scene::{LeafPayload, Scene};

use crate::commands::mesh_loader::display_warnings;
use crate::commands::mesh_writer::write_scene;
use crate::config::cmd_options::Point3;
use crate::filesystem::{
    ensure_distinct_files, validate_sourcefile_path, validate_targetfile_path,
};

pub(crate) struct GenSupportArgs {
    pub source: OsString,
    pub source_format: Option<MeshFileType>,
    pub target: OsString,
    pub target_format: Option<MeshFileType>,
    pub radius_mm: f64,
    pub gravity: Point3,
    pub boundary_min: Point3,
    pub boundary_max: Point3,
    pub anchors: Vec<Point3>,
}

pub(crate) fn support_gen(args: GenSupportArgs) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(args.source))?;
    let target_path = validate_targetfile_path(PathBuf::from(args.target))?;
    ensure_distinct_files(&source_path, &target_path)?;

    let target_format = match args.target_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&target_path)?,
    };
    let target_write_format = WriteFormat::try_from(target_format)?;

    let loaded = load_mesh_from_path(
        &source_path,
        args.source_format,
        Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES),
    )?;
    display_warnings(&loaded.warnings, &mut std::io::stderr());

    let boundary_shape = BoundaryShape::BuildVolumeBox {
        min: args.boundary_min.into(),
        max: args.boundary_max.into(),
    };
    let print_plate = PrintPlate {
        boundary: boundary_shape.clone(),
        plate_normal: sc_mesh_core::Vector3D::new([0.0, 0.0, 1.0]),
    };
    let support_general_specs = SupportStructureSpecs {
        gravity: args.gravity.into(),
        boundary_shape,
        print_plate,
        explicit_locations: args.anchors.into_iter().map(Into::into).collect(),
        allow_expand_touch: false,
        allow_expand_trunk: false,
    };
    let specific_alg_specs = TrivialSupportSpecs {
        radius_mm: args.radius_mm,
    };

    let handle = run_geom_algorithm(
        &TrivialSupport,
        &loaded.graph,
        &support_general_specs,
        &specific_alg_specs,
    )
    .map_err(|e| anyhow!("could not start support-gen job: {e}"))?;

    let result = loop {
        if let Some(result) = handle.try_poll() {
            break result;
        }
        let progress = handle.progress();
        eprintln!(
            "[support-gen] {} ({}/{})",
            progress.phase, progress.completed, progress.total
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    .map_err(|e| anyhow!("support-gen job failed: {e}"))?;

    for diagnostic in handle.diagnostics_snapshot() {
        eprintln!(
            "[support-gen] {:?}: {}",
            diagnostic.severity, diagnostic.message
        );
    }

    if result.meshes.is_empty() {
        return Err(anyhow!(
            "support-gen produced no meshes; see the diagnostics above"
        ));
    }

    let mut out_scene = Scene::new();
    for mesh in result.meshes {
        let leaf = out_scene.add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh)),
        )?;
        out_scene.add_root(leaf)?;
    }
    write_scene(&out_scene, &target_path, target_write_format)?;

    println!("resolution trail:");
    for record in &result.resolution_trail {
        println!(
            "  {} <- {:?} = {}",
            record.field, record.source, record.final_value
        );
    }

    println!(
        "recipe: algorithm={} version={}",
        result.recipe.algorithm_id, result.recipe.algorithm_version
    );
    for (key, value) in &result.recipe.resolved_settings {
        println!("  {key} = {value}");
    }
    println!("input mesh hash: {}", result.recipe.input_mesh_hash);
    println!(
        "estimated material: {:?} g",
        result.algorithm_specs.estimated_material_g
    );

    Ok(())
}
