/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

#![deny(missing_docs)]

//! Command-line frontend for the SoulCrafted Slicer. Slices plates, converts
//! between mesh formats, diagnoses and repairs meshes, and imports, exports and
//! lists printer and material profiles.

// The review and the slicer's workers allocate and free layer buffers from
// every core at once, and the Windows heap makes them queue: measured on a
// 3536-layer plate, the sweep over its layers took 3.3 s on it and 0.6 s on
// this one (#432).
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod commands;
mod config;
mod filesystem;

use anyhow::Result;
use sc_mesh_core::indexed_mesh::RepairPasses;

use crate::commands::bodies::bodies;
use crate::commands::convert::convert;
use crate::commands::diagnose::diagnose;
use crate::commands::fix::fix;
use crate::commands::orient::orient;
use crate::commands::profiles::{
    export as profiles_export, import as profiles_import, list as profiles_list,
};
use crate::commands::review::{ReviewArgs, review};
use crate::commands::slice::{SliceArgs, slice};
use crate::commands::support_gen::{GenSupportArgs, support_gen};
use crate::config::cmd_options::{CmdOptions, get_cmd_options};
use crate::config::profile_libraries::resolve_profile_libraries;

fn main() -> Result<()> {
    // The formats ported from UVtools write through `sc-sla-formats` once
    // installed.
    sc_sla_formats_uvtools::install();
    let cmd_options = get_cmd_options().run();
    let libraries = resolve_profile_libraries();

    match cmd_options {
        CmdOptions::Bodies(args) => bodies(args),
        CmdOptions::Convert {
            source,
            source_format,
            target,
            target_format,
        } => convert(source, source_format, target, target_format),
        CmdOptions::Diagnose {
            source,
            source_format,
        } => diagnose(source, source_format, &mut std::io::stderr()),
        CmdOptions::Fix {
            source,
            source_format,
            target,
            target_format,
            fix_zero_length_edges,
            fix_flipped_faces,
            fix_collinear_faces,
            fix_non_manifold_edges,
            fix_holes,
        } => fix(
            source,
            source_format,
            target,
            target_format,
            RepairPasses {
                zero_length_edges: fix_zero_length_edges,
                flipped_faces: fix_flipped_faces,
                collinear_faces: fix_collinear_faces,
                non_manifold_edges: fix_non_manifold_edges,
                holes: fix_holes,
            },
            &mut std::io::stderr(),
        ),
        CmdOptions::Help => {
            let help_text = get_cmd_options()
                .run_inner(&["-h"])
                .unwrap_err()
                .unwrap_stdout();
            println!("{}", help_text);
            Ok(())
        }
        CmdOptions::Inspect {} => {
            todo!("Not yet implemented :(");
        }
        CmdOptions::Orient(args) => orient(args, &libraries),
        CmdOptions::ProfilesExport {
            output,
            printers,
            materials,
            ids,
        } => profiles_export(output, printers, materials, ids, &libraries),
        CmdOptions::ProfilesImport { input, force } => profiles_import(input, force, &libraries),
        CmdOptions::ProfilesList { kind } => profiles_list(kind, &libraries),
        CmdOptions::Review {
            source,
            plate,
            json,
            fill,
            fix_all,
        } => review(
            ReviewArgs {
                source,
                plate,
                json,
                fill,
                fix_all,
            },
            &libraries,
        ),
        CmdOptions::Slice {
            source,
            target,
            target_format,
            plate,
        } => slice(
            SliceArgs {
                source,
                target,
                target_format,
                plate,
            },
            &libraries,
        ),
        CmdOptions::GenSupport {
            source,
            source_format,
            target,
            target_format,
            radius_mm,
            gravity,
            boundary_min,
            boundary_max,
            anchors,
        } => support_gen(GenSupportArgs {
            source,
            source_format,
            target,
            target_format,
            radius_mm,
            gravity,
            boundary_min,
            boundary_max,
            anchors,
        }),
        CmdOptions::Version => {
            const SLICER_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
            println!("Version: {SLICER_CLI_VERSION}");
            Ok(())
        }
    }
}
