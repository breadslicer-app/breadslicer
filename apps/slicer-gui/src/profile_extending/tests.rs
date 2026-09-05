/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fs;

use sc_print_primitives::BuildVolume;
use sc_print_profiles::load_material_profile_from_path;

use crate::profile_catalog::ProfileSource;

use super::*;

const BASE_TOML: &str = "\
[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"standard\"
name = \"Resin\"
profile_version = \"1.2.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[per_printer.printer-a]
normal_exposure_sec = 2.5
";

const EXTENSION_TOML: &str = "\
extension = true

[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"standard\"
name = \"Resin\"
profile_version = \"1.2.0\"

[intent]

[defaults]

[per_printer.printer-b]
normal_exposure_sec = 2.2
";

fn parse(toml: &str) -> MaterialProfile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.toml");
    fs::write(&path, toml).unwrap();
    load_material_profile_from_path(&path).unwrap()
}

/// Compose a base and an optional extension, asserting the pair is
/// clean.
fn effective(base: MaterialProfile, extension: Option<MaterialProfile>) -> EffectiveMaterial {
    let (material, issues) = EffectiveMaterial::compose(base, extension);
    assert!(issues.is_empty(), "issues: {issues:?}");
    material
}

fn printer_entry(id: &str, name: &str) -> PrinterCatalogEntry {
    PrinterCatalogEntry {
        id: PrinterId::new(id).unwrap(),
        manufacturer: "Maker".to_owned(),
        name: name.to_owned(),
        build_volume: BuildVolume::new(100.0, 60.0, 80.0).unwrap(),
        image_path: None,
        source: ProfileSource::Bundled,
    }
}

fn printers() -> Vec<PrinterCatalogEntry> {
    vec![
        printer_entry("printer-a", "Printer A"),
        printer_entry("printer-b", "Printer B"),
        printer_entry("printer-z", "Printer Z"),
        printer_entry("generic-machine", "Generic Machine"),
    ]
}

fn printer_id(id: &str) -> PrinterId {
    PrinterId::new(id).unwrap()
}

fn target(id: &str) -> CalibrationTarget {
    CalibrationTarget::from(printer_id(id))
}

fn sample_block() -> MaterialPerPrinter {
    MaterialPerPrinter {
        normal_exposure_sec: None,
        bottom_exposure_sec: None,
        bottom_layer_count: Some(4),
        wait_before_cure_sec: None,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: None,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: None,
        lift_speed_mm_min: None,
        retract_speed_mm_min: None,
    }
}

#[test]
fn covered_targets_are_excluded_from_the_selectable_list() {
    let material = effective(parse(BASE_TOML), Some(parse(EXTENSION_TOML)));
    let targets = uncovered_targets(&material, &printers(), None);

    let ids: Vec<&str> = targets.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["generic-machine", "printer-z"]);
}

#[test]
fn without_an_extension_only_the_base_blocks_cover() {
    let material = effective(parse(BASE_TOML), None);
    let targets = uncovered_targets(&material, &printers(), None);

    let ids: Vec<&str> = targets.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["generic-machine", "printer-b", "printer-z"]);
}

#[test]
fn the_default_printer_ranks_before_generic_and_the_rest() {
    let material = effective(parse(BASE_TOML), None);
    let default = printer_id("printer-z");
    let targets = uncovered_targets(&material, &printers(), Some(&default));

    let ids: Vec<&str> = targets.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["printer-z", "generic-machine", "printer-b"]);
}

#[test]
fn the_generic_target_gets_the_translated_label() {
    let material = effective(parse(BASE_TOML), None);
    let targets = uncovered_targets(&material, &printers(), None);

    let generic = targets
        .iter()
        .find(|(id, _)| id.as_str() == "generic-machine")
        .unwrap();
    assert_eq!(generic.1.as_str(), "Generic");
    let named = targets
        .iter()
        .find(|(id, _)| id.as_str() == "printer-b")
        .unwrap();
    assert_eq!(named.1.as_str(), "Printer B");
}

#[test]
fn full_coverage_leaves_nothing_to_extend() {
    let material = effective(parse(BASE_TOML), None);
    let subset = vec![printer_entry("printer-a", "Printer A")];
    assert!(uncovered_targets(&material, &subset, None).is_empty());
}

#[test]
fn the_first_extend_starts_a_fresh_extension_from_the_base_identity() {
    let base = parse(BASE_TOML);
    let new_target = target("printer-z");
    let update = build_extension_update(&base, None, &new_target, sample_block());

    assert!(update.extension);
    assert_eq!(update.identity, base.identity);
    assert_eq!(update.intent.target_energy_density_mj_per_cm2, None);
    assert_eq!(update.intent.target_layer_height_mm, None);
    assert_eq!(update.defaults.scale_compensation_pct, None);
    assert_eq!(update.defaults.bottom_layer_count, None);
    assert_eq!(update.per_printer.len(), 1);
    assert_eq!(update.per_printer.get(&new_target), Some(&sample_block()));
}

#[test]
fn a_later_extend_appends_to_the_existing_extension() {
    let base = parse(BASE_TOML);
    let extension = parse(EXTENSION_TOML);
    let new_target = target("printer-z");
    let update = build_extension_update(&base, Some(&extension), &new_target, sample_block());

    assert!(update.extension);
    assert_eq!(update.per_printer.len(), 2);
    assert_eq!(
        update.per_printer.get(&target("printer-b")),
        extension.per_printer.get(&target("printer-b")),
    );
    assert_eq!(update.per_printer.get(&new_target), Some(&sample_block()));
}

#[test]
fn extending_an_orphaned_extension_appends_to_its_own_pairs() {
    let orphan = parse(EXTENSION_TOML);
    let new_target = target("printer-z");
    let update = build_extension_update(&orphan, None, &new_target, sample_block());

    assert!(update.extension);
    assert_eq!(update.per_printer.len(), 2);
    assert_eq!(
        update.per_printer.get(&target("printer-b")),
        orphan.per_printer.get(&target("printer-b")),
    );
    assert_eq!(update.per_printer.get(&new_target), Some(&sample_block()));
}
