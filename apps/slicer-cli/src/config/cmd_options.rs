/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::ffi::OsString;

use bpaf::{OptionParser, Parser, construct, long, short};
use sc_mesh_formats::filetypes::SourceMeshFileType;

#[derive(Clone, Debug)]
pub enum CmdOptions {
    Convert {
        source: OsString,
        source_format: Option<SourceMeshFileType>,
        target: OsString,
        target_format: Option<SourceMeshFileType>,
    },
    Diagnose {},
    Fix {},
    Inspect {},
    Slice {},
    Version,
}

fn get_convert_options() -> impl Parser<CmdOptions> {
    let source = short('s')
        .long("source")
        .help("Specify the path of the file to be converted")
        .argument::<OsString>("SOURCE_FILEPATH");

    let source_format = long("source-format")
        .help("Specify the file format of the source file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<SourceMeshFileType>("SOURCE_FORMAT")
        .optional();

    let target = short('t')
        .long("target")
        .help("Specify the path for the converted file")
        .argument::<OsString>("TARGET_FILEPATH");

    let target_format = long("target-format")
        .help("Specify the file format of the target file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<SourceMeshFileType>("TARGET_FORMAT")
        .optional();

    construct!(CmdOptions::Convert {
        source,
        source_format,
        target,
        target_format,
    })
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
