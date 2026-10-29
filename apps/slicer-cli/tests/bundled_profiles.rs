/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::fs;
use std::path::{Path, PathBuf};

use sc_print_primitives::{FormatKind, PanelRotation, PrinterId};
use sc_print_profiles::{
    CalibrationTarget, EffectiveMaterial, PrinterProfile, SceneOverrides,
    load_material_profile_from_path, load_printer_profile_from_path, resolve,
    save_material_profile_to_path, save_printer_profile_to_path,
};
use sc_sla_formats::PrinterFileFormat;

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

/// Pinned because the dataset the Anycubic profiles were first converted
/// from stated no mirror for them, while UVtools' printer data mirrors every
/// Anycubic machine in X and Photon Workshop's own M7 Pro file needs it (#142,
/// #195): text printed from an unmirrored file reads backwards.
#[test]
fn every_anycubic_printer_mirrors_x_and_not_y() {
    for printer in bundled_printers() {
        if printer.identity.manufacturer.as_str() != "Anycubic" {
            continue;
        }
        let mirror = printer.hardware.mirror();
        assert!(
            mirror.x && !mirror.y,
            "{}: Anycubic machines mirror in X only, this one mirrors x={} y={}",
            printer.identity.id.as_str(),
            mirror.x,
            mirror.y,
        );
    }
}

/// The Anycubic panels the first dataset laid out the other way round, or at
/// another resolution, as UVtools' printer data lists them: pixels, then
/// millimetres (#195).
const ANYCUBIC_PANELS_AS_UVTOOLS_LISTS_THEM: [(&str, (u32, u32), (f64, f64)); 6] = [
    ("anycubic-photon-s", (1440, 2560), (68.04, 120.96)),
    ("anycubic-photon-zero", (480, 854), (55.4, 98.63)),
    ("anycubic-photon-mono", (1620, 2560), (82.62, 130.56)),
    ("anycubic-photon-mono-se", (1620, 2560), (82.62, 130.56)),
    ("anycubic-photon-mono-sq", (2400, 2560), (120.0, 128.0)),
    ("anycubic-photon-m3-premium", (7680, 4320), (218.88, 123.12)),
];

#[test]
fn the_anycubic_panels_lie_as_uvtools_lists_them() {
    for (id, (x, y), (x_mm, y_mm)) in ANYCUBIC_PANELS_AS_UVTOOLS_LISTS_THEM {
        let hardware = bundled_printer(id).hardware;
        let pixels = hardware.pixel_resolution();
        assert_eq!((pixels.x(), pixels.y()), (x, y), "{id}");
        let plate = hardware.build_volume();
        assert_eq!((plate.x_mm(), plate.y_mm()), (x_mm, y_mm), "{id}");
        assert_eq!(hardware.panel_rotation(), PanelRotation::None, "{id}");
    }
}

/// Every binding of a family this workspace knows names a printer file, so a
/// regenerated profile cannot ask for a revision nothing can identify. The
/// families nothing here identifies yet (UVtools' NanoDLP, SVGX, CWS, ...)
/// are the printer's own business.
#[test]
fn every_known_format_a_printer_declares_names_a_printer_file() {
    for printer in bundled_printers() {
        for binding in printer.capabilities.output_formats.iter() {
            if !matches!(
                binding.format,
                FormatKind::Goo
                    | FormatKind::Ctb
                    | FormatKind::Cxdlp
                    | FormatKind::Azf
                    | FormatKind::Aff
            ) {
                continue;
            }
            assert!(
                PrinterFileFormat::from_binding(binding).is_ok(),
                "{}: {binding} names no printer file",
                printer.identity.id.as_str(),
            );
        }
    }
}

/// The printers no photo can be shipped for: the image sources the
/// library vendors (see `images/README.md`) have none of them, so their
/// profiles name no image, as the generic machine does.
const PRINTERS_WITHOUT_A_SHIPPABLE_PHOTO: [&str; 3] = [
    "creality-halot-one",
    "creality-halot-one-plus",
    "creality-halot-one-pro",
];

#[test]
fn every_image_a_bundled_printer_names_is_shipped() {
    let root = bundled_profiles_dir();
    for printer in bundled_printers() {
        let id = printer.identity.id.as_str().to_owned();
        if PRINTERS_WITHOUT_A_SHIPPABLE_PHOTO.contains(&id.as_str()) {
            assert!(
                printer.identity.image_asset_path.is_none(),
                "{id}: a photo nobody may ship must not be named",
            );
            continue;
        }
        if let Some(image) = printer.identity.image_asset_path {
            assert!(
                image.resolve_in(&root).is_file(),
                "{id}: \"{}\" is not a file in the library",
                image.as_str(),
            );
        }
    }
}

/// Every photo the library ships is shown by the printer it is named for.
///
/// Pinned because most printers come from UVtools' printer catalog, which
/// ships no photo, so a regeneration that dropped a profile's image would
/// leave its machine without one and nothing would say so.
#[test]
fn every_shipped_photo_is_named_by_its_printer() {
    let images = bundled_profiles_dir().join("images");
    let mut photos = 0;
    for entry in fs::read_dir(&images).unwrap() {
        let path = entry.unwrap().path();
        if !path.extension().is_some_and(|ext| ext == "png") {
            continue;
        }
        let file = path.file_name().unwrap().to_str().unwrap().to_owned();
        let id = file.trim_end_matches(".png");
        let named = bundled_printer(id).identity.image_asset_path;
        assert_eq!(
            named.as_ref().map(|image| image.as_str()),
            Some(format!("images/{file}").as_str()),
            "{id} must show the photo shipped for it",
        );
        photos += 1;
    }
    assert!(photos > 0, "no photo found under {}", images.display());
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

/// The Photon Mono M7 family's plate in millimetres and its X mirror,
/// as UVtools' printer data states them and its Anycubic writers are
/// validated against.
///
/// Pinned here because the catalogue the converter reads stated a
/// 126.48 mm deep M7 / M7 Pro plate (its own 24.8 µm pixel says
/// 126.976), a 297.5 x 164 mm M7 Max and no mirror for the M7 as of
/// 2026-10-27, and a mirrored or misscaled part is a failure nothing
/// in the chain reports. For the M7 Pro both were checked against files
/// that print correctly: Lychee rasterises it at 24.806 µm in Y (measured
/// against its 50 µm Halot One file of the same scene, although its header
/// says 126.48 mm), and Photon Workshop's `.pwsz` matches ours unmirrored
/// only with mirror X on (`tests/printer_orientation.rs`).
const M7_FAMILY_PLATES: [(&str, f64, f64); 3] = [
    ("anycubic-photon-mono-m7", 223.642, 126.976),
    ("anycubic-photon-mono-m7-pro", 223.642, 126.976),
    ("anycubic-photon-mono-m7-max", 298.08, 165.6),
];

/// The M7 Pro runs the 2 stage move and the 6 transition layers that the
/// Lychee files its owner prints from carry.
#[test]
fn the_m7_pro_runs_the_two_stage_move_and_transition_layers_lychee_writes() {
    let capabilities = bundled_printer("anycubic-photon-mono-m7-pro").capabilities;
    assert!(capabilities.two_stage_lift);
    assert_eq!(capabilities.transition_layers, Some(6));
}

/// The Halot One family writes CXDLP v2 by default, the dialect of the
/// Lychee files the Halot One prints from; v3 stays selectable.
#[test]
fn the_halot_family_lists_cxdlp_v2_first() {
    for id in [
        "creality-halot-one",
        "creality-halot-one-pro",
        "creality-halot-one-plus",
    ] {
        let formats: Vec<String> = bundled_printer(id)
            .capabilities
            .output_formats
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(formats, ["cxdlp@v2", "cxdlp@v3"], "{id}");
    }
}

/// The Halot One's plate is 128 mm wide and 81 mm deep, as the machine
/// stands and as Lychee lays it out; its files keep the panel's 1620 x 2560
/// pixels, so the plate is stored turned a quarter turn.
#[test]
fn the_halot_one_plate_runs_wide_and_its_panel_turns() {
    let hardware = bundled_printer("creality-halot-one").hardware;
    let plate = hardware.build_volume();
    assert_eq!((plate.x_mm(), plate.y_mm()), (128.0, 81.0));
    let pixels = hardware.pixel_resolution();
    assert_eq!((pixels.x(), pixels.y()), (2560, 1620));
    assert_eq!(hardware.panel_rotation(), PanelRotation::Ccw90);
    let mirror = hardware.mirror();
    assert!(!mirror.x && !mirror.y, "the Halot One mirrors neither axis");
}

#[test]
fn the_m7_family_keeps_uvtools_plate_and_mirrors_x() {
    for (id, x_mm, y_mm) in M7_FAMILY_PLATES {
        let hardware = bundled_printer(id).hardware;
        let plate = hardware.build_volume();
        assert_eq!((plate.x_mm(), plate.y_mm()), (x_mm, y_mm), "{id}");
        let mirror = hardware.mirror();
        assert!(mirror.x && !mirror.y, "{id} mirrors in X only");
    }
}
