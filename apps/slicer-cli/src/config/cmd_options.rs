/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use bpaf::{OptionParser, Parser, construct};

#[derive(Clone, Debug)]
pub enum CmdOptions {
    Convert {},
    Diagnose {},
    Fix {},
    Inspect {},
    Slice {},
    Version,
}

fn get_convert_options() -> impl Parser<CmdOptions> {
    construct!(CmdOptions::Convert {})
}

fn get_diagnose_options() -> impl Parser<CmdOptions> {
    construct!(CmdOptions::Diagnose {})
}

fn get_fix_options() -> impl Parser<CmdOptions> {
    construct!(CmdOptions::Fix {})
}

fn get_inspect_options() -> impl Parser<CmdOptions> {
    construct!(CmdOptions::Inspect {})
}

fn get_slice_options() -> impl Parser<CmdOptions> {
    construct!(CmdOptions::Slice {})
}

pub fn get_cmd_options() -> OptionParser<CmdOptions> {
    let convert = get_convert_options()
        .to_options()
        .descr("Convert between file formats")
        .command("convert");
    let diagnose = get_diagnose_options()
        .to_options()
        .descr("(Try to) diagnose potential problems in mesh files")
        .command("diagnose");
    let fix = get_fix_options()
        .to_options()
        .descr("(Try to) fix a mesh file")
        .command("fix");
    let inspect = get_inspect_options()
        .to_options()
        .descr("Inspect a file to obtain some details")
        .command("inspect");
    let slice = get_slice_options()
        .to_options()
        .descr("Slice a given 3d model file")
        .command("slice");
    let version = construct!(CmdOptions::Version {})
        .to_options()
        .command("version");

    construct!([convert, diagnose, fix, inspect, slice, version]).to_options()
}
