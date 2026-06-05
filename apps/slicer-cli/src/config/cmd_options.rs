/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::ffi::OsString;
use std::num::NonZeroU32;

use bpaf::{OptionParser, Parser, construct, long, short};
use sc_mesh_formats::filetypes::MeshFileType;
use sc_sl1cer::profile::{AaLevel, PositiveFiniteMm};
use sc_sla_formats::Format;

#[derive(Clone, Debug)]
pub enum CmdOptions {
    Convert {
        source: OsString,
        source_format: Option<MeshFileType>,
        target: OsString,
        target_format: Option<MeshFileType>,
    },
    Diagnose {
        source: OsString,
        source_format: Option<MeshFileType>,
    },
    Fix {
        source: OsString,
        source_format: Option<MeshFileType>,
        target: OsString,
        target_format: Option<MeshFileType>,
        fix_zero_length_edges: bool,
        fix_flipped_faces: bool,
        fix_face_normals: bool,
    },
    Help,
    Inspect {},
    Slice {
        input: OsString,
        output: OsString,
        format: Option<Format>,
        layer_height: PositiveFiniteMm,
        first_layer_height: Option<PositiveFiniteMm>,
        pixels_x: NonZeroU32,
        pixels_y: NonZeroU32,
        build_x: PositiveFiniteMm,
        build_y: PositiveFiniteMm,
        aa: AaLevel,
        verbose: bool,
        no_auto_place: bool,
    },
    Version,
}

fn get_convert_options() -> impl Parser<CmdOptions> {
    let source = short('s')
        .long("source")
        .help("Specify the path of the file to be converted")
        .argument::<OsString>("SOURCE_FILEPATH");

    let source_format = long("source-format")
        .help("Specify the file format of the source file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<MeshFileType>("SOURCE_FORMAT")
        .optional();

    let target = short('t')
        .long("target")
        .help("Specify the path for the converted file")
        .argument::<OsString>("TARGET_FILEPATH");

    let target_format = long("target-format")
        .help("Specify the file format of the target file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<MeshFileType>("TARGET_FORMAT")
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
        .argument::<MeshFileType>("SOURCE_FORMAT")
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
        .argument::<MeshFileType>("SOURCE_FORMAT")
        .optional();

    let target = short('t')
        .long("target")
        .help("Specify the path for the fixed file")
        .argument::<OsString>("TARGET_FILEPATH");

    let target_format = long("target-format")
        .help("Specify the file format of the target file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<MeshFileType>("TARGET_FORMAT")
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
    let input = short('i')
        .long("input")
        .help("Path to the input 3D model file")
        .argument::<OsString>("INPUT_FILEPATH");

    let output = short('o')
        .long("output")
        .help(
            "Output path. A single file for packed formats (e.g. svg-combined), a directory for per-layer formats (svg-per-layer, png-per-layer).",
        )
        .argument::<OsString>("OUTPUT_PATH");

    let format = long("format")
        .help(
            "Output format. Accepted values: svg-per-layer, svg-combined, png-per-layer. Inferred from the output file extension when omitted.",
        )
        .argument::<Format>("FORMAT")
        .optional();

    let layer_height = long("layer-height")
        .help("Layer height in millimetres (strictly positive)")
        .argument::<PositiveFiniteMm>("MM");

    let first_layer_height = long("first-layer-height")
        .help(
            "Optional thicker first-layer height in millimetres (strictly positive); defaults to the regular layer height when omitted.",
        )
        .argument::<PositiveFiniteMm>("MM")
        .optional();

    let pixels_x = long("pixels-x")
        .help("Print panel pixel resolution along X (non-zero)")
        .argument::<NonZeroU32>("PIXELS");

    let pixels_y = long("pixels-y")
        .help("Print panel pixel resolution along Y (non-zero)")
        .argument::<NonZeroU32>("PIXELS");

    let build_x = long("build-x")
        .help("Build volume X dimension in millimetres (strictly positive)")
        .argument::<PositiveFiniteMm>("MM");

    let build_y = long("build-y")
        .help("Build volume Y dimension in millimetres (strictly positive)")
        .argument::<PositiveFiniteMm>("MM");

    let aa = long("aa")
        .help("Per-pixel anti-aliasing: 0 (off), 2, 4, or 8")
        .argument::<AaLevel>("AA")
        .fallback(AaLevel::Off);

    let verbose = short('v')
        .long("verbose")
        .help(
            "On a mesh-validation failure, list every offending face grouped by error variant. Without this flag the slicer prints only an aggregated summary.",
        )
        .switch();

    let no_auto_place = long("no-auto-place")
        .help(
            "Skip auto-placement: slice the model at its file coordinates instead of centring it on the build plate with its lowest point at z = 0.",
        )
        .switch();

    construct!(CmdOptions::Slice {
        input,
        output,
        format,
        layer_height,
        first_layer_height,
        pixels_x,
        pixels_y,
        build_x,
        build_y,
        aa,
        verbose,
        no_auto_place,
    })
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
        .descr("sc-slicer-cli (SoulCrafted Slicer, CLI) is an FLOSS command line slicer for resin 3d printers")
        // .header(header) // TODO: Implement these lines for the embedded help
        .footer("Add `--help` after any of the subcommands to learn more about them.")
        .fallback_to_usage()
        .version(env!("CARGO_PKG_VERSION"))
}
