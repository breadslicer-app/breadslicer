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

/// The `MIN:MAX` range of `--adaptive-layers`, in millimetres (#60).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AdaptiveRange {
    pub min_mm: PositiveFiniteMm,
    pub max_mm: PositiveFiniteMm,
}

impl FromStr for AdaptiveRange {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((min, max)) = value.split_once(':') else {
            return Err(format!(
                "expected the thinnest and the thickest layer as MIN:MAX, got '{value}'"
            ));
        };
        let parse = |s: &str| {
            s.trim()
                .parse::<f64>()
                .ok()
                .and_then(|mm| PositiveFiniteMm::new(mm).ok())
                .ok_or_else(|| format!("'{s}' is not a positive length in millimetres"))
        };
        let (min_mm, max_mm) = (parse(min)?, parse(max)?);
        if min_mm.value() > max_mm.value() {
            return Err(format!(
                "the thinnest layer ({min}) is thicker than the thickest ({max})"
            ));
        }
        Ok(Self { min_mm, max_mm })
    }
}

/// A `X,Y,Z` triple of `f64`s, parsed from 1 CLI argument. Used by
/// `orient` for the build volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Point3(pub(crate) f64, pub(crate) f64, pub(crate) f64);

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

/// The options `slice` and `review` share: the printer and material, and how
/// the model is prepared and sliced.
#[derive(Clone, Debug)]
pub(crate) struct PlateOptions {
    pub printer_profile: PrinterProfileRef,
    pub material_profile: MaterialProfileRef,
    pub layer_height: Option<PositiveFiniteMm>,
    pub first_layer_height: Option<PositiveFiniteMm>,
    pub adaptive_layers: Option<AdaptiveRange>,
    pub aa: AaLevel,
    pub verbose: bool,
    pub no_auto_place: bool,
    pub repair: bool,
    pub generate_supports: bool,
    pub support_lift: f64,
}

#[derive(Clone, Debug)]
pub(crate) enum CmdOptions {
    Bodies(crate::commands::bodies::BodiesArgs),
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
        fix_collinear_faces: bool,
        fix_non_manifold_edges: bool,
        fix_holes: bool,
    },
    Help,
    Inspect {},
    Orient(crate::commands::orient::OrientArgs),
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
    Review {
        source: OsString,
        plate: PlateOptions,
        json: bool,
        fill: bool,
        fix_all: bool,
        add_supports: bool,
    },
    Slice {
        source: OsString,
        target: OsString,
        target_format: Option<TargetFormat>,
        plate: PlateOptions,
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

    // Flag fallbacks come from the shared default pass set, so the `fix`
    // defaults, `slice --repair`, and any other frontend cannot drift apart.
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

    let fix_collinear_faces = long("fix-collinear-faces")
        .help("Removes faces with collinear vertices")
        .argument::<bool>("FIX_COLLINEAR_FACES")
        .adjacent()
        .fallback(RepairPasses::default_repair().collinear_faces);

    let fix_non_manifold_edges = long("fix-non-manifold-edges")
        .help(
            "Separates parts that touch at an edge or a vertex (edges shared by more than 2 faces)",
        )
        .argument::<bool>("FIX_NON_MANIFOLD_EDGES")
        .adjacent()
        .fallback(RepairPasses::default_repair().non_manifold_edges);

    let fix_holes = long("fix-holes")
        .help("Welds open boundaries and fills holes")
        .argument::<bool>("FIX_HOLES")
        .adjacent()
        .fallback(RepairPasses::default_repair().holes);

    construct!(CmdOptions::Fix {
        source,
        source_format,
        target,
        target_format,
        fix_zero_length_edges,
        fix_flipped_faces,
        fix_collinear_faces,
        fix_non_manifold_edges,
        fix_holes,
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

    let plate = get_plate_options();

    construct!(CmdOptions::Slice {
        source,
        target,
        target_format,
        plate,
    })
}

fn get_review_options() -> impl Parser<CmdOptions> {
    let source = short('s')
        .long("source")
        .help("Specify the path of the 3D model file to be sliced and reviewed")
        .argument::<OsString>("SOURCE_FILEPATH");

    let plate = get_plate_options();

    let json = long("json")
        .help("Print the findings and the summary as JSON instead of text.")
        .switch();

    let fill = long("fill")
        .help(
            "Fill every small resin trap the review offers to fill (a sealed trap under 1 ml), as the app does on the next slice, and review the plate again.",
        )
        .switch();

    let fix_all = long("fix-all")
        .help(
            "Do what a pass of the app's Fix All does, from the list of this review: place tips on the islands without support, fill the small resin traps, slice the plate again and review it once.",
        )
        .switch();

    let add_supports = long("add-supports")
        .help(
            "Answer Yes to the app's question after Slice: place tips on the islands without support, slice the plate again and review it.",
        )
        .switch();

    construct!(CmdOptions::Review {
        source,
        plate,
        json,
        fill,
        fix_all,
        add_supports,
    })
}

/// The options `slice` and `review` share, in the order `slice --help` lists
/// them.
fn get_plate_options() -> impl Parser<PlateOptions> {
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

    let adaptive_layers = long("adaptive-layers")
        .help(
            "Adaptive layer heights between MIN and MAX millimetres (e.g. 0.025:0.1): thin layers where the surface lies shallow, thick ones on steep walls. The bottom and transition layers keep the layer height. A printer file that keeps one layer height is sliced at a fixed height instead, with a warning.",
        )
        .argument::<AdaptiveRange>("MIN:MAX")
        .optional();

    let aa = long("aa")
        .help("Per-pixel anti-aliasing: off, x2, x4 or x8 (default: off)")
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
            "Skip auto-placement: slice the scene at its file coordinates instead of centring it on the build plate with its lowest point at z = 0. Multi-model scenes keep their relative layout either way. What then lies past the plate's edges or under it is not printed, and is warned about.",
        )
        .switch();

    let repair = long("repair")
        .help(
            "Repair the loaded mesh before slicing, using the same defaults as the `fix` subcommand (every repair pass enabled, holes and parts that touch at an edge included). Pre-existing problems that the enabled passes cannot address still propagate as a slicer validation error.",
        )
        .switch();

    let generate_supports = long("generate-supports")
        .help(
            "Generate supports and a raft before slicing, as the Supports tab does in the app. The supports are sliced with the scene.",
        )
        .switch();

    let support_lift = long("support-lift")
        // Built once per parse; the help names the generator's own default.
        .help(&*Box::leak(
            format!(
                "With --generate-supports and auto-placement: the height in millimetres the scene is raised off the plate to make room for the supports (default {}).",
                sc_geom::supports::DEFAULT_SUPPORT_LIFT_MM
            )
            .into_boxed_str(),
        ))
        .argument::<f64>("MM")
        .fallback(sc_geom::supports::DEFAULT_SUPPORT_LIFT_MM);

    construct!(PlateOptions {
        printer_profile,
        material_profile,
        layer_height,
        first_layer_height,
        adaptive_layers,
        aa,
        verbose,
        no_auto_place,
        repair,
        generate_supports,
        support_lift,
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
    let bodies = crate::commands::bodies::get_bodies_options()
        .to_options()
        .descr("List the bodies a 3d model splits into, with the time it takes")
        .command("bodies");
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
    let orient = crate::commands::orient::get_orient_options()
        .to_options()
        .descr("Find the best orientation of a 3d model for printing, with three options and their physical breakdown")
        .command("orient");
    let profiles = get_profiles_options()
        .to_options()
        .descr("List, export, and import profiles in the user library")
        .command("profiles");
    let review = get_review_options()
        .to_options()
        .descr("Slice a given 3d model file without writing it, and list the pre-flight findings")
        .command("review");
    let slice = get_slice_options()
        .to_options()
        .descr("Slice a given 3d model file")
        .command("slice");
    let version = construct!(CmdOptions::Version {})
        .to_options()
        .descr("Prints version information")
        .command("version");

    construct!([
        bodies,
        convert,
        diagnose,
        fix,
        help,
        inspect,
        orient,
        profiles,
        review,
        slice,
        version
    ])
        .to_options()
        .descr("sc-slicer-cli (Breadslicer, CLI) is a free software command line slicer for resin 3D printers")
        // .header(header) // TODO: Implement these lines for the embedded help
        .footer("Add `--help` after any of the subcommands to learn more about them.")
        .fallback_to_usage()
        .version(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod adaptive_range_tests {
    use super::*;

    #[test]
    fn the_range_reads_as_min_colon_max() {
        let range: AdaptiveRange = "0.025:0.1".parse().unwrap();
        assert_eq!(range.min_mm.value(), 0.025);
        assert_eq!(range.max_mm.value(), 0.1);
        for wrong in ["0.1", "0.1:0.025", "0:0.1", "a:0.1", "0.025:"] {
            assert!(wrong.parse::<AdaptiveRange>().is_err(), "{wrong}");
        }
    }
}
