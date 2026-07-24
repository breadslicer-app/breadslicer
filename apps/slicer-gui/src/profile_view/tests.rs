/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fs;

use sc_print_primitives::{BuildVolume, PrinterId};
use sc_print_profiles::load_material_profile_from_path;
use slint::Model;

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
bottom_exposure_sec = 30.0

[per_printer.generic-machine]
normal_exposure_sec = 3.0

[per_printer.printer-z]
normal_exposure_sec = 2.8
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

fn printer(id: &str) -> PrinterId {
    PrinterId::new(id).unwrap()
}

fn titles(sections: &[ProfileViewSection]) -> Vec<String> {
    sections.iter().map(|s| s.title.to_string()).collect()
}

fn fields(section: &ProfileViewSection) -> Vec<(String, String)> {
    section
        .fields
        .iter()
        .map(|f| (f.label.to_string(), f.value.to_string()))
        .collect()
}

#[test]
fn sections_are_ordered_identity_intent_defaults_then_blocks() {
    let profile = parse(BASE_TOML);
    let sections = build_profile_view_model(
        &profile,
        None,
        &printers(),
        Some(&printer("printer-a")),
        ProfileSource::Bundled,
    );
    assert_eq!(
        titles(&sections),
        vec![
            "Identity",
            "Intent",
            "Defaults",
            "Calibration: Printer A",
            "Generic calibration",
            "Calibration: Printer Z",
        ],
    );
}

#[test]
fn the_default_printers_block_is_first_and_highlighted() {
    let profile = parse(BASE_TOML);
    let sections = build_profile_view_model(
        &profile,
        None,
        &printers(),
        Some(&printer("printer-a")),
        ProfileSource::Bundled,
    );
    let highlighted: Vec<&str> = sections
        .iter()
        .filter(|s| s.highlighted)
        .map(|s| s.title.as_str())
        .collect();
    assert_eq!(highlighted, vec!["Calibration: Printer A"]);
}

#[test]
fn the_generic_block_is_highlighted_when_it_is_the_fallback() {
    let profile = parse(BASE_TOML);
    // printer-b has no block of its own; the generic block is what it
    // would print with.
    let sections = build_profile_view_model(
        &profile,
        None,
        &printers(),
        Some(&printer("printer-b")),
        ProfileSource::Bundled,
    );
    let highlighted: Vec<&str> = sections
        .iter()
        .filter(|s| s.highlighted)
        .map(|s| s.title.as_str())
        .collect();
    assert_eq!(highlighted, vec!["Generic calibration"]);
}

#[test]
fn nothing_is_highlighted_without_a_default_printer_or_matching_block() {
    let profile = parse(BASE_TOML);
    let none_selected =
        build_profile_view_model(&profile, None, &printers(), None, ProfileSource::Bundled);
    assert!(none_selected.iter().all(|s| !s.highlighted));

    let mut no_generic = parse(BASE_TOML);
    no_generic.per_printer.remove(&printer("generic-machine"));
    let unmatched = build_profile_view_model(
        &no_generic,
        None,
        &printers(),
        Some(&printer("printer-b")),
        ProfileSource::Bundled,
    );
    assert!(unmatched.iter().all(|s| !s.highlighted));
}

#[test]
fn extension_blocks_are_included_in_display_name_order() {
    let profile = parse(BASE_TOML);
    let extension = parse(EXTENSION_TOML);
    let sections = build_profile_view_model(
        &profile,
        Some(&extension),
        &printers(),
        Some(&printer("printer-a")),
        ProfileSource::Bundled,
    );
    assert_eq!(
        titles(&sections)[3..],
        vec![
            "Calibration: Printer A".to_owned(),
            "Generic calibration".to_owned(),
            "Calibration: Printer B".to_owned(),
            "Calibration: Printer Z".to_owned(),
        ],
    );
}

#[test]
fn unset_optional_fields_render_the_not_set_text() {
    let profile = parse(BASE_TOML);
    let sections = build_profile_view_model(
        &profile,
        None,
        &printers(),
        Some(&printer("printer-a")),
        ProfileSource::Bundled,
    );

    // Intent: energy density unset, layer height set.
    let intent = fields(&sections[1]);
    assert_eq!(intent[0].1, "not set");
    assert_eq!(intent[1].1, "0.05");

    // Defaults: scale compensation unset, bottom layer count set.
    let defaults = fields(&sections[2]);
    assert_eq!(defaults[0].1, "not set");
    assert_eq!(defaults[1].1, "5");

    // The default printer's block: two set values, four unset, all
    // six rendered.
    let block = fields(&sections[3]);
    assert_eq!(block.len(), 6);
    assert_eq!(block[0].1, "2.5");
    assert_eq!(block[1].1, "30");
    assert_eq!(block[2].1, "not set");
    assert_eq!(block[3].1, "not set");
    assert_eq!(block[4].1, "not set");
    assert_eq!(block[5].1, "not set");
}

#[test]
fn identity_section_carries_the_origin() {
    let profile = parse(BASE_TOML);
    let bundled =
        build_profile_view_model(&profile, None, &printers(), None, ProfileSource::Bundled);
    let custom = build_profile_view_model(&profile, None, &printers(), None, ProfileSource::User);
    let origin = |sections: &[ProfileViewSection]| -> String {
        fields(&sections[0])
            .into_iter()
            .find(|(label, _)| label == "Origin")
            .map(|(_, value)| value)
            .unwrap()
    };
    assert_eq!(origin(&bundled), "Bundled");
    assert_eq!(origin(&custom), "Custom");
    assert_eq!(
        fields(&bundled[0]),
        vec![
            ("Brand".to_owned(), "Brand".to_owned()),
            ("Name".to_owned(), "Resin".to_owned()),
            ("Family".to_owned(), "standard".to_owned()),
            ("Profile version".to_owned(), "1.2.0".to_owned()),
            ("Origin".to_owned(), "Bundled".to_owned()),
        ],
    );
}

#[test]
fn a_block_for_an_unknown_printer_falls_back_to_its_id() {
    let profile = parse(BASE_TOML);
    let sections = build_profile_view_model(
        &profile,
        None,
        &[],
        Some(&printer("printer-a")),
        ProfileSource::Bundled,
    );
    assert!(
        sections
            .iter()
            .any(|s| s.title.as_str() == "Calibration: printer-a"),
        "titles: {:?}",
        titles(&sections),
    );
}

#[test]
fn display_name_is_brand_plus_name() {
    let profile = parse(BASE_TOML);
    assert_eq!(profile_display_name(&profile), "Brand Resin");
}
