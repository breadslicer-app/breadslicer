/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

#![deny(missing_docs)]

//! Command-line frontend for the SoulCrafted Slicer. Slices plates,
//! converts between mesh formats, diagnoses and repairs meshes, and
//! imports, exports and lists printer and material profiles.

mod commands;
mod config;
mod filesystem;

use anyhow::Result;
use sc_mesh_core::indexed_mesh::RepairPasses;

use crate::commands::convert::convert;
use crate::commands::diagnose::diagnose;
use crate::commands::fix::fix;
use crate::commands::profiles::{
    export as profiles_export, import as profiles_import, list as profiles_list,
};
use crate::commands::slice::{SliceArgs, slice};
use crate::config::cmd_options::{CmdOptions, get_cmd_options};
use crate::config::profile_libraries::resolve_profile_libraries;

fn main() -> Result<()> {
    let cmd_options = get_cmd_options().run();
    let libraries = resolve_profile_libraries();

    match cmd_options {
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
            fix_face_normals,
        } => fix(
            source,
            source_format,
            target,
            target_format,
            RepairPasses {
                zero_length_edges: fix_zero_length_edges,
                flipped_faces: fix_flipped_faces,
                face_normals: fix_face_normals,
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
        CmdOptions::ProfilesExport {
            output,
            printers,
            materials,
            ids,
        } => profiles_export(output, printers, materials, ids, &libraries),
        CmdOptions::ProfilesImport { input, force } => profiles_import(input, force, &libraries),
        CmdOptions::ProfilesList { kind } => profiles_list(kind, &libraries),
        CmdOptions::Slice {
            source,
            target,
            target_format,
            printer_profile,
            material_profile,
            layer_height,
            first_layer_height,
            aa,
            verbose,
            no_auto_place,
            repair,
        } => slice(
            SliceArgs {
                source,
                target,
                target_format,
                printer_profile,
                material_profile,
                layer_height,
                first_layer_height,
                aa,
                verbose,
                no_auto_place,
                repair,
            },
            &libraries,
        ),
        CmdOptions::Version => {
            const SLICER_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
            println!("Version: {SLICER_CLI_VERSION}");
            Ok(())
        }
    }
}
