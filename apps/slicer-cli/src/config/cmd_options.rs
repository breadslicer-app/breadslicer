/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::ffi::OsString;
use std::str::FromStr;

use bpaf::{OptionParser, Parser, construct, long, positional, short};
use sc_mesh_core::indexed_mesh::RepairPasses;
use sc_mesh_formats::filetypes::MeshFileType;
use sc_sl1cer::profile::{AaLevel, PositiveFiniteMm};

use crate::config::profile_ref::{MaterialProfileRef, PrinterProfileRef};
use crate::config::target_format::{ACCEPTED_TARGET_FORMATS, TargetFormat};

/// Which kinds of profile a `profiles list` invocation lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListKind {
    All,
    Printers,
    Materials,
}

impl FromStr for ListKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "printers" => Ok(Self::Printers),
            "materials" => Ok(Self::Materials),
            other => Err(format!("expected 'printers' or 'materials', got '{other}'")),
        }
    }
}

/// A `X,Y,Z` triple of `f64`s, parsed from one CLI argument. Used by
/// `support-gen` for gravity, boundary corners, and anchor points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point3(pub f64, pub f64, pub f64);

impl FromStr for Point3 {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = value.split(',').collect();
        let [x, y, z] = parts.as_slice() else {
            return Err(format!(
                "expected 3 comma-separated numbers (X,Y,Z), got '{value}'"
            ));
        };
        let parse = |s: &str| {
            s.parse::<f64>()
                .map_err(|_| format!("'{s}' is not a number"))
        };
        Ok(Self(parse(x)?, parse(y)?, parse(z)?))
    }
}

impl From<Point3> for sc_mesh_core::Vector3D<f64> {
    fn from(p: Point3) -> Self {
        sc_mesh_core::Vector3D::new([p.0, p.1, p.2])
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CmdOptions {
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
    ProfilesExport {
        output: OsString,
        printers: bool,
        materials: bool,
        ids: Vec<String>,
    },
    ProfilesImport {
        input: OsString,
        force: bool,
    },
    ProfilesList {
        kind: ListKind,
    },
    Slice {
        source: OsString,
        target: OsString,
        target_format: Option<TargetFormat>,
        printer_profile: PrinterProfileRef,
        material_profile: MaterialProfileRef,
        layer_height: Option<PositiveFiniteMm>,
        first_layer_height: Option<PositiveFiniteMm>,
        aa: AaLevel,
        verbose: bool,
        no_auto_place: bool,
        repair: bool,
    },
    GenSupport {
        source: OsString,
        source_format: Option<MeshFileType>,
        target: OsString,
        target_format: Option<MeshFileType>,
        radius_mm: f64,
        gravity: Point3,
        boundary_min: Point3,
        boundary_max: Point3,
        anchors: Vec<Point3>,
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

    // Flag fallbacks come from the shared default pass set, so the
    // `fix` defaults, `slice --repair`, and any other frontend cannot
    // drift apart.
    let fix_zero_length_edges = long("fix-zero-length-edges")
        .help("Removes faces with zero-length edges")
        .argument::<bool>("FIX_ZERO_LENGTH_EDGES")
        .adjacent()
        .fallback(RepairPasses::default_repair().zero_length_edges);

    let fix_flipped_faces = long("fix-flipped-faces")
        .help("Flips faces pointing to the wrong direction")
        .argument::<bool>("FIX_FLIPPED_FACES")
        .adjacent()
        .fallback(RepairPasses::default_repair().flipped_faces);

    let fix_face_normals = long("fix-face-normals")
        .help("Fixes face normal vectors")
        .argument::<bool>("FIX_FACE_NORMALS")
        .adjacent()
        .fallback(RepairPasses::default_repair().face_normals);

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
    let source = short('s')
        .long("source")
        .help("Specify the path of the 3D model file to be sliced")
        .argument::<OsString>("SOURCE_FILEPATH");

    let target = short('t')
        .long("target")
        .help(
            "Specify the output path. A single file for packed formats (e.g. svg-combined, 3mf), a directory for per-layer formats (svg-per-layer, png-per-layer).",
        )
        .argument::<OsString>("TARGET_PATH");

    let target_format_help = format!(
        "Specify the output format. Accepted values: {ACCEPTED_TARGET_FORMATS}. Inferred from the target file extension when omitted.",
    );
    let target_format = long("target-format")
        .help(target_format_help.as_str())
        .argument::<TargetFormat>("TARGET_FORMAT")
        .optional();

    let printer_profile = long("printer-profile")
        .help(
            "Printer profile to slice for: either a profile id (slug) looked up in the configured libraries, or a path to a profile TOML file (a value starting with a path marker such as '/' or './').",
        )
        .argument::<PrinterProfileRef>("PRINTER_PROFILE");

    let material_profile = long("material-profile")
        .help(
            "Material profile to slice with: either a profile id (slug) looked up in the configured libraries, or a path to a profile TOML file (a value starting with a path marker such as '/' or './').",
        )
        .argument::<MaterialProfileRef>("MATERIAL_PROFILE");

    let layer_height = long("layer-height")
        .help(
            "Optional layer-height override in millimetres (strictly positive). When omitted, the layer height comes from the resolved material profile.",
        )
        .argument::<PositiveFiniteMm>("MM")
        .optional();

    let first_layer_height = long("first-layer-height")
        .help(
            "Optional thicker first-layer height in millimetres (strictly positive); defaults to the regular layer height when omitted.",
        )
        .argument::<PositiveFiniteMm>("MM")
        .optional();

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
            "Skip auto-placement: slice the scene at its file coordinates instead of centring it on the build plate with its lowest point at z = 0. Multi-model scenes keep their relative layout either way.",
        )
        .switch();

    let repair = long("repair")
        .help(
            "Repair the loaded mesh before slicing, using the same defaults as the `fix` subcommand (zero-length-edge removal and flipped-face correction enabled, normal recomputation off). Pre-existing problems that the enabled passes cannot address still propagate as a slicer validation error.",
        )
        .switch();

    construct!(CmdOptions::Slice {
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
    })
}

fn get_support_gen_options() -> impl Parser<CmdOptions> {
    let source = short('s')
        .long("source")
        .help("Specify the path of the mesh file to generate trivial support structures for")
        .argument::<OsString>("SOURCE_FILEPATH");

    let source_format = long("source-format")
        .help("Specify the file format of the source file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<MeshFileType>("SOURCE_FORMAT")
        .optional();

    let target = short('t')
        .long("target")
        .help("Specify the path to write the generated support mesh(es) to")
        .argument::<OsString>("TARGET_FILEPATH");

    let target_format = long("target-format")
        .help("Specify the file format of the target file. Supported file formats: stl, stla, stlb, obj, ply, amf, 3mf.")
        .argument::<MeshFileType>("TARGET_FORMAT")
        .optional();

    // See note on "Strut" in support-gen-trivial/src/trivial_support.rs
    let radius_mm = long("radius-mm")
        .help("Strut radius in millimetres")
        .argument::<f64>("MM")
        .fallback(1.0);

    let gravity = long("gravity")
        .help("Direction things fall, as X,Y,Z (need not be a unit vector; default -Z)")
        .argument::<Point3>("X,Y,Z")
        .fallback(Point3(0.0, 0.0, -1.0));

    let boundary_min = long("boundary-min")
        .help("Minimum corner of the job's boundary box, as X,Y,Z in millimetres")
        .argument::<Point3>("X,Y,Z")
        .fallback(Point3(-500.0, -500.0, -500.0));

    let boundary_max = long("boundary-max")
        .help("Maximum corner of the job's boundary box, as X,Y,Z in millimetres")
        .argument::<Point3>("X,Y,Z")
        .fallback(Point3(500.0, 500.0, 500.0));

    let anchors = long("anchor")
        .help(
            "Explicit support anchor point, as X,Y,Z in millimetres (repeatable). Omit entirely to auto-detect one.",
        )
        .argument::<Point3>("X,Y,Z")
        .many();

    construct!(CmdOptions::GenSupport {
        source,
        source_format,
        target,
        target_format,
        radius_mm,
        gravity,
        boundary_min,
        boundary_max,
        anchors,
    })
}

fn get_profiles_list_options() -> impl Parser<CmdOptions> {
    let kind = positional::<ListKind>("KIND")
        .help("Restrict the listing to 'printers' or 'materials'; lists everything when omitted.")
        .fallback(ListKind::All);
    construct!(CmdOptions::ProfilesList { kind })
}

fn get_profiles_export_options() -> impl Parser<CmdOptions> {
    let printers = long("printers")
        .help("Export only printer profiles. Combined with --materials this exports both kinds.")
        .switch();
    let materials = long("materials")
        .help("Export only material profiles. Combined with --printers this exports both kinds.")
        .switch();
    let ids = long("id")
        .help(
            "Export exactly the named profile id (repeatable). Mutually exclusive with --printers / --materials.",
        )
        .argument::<String>("ID")
        .many();
    let output = positional::<OsString>("OUTPUT_ZIP").help("Path of the zip archive to write.");
    construct!(CmdOptions::ProfilesExport {
        printers,
        materials,
        ids,
        output,
    })
}

fn get_profiles_import_options() -> impl Parser<CmdOptions> {
    let force = long("force")
        .help("Overwrite profiles in the user library that the package would replace.")
        .switch();
    let input = positional::<OsString>("PACKAGE_ZIP").help("Path of the zip archive to import.");
    construct!(CmdOptions::ProfilesImport { force, input })
}

fn get_profiles_options() -> impl Parser<CmdOptions> {
    let list = get_profiles_list_options()
        .to_options()
        .descr("List the profiles in the user library")
        .command("list");
    let export = get_profiles_export_options()
        .to_options()
        .descr("Export profiles from the user library into a zip archive")
        .command("export");
    let import = get_profiles_import_options()
        .to_options()
        .descr("Import profiles from a zip archive into the user library")
        .command("import");
    construct!([list, export, import])
}

pub(crate) fn get_cmd_options() -> OptionParser<CmdOptions> {
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
    let profiles = get_profiles_options()
        .to_options()
        .descr("List, export, and import profiles in the user library")
        .command("profiles");
    let slice = get_slice_options()
        .to_options()
        .descr("Slice a given 3d model file")
        .command("slice");
    let support_gen = get_support_gen_options()
        .to_options()
        .descr(
            "Run the sc-geom toy trivial support reference algorithm end to end (interface/integration demo, not a real support-generation feature)",
        )
        .command("support-gen");
    let version = construct!(CmdOptions::Version {})
        .to_options()
        .descr("Prints version information")
        .command("version");

    construct!([
        convert,
        diagnose,
        fix,
        help,
        inspect,
        profiles,
        slice,
        support_gen,
        version
    ])
        .to_options()
        .descr("sc-slicer-cli (SoulCrafted Slicer, CLI) is an FLOSS command line slicer for resin 3d printers")
        // .header(header) // TODO: Implement these lines for the embedded help
        .footer("Add `--help` after any of the subcommands to learn more about them.")
        .fallback_to_usage()
        .version(env!("CARGO_PKG_VERSION"))
}
