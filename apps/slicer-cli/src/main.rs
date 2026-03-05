/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

mod config;

use config::cmd_options::{CmdOptions, get_cmd_options};

fn main() {
    let cmd_options = get_cmd_options().run();

    match cmd_options {
        CmdOptions::Convert {
            source,
            source_format,
            target,
            target_format,
        } => {
            todo!("Not yet implemented :(");
        }
        CmdOptions::Diagnose {} => {
            todo!("Not yet implemented :(");
        }
        CmdOptions::Fix {} => {
            todo!("Not yet implemented :(");
        }
        CmdOptions::Inspect {} => {
            todo!("Not yet implemented :(");
        }
        CmdOptions::Slice {} => {
            todo!("Not yet implemented :(");
        }
        CmdOptions::Version => {
            const SLICER_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
            println!("SoulCrafted Slicer (CLI) v{}", &SLICER_CLI_VERSION);
        }
    }
}
