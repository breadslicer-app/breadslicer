/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::fs;
use std::path::{Path, PathBuf};

use sc_print_profiles::{
    load_material_profile_from_path, load_printer_profile_from_path, save_material_profile_to_path,
    save_printer_profile_to_path,
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
