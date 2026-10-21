/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Command-line surface of the profiles converter.
//!
//! `--format` selects the source format adapter (currently only
//! `dragonfruit`). `--input` (repeatable) is an input file or a
//! directory of them, interpreted according to the chosen format.
//! `--out` is the destination profile-library root (the directory that
//! gains `printers/`, `materials/`, and, when image copying is on,
//! `images/`). Every argument is named; validation of the parsed values
//! lives in [`crate::request`].

use std::path::PathBuf;

use bpaf::{OptionParser, Parser, construct, long, short};

use crate::format::SourceFormat;

/// The raw, unvalidated command-line values. Turning this into a
/// runnable request (which enforces that at least one input is given)
/// is [`crate::request::ConversionRequest::from_raw`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCliArgs {
    pub format: SourceFormat,
    pub inputs: Vec<PathBuf>,
    pub out: PathBuf,
    pub with_images: bool,
    pub printer_library: Option<PathBuf>,
    pub material_curation: Option<PathBuf>,
}

/// Build the parser for the converter's command line.
pub fn cli_parser() -> OptionParser<RawCliArgs> {
    let format = short('f')
        .long("format")
        .help("Source format to read. Supported: dragonfruit.")
        .argument::<SourceFormat>("FORMAT");
    let inputs = short('i')
        .long("input")
        .help(
            "Input file, or a directory of them; repeatable. Interpreted according to the chosen `--format`.",
        )
        .argument::<PathBuf>("PATH")
        .many();
    let out = short('o')
        .long("out")
        .help("Destination profile-library root that gains `printers/` and `materials/`.")
        .argument::<PathBuf>("DIR");
    let with_images = long("with-images")
        .help("Copy referenced preview images into `<out>/images/` and wire the profiles to them.")
        .switch();
    let printer_library = long("printer-library")
        .help(
            "Profile library root whose printer profiles say what each machine can do. Without it, no calibration block carries a second travel stage.",
        )
        .argument::<PathBuf>("DIR")
        .optional();
    let material_curation = long("material-curation")
        .help(
            "File stating what we know about a resin that no source publishes, such as its densities. Without it, those fields are left empty.",
        )
        .argument::<PathBuf>("FILE")
        .optional();

    construct!(RawCliArgs {
        format,
        inputs,
        out,
        with_images,
        printer_library,
        material_curation,
    })
    .to_options()
    .descr(
        "Convert third-party printer and material profiles into the SoulCrafted Slicer TOML format",
    )
    .version(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<RawCliArgs, String> {
        cli_parser()
            .run_inner(args)
            .map_err(|err| format!("{err:?}"))
    }

    #[test]
    fn parses_a_single_input_with_output() {
        let parsed = parse(&[
            "--format",
            "dragonfruit",
            "--input",
            "/src/a.json",
            "--out",
            "/dst",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            RawCliArgs {
                format: SourceFormat::DragonFruit,
                inputs: vec![PathBuf::from("/src/a.json")],
                out: PathBuf::from("/dst"),
                with_images: false,
                printer_library: None,
                material_curation: None,
            },
        );
    }

    #[test]
    fn parses_short_flags_many_inputs_and_the_images_switch() {
        let parsed = parse(&[
            "-f",
            "dragonfruit",
            "-i",
            "/src/a.json",
            "-i",
            "/src/dir",
            "--with-images",
            "--printer-library",
            "/lib",
            "--material-curation",
            "/lib/materials.toml",
            "-o",
            "/dst",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            RawCliArgs {
                format: SourceFormat::DragonFruit,
                inputs: vec![PathBuf::from("/src/a.json"), PathBuf::from("/src/dir")],
                out: PathBuf::from("/dst"),
                with_images: true,
                printer_library: Some(PathBuf::from("/lib")),
                material_curation: Some(PathBuf::from("/lib/materials.toml")),
            },
        );
    }

    #[test]
    fn format_is_required() {
        assert!(parse(&["--input", "/src/a.json", "--out", "/dst"]).is_err());
    }

    #[test]
    fn out_is_required() {
        assert!(parse(&["--format", "dragonfruit", "--input", "/src/a.json"]).is_err());
    }

    #[test]
    fn an_unknown_format_is_rejected() {
        assert!(
            parse(&[
                "--format",
                "lychee",
                "--input",
                "/src/a.json",
                "--out",
                "/dst"
            ])
            .is_err()
        );
    }

    #[test]
    fn no_inputs_parses_to_an_empty_list() {
        // Emptiness is rejected later, by ConversionRequest::from_raw.
        let parsed = parse(&["--format", "dragonfruit", "--out", "/dst"]).unwrap();
        assert!(parsed.inputs.is_empty());
    }
}
