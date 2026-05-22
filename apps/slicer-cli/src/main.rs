/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

mod commands;
mod config;
mod filesystem;

use anyhow::Result;

use crate::commands::convert::convert;
use crate::commands::diagnose::diagnose;
use crate::commands::fix::{FixOptions, fix};
use crate::config::cmd_options::{CmdOptions, get_cmd_options};

fn main() -> Result<()> {
    let cmd_options = get_cmd_options().run();

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
            FixOptions {
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
        CmdOptions::Slice {} => {
            todo!("Not yet implemented :(");
        }
        CmdOptions::Version => {
            const SLICER_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
            println!("Version: {}", &SLICER_CLI_VERSION);
            Ok(())
        }
    }
}
