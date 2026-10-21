/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::fs;
use std::path::{Path, PathBuf};

use sc_print_primitives::PrinterId;
use sc_print_profiles::{
    CalibrationTarget, EffectiveMaterial, PrinterProfile, SceneOverrides,
    load_material_profile_from_path, load_printer_profile_from_path, resolve,
    save_material_profile_to_path, save_printer_profile_to_path,
};

fn bundled_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("profiles")
}

/// The `.toml` files directly under `dir`, sorted, so failures are
/// reported in a stable order.
fn toml_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("cannot list {}: {err}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no profiles found under {}",
        dir.display()
    );
    files
}

#[test]
fn every_bundled_printer_loads_and_re_saves_byte_identically() {
    let scratch = tempfile::tempdir().unwrap();
    for path in toml_files(&bundled_profiles_dir().join("printers")) {
        let profile = load_printer_profile_from_path(&path)
            .unwrap_or_else(|err| panic!("bundled printer failed to load: {err}"));

        let re_saved = scratch.path().join(path.file_name().unwrap());
        save_printer_profile_to_path(&re_saved, &profile).unwrap();
        assert_eq!(
            fs::read_to_string(&re_saved).unwrap(),
            fs::read_to_string(&path).unwrap(),
            "{} is not in the writer's dialect",
            path.display()
        );
    }
}

#[test]
fn every_bundled_material_loads_and_re_saves_byte_identically() {
    let scratch = tempfile::tempdir().unwrap();
    for path in toml_files(&bundled_profiles_dir().join("materials")) {
        let profile = load_material_profile_from_path(&path)
            .unwrap_or_else(|err| panic!("bundled material failed to load: {err}"));

        let re_saved = scratch.path().join(path.file_name().unwrap());
        save_material_profile_to_path(&re_saved, &profile).unwrap();
        assert_eq!(
            fs::read_to_string(&re_saved).unwrap(),
            fs::read_to_string(&path).unwrap(),
            "{} is not in the writer's dialect",
            path.display()
        );
    }
}

/// Widest span the published photopolymer densities occupy, with room
/// either side: the lightest sheet read was 1.06 and the heaviest 1.6,
/// both cured figures included.
const PLAUSIBLE_DENSITY_G_PER_CM3: std::ops::RangeInclusive<f64> = 0.9..=2.0;

#[test]
fn every_stated_bundled_density_is_a_plausible_resin_density() {
    for path in toml_files(&bundled_profiles_dir().join("materials")) {
        let profile = load_material_profile_from_path(&path).unwrap();
        for (label, density) in [
            ("liquid", profile.physical.liquid_density_g_per_cm3),
            ("cured", profile.physical.cured_density_g_per_cm3),
        ] {
            let Some(density) = density else { continue };
            assert!(
                PLAUSIBLE_DENSITY_G_PER_CM3.contains(&density.value()),
                "{}: {label} density {} is outside {PLAUSIBLE_DENSITY_G_PER_CM3:?}",
                path.display(),
                density.value(),
            );
        }
    }
}

#[test]
fn no_bundled_resin_claims_to_expand_as_it_cures() {
    for path in toml_files(&bundled_profiles_dir().join("materials")) {
        let profile = load_material_profile_from_path(&path).unwrap();
        let (Some(liquid), Some(cured)) = (
            profile.physical.liquid_density_g_per_cm3,
            profile.physical.cured_density_g_per_cm3,
        ) else {
            continue;
        };
        assert!(
            cured.value() >= liquid.value(),
            "{}: cured {} is lighter than liquid {}",
            path.display(),
            cured.value(),
            liquid.value(),
        );
    }
}

/// Load every bundled printer profile, so a caller can match against
/// the id each one declares rather than against its file name: the two
/// need not agree, and calibration blocks are keyed by the id.
fn bundled_printers() -> Vec<PrinterProfile> {
    toml_files(&bundled_profiles_dir().join("printers"))
        .iter()
        .map(|path| {
            load_printer_profile_from_path(path)
                .unwrap_or_else(|err| panic!("bundled printer failed to load: {err}"))
        })
        .collect()
}

#[test]
fn every_bundled_calibration_block_names_a_printer_we_ship() {
    let known: Vec<PrinterId> = bundled_printers()
        .into_iter()
        .map(|printer| printer.identity.id)
        .collect();

    for path in toml_files(&bundled_profiles_dir().join("materials")) {
        let profile = load_material_profile_from_path(&path).unwrap();
        for target in profile.per_printer.keys() {
            if *target == CalibrationTarget::Generic {
                continue;
            }
            assert!(
                known.contains(target.printer_id()),
                "{}: calibration block \"{target}\" names no bundled printer",
                path.display(),
            );
        }
    }
}

#[test]
fn every_bundled_resin_resolves_on_the_printers_it_is_calibrated_for() {
    let printers = bundled_printers();

    for path in toml_files(&bundled_profiles_dir().join("materials")) {
        let profile = load_material_profile_from_path(&path).unwrap();
        let targets: Vec<CalibrationTarget> = profile.per_printer.keys().cloned().collect();
        let material = EffectiveMaterial::from(profile);

        for target in targets {
            let Some(printer) = printers
                .iter()
                .find(|printer| printer.identity.id == *target.printer_id())
            else {
                continue;
            };
            resolve(printer, &material, &SceneOverrides::default()).unwrap_or_else(|err| {
                panic!(
                    "{} on {}: {err}",
                    path.display(),
                    printer.identity.id.as_str(),
                )
            });
        }
    }
}

/// The slug a machine's own manufacturer and model make, which is what
/// the converter derives a printer id from.
fn slug_of(manufacturer: &str, name: &str) -> String {
    format!("{manufacturer} {name}")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

/// The ELEGOO machines whose own manufacturer publishes a 2 stage
/// move for them, in the resin settings it hands its customers.
///
/// Listed here because the converter that writes these profiles has
/// no manufacturer data for the capability and defaults it to false,
/// so a re-import would silently take it away again.
const ELEGOO_PRINTERS_THAT_SPLIT_THEIR_TRAVEL: [&str; 11] = [
    "elegoo-jupiter",
    "elegoo-jupiter-se",
    "elegoo-mars-4",
    "elegoo-mars-4-dlp",
    "elegoo-mars-4-max",
    "elegoo-mars-4-ultra",
    "elegoo-mars-5",
    "elegoo-saturn-2",
    "elegoo-saturn-3",
    "elegoo-saturn-3-ultra",
    "elegoo-saturn-4",
];

/// The machines that release the part by tilting the vat, which have
/// no lift for a second stage to extend.
const ELEGOO_TILTING_VAT_PRINTERS: [&str; 3] = [
    "elegoo-mars-5-ultra",
    "elegoo-saturn-4-ultra",
    "elegoo-saturn-4-ultra-16k",
];

#[test]
fn every_tilting_vat_printer_declares_no_split_travel() {
    let printers = bundled_printers();
    for id in ELEGOO_TILTING_VAT_PRINTERS {
        let printer = printers
            .iter()
            .find(|printer| printer.identity.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} is a printer we ship"));
        assert!(
            !printer.capabilities.two_stage_lift,
            "{id} tilts its vat instead of lifting, so it has no second stage",
        );
    }
}

#[test]
fn every_printer_the_vendor_splits_the_travel_for_declares_it() {
    let printers = bundled_printers();
    for id in ELEGOO_PRINTERS_THAT_SPLIT_THEIR_TRAVEL {
        let printer = printers
            .iter()
            .find(|printer| printer.identity.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} is a printer we ship"));
        assert!(
            printer.capabilities.two_stage_lift,
            "{id} runs the 2 stage move its own manufacturer publishes",
        );
    }
}

/// The one bundled printer with the given id.
fn bundled_printer(id: &str) -> PrinterProfile {
    bundled_printers()
        .into_iter()
        .find(|printer| printer.identity.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is a printer we ship"))
}

/// The Mars 5's build height in millimetres, as ELEGOO publishes it.
///
/// Pinned here because the catalogue the converter reads stated 165 as
/// of 2026-09-10, which is the Mars 5 Ultra's figure, and a ceiling
/// that high accepts scenes the machine cannot print.
const MARS_5_BUILD_HEIGHT_MM: f64 = 150.0;

#[test]
fn the_mars_5_builds_no_taller_than_elegoo_publishes() {
    assert_eq!(
        bundled_printer("elegoo-mars-5")
            .hardware
            .build_volume()
            .z_mm(),
        MARS_5_BUILD_HEIGHT_MM,
    );
}

/// Every format the Mars 5 Ultra takes, in the order its profile
/// lists them.
///
/// Pinned here because the catalogue the converter reads stated a
/// single format per machine as of 2026-09-10, so a re-import would
/// drop whichever it does not name.
const MARS_5_ULTRA_OUTPUT_FORMATS: [&str; 2] = ["ctb", "goo"];

#[test]
fn the_mars_5_ultra_keeps_every_format_we_bind_it_to() {
    let ultra = bundled_printer("elegoo-mars-5-ultra");
    let declared: Vec<&str> = ultra
        .capabilities
        .output_formats
        .iter()
        .map(|binding| binding.format.as_canonical())
        .collect();

    assert_eq!(declared, MARS_5_ULTRA_OUTPUT_FORMATS);
}

/// Pinned because the catalogue the converter reads stated
/// `mirrorX: false` for the Mars 2 Pro as of 2026-09-10, alone among
/// the ELEGOO machines it describes, and a part that comes out
/// mirrored is a failure nothing in the chain reports.
#[test]
fn every_elegoo_printer_mirrors_x_and_not_y() {
    for printer in bundled_printers() {
        if printer.identity.manufacturer.as_str() != "ELEGOO" {
            continue;
        }
        let mirror = printer.hardware.mirror();
        assert!(
            mirror.x && !mirror.y,
            "{}: ELEGOO machines mirror in X only, this one mirrors x={} y={}",
            printer.identity.id.as_str(),
            mirror.x,
            mirror.y,
        );
    }
}

#[test]
fn every_bundled_printer_declares_an_image_we_ship() {
    let root = bundled_profiles_dir();
    for printer in bundled_printers() {
        let id = printer.identity.id.as_str().to_owned();
        // Our own stand-in, so there is no machine to photograph.
        if id == "generic-machine" {
            continue;
        }
        let image = printer
            .identity
            .image_asset_path
            .unwrap_or_else(|| panic!("{id} declares no image"));
        assert!(
            image.resolve_in(&root).is_file(),
            "{id}: \"{}\" is not a file in the library",
            image.as_str(),
        );
    }
}

#[test]
fn every_bundled_printer_id_is_the_machine_it_names() {
    for printer in bundled_printers() {
        let id = printer.identity.id.as_str();
        // The generic machine is our own stand-in rather than a
        // catalogued model, and its id is reserved in code.
        if id == "generic-machine" {
            continue;
        }
        assert_eq!(
            id,
            slug_of(
                printer.identity.manufacturer.as_str(),
                printer.identity.name.as_str(),
            ),
            "a printer id must be the machine it names and nothing else",
        );
    }
}
