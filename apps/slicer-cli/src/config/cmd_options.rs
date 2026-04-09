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
    Diagnose {
        source: OsString,
        source_format: Option<SourceMeshFileType>,
    },
    Fix {
        source: OsString,
        source_format: Option<SourceMeshFileType>,
        target: OsString,
        target_format: Option<SourceMeshFileType>,
        fix_zero_length_edges: bool,
        fix_flipped_faces: bool,
        fix_face_normals: bool,
    },
    Help,
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
    let source = short('s')
        .long("source")
        .help("Specify the path of the file to be diagnosed")
        .argument::<OsString>("SOURCE_FILEPATH");

    let source_format = long("source-format")
        .help("Specify the file format of the source file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<SourceMeshFileType>("SOURCE_FORMAT")
        .optional();

    construct!(CmdOptions::Diagnose {
        source,
        source_format,
    })
}

fn get_fix_options() -> impl Parser<CmdOptions> {
    let source = short('s')
        .long("source")
        .help("Specify the path of the file to be fixed")
        .argument::<OsString>("SOURCE_FILEPATH");

    let source_format = long("source-format")
        .help("Specify the file format of the source file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<SourceMeshFileType>("SOURCE_FORMAT")
        .optional();

    let target = short('t')
        .long("target")
        .help("Specify the path for the fixed file")
        .argument::<OsString>("TARGET_FILEPATH");

    let target_format = long("target-format")
        .help("Specify the file format of the target file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<SourceMeshFileType>("TARGET_FORMAT")
        .optional();

    let fix_zero_length_edges = long("fix-zero-length-edges")
        .help("Removes faces with zero-length edges")
        .argument::<bool>("FIX_ZERO_LENGTH_EDGES")
        .adjacent()
        .fallback(true);

    let fix_flipped_faces = long("fix-flipped-faces")
        .help("Flips faces pointing to the wrong direction")
        .argument::<bool>("FIX_FLIPPED_FACES")
        .adjacent()
        .fallback(true);

    let fix_face_normals = long("fix-face-normals")
        .help("Fixes face normal vectors")
        .argument::<bool>("FIX_FACE_NORMALS")
        .adjacent()
        .fallback(false);

    construct!(CmdOptions::Fix {
        source,
        source_format,
        target,
        target_format,
        fix_zero_length_edges,
        fix_flipped_faces,
        fix_face_normals,
    })
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
    let help = construct!(CmdOptions::Help {})
        .to_options()
        .descr("Prints help information")
        .command("help");
    let slice = get_slice_options()
        .to_options()
        .descr("Slice a given 3d model file")
        .command("slice");
    let version = construct!(CmdOptions::Version {})
        .to_options()
        .descr("Prints version information")
        .command("version");

    construct!([convert, diagnose, fix, help, inspect, slice, version])
        .to_options()
        .descr("sc-slicer-cli (SoulCrafter Slicer, CLI) is an FLOSS command line slicer for resin 3d printers")
        // .header(header) // TODO: Implement these lines for the embedded help
        .footer("Add `--help` after any of the subcommands to learn more about them.")
        .fallback_to_usage()
        .version(env!("CARGO_PKG_VERSION"))
}
