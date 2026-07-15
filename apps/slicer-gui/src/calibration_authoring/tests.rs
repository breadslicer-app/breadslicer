/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use sc_print_profiles::load_material_profile_from_path;

use super::*;

const SOURCE_TOML: &str = "\
[identity]
id = \"source-resin\"
brand = \"Brand\"
family = \"tough\"
name = \"Source Resin\"
profile_version = \"2.3.4\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[per_printer]
";

fn source_profile() -> MaterialProfile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.toml");
    std::fs::write(&path, SOURCE_TOML).unwrap();
    load_material_profile_from_path(&path).unwrap()
}

fn material_id(id: &str) -> MaterialId {
    MaterialId::new(id).unwrap()
}

fn valid_input(name: &str) -> CalibrationInput {
    CalibrationInput {
        name: name.to_owned(),
        normal_exposure_sec: "2.5".to_owned(),
        bottom_exposure_sec: "30".to_owned(),
        bottom_layer_count: "6".to_owned(),
        lift_distance_mm: "6".to_owned(),
        lift_speed_mm_min: "60".to_owned(),
        retract_speed_mm_min: "150".to_owned(),
    }
}

#[test]
fn slugify_lowercases_and_collapses_separators() {
    assert_eq!(slugify("My Resin"), "my-resin");
    assert_eq!(slugify("  Fast!! Grey  "), "fast-grey");
    // Non-ASCII letters (here an accented `e`) are treated as
    // separators, so they collapse to a single hyphen.
    assert_eq!(slugify("caf\u{e9} 4k"), "caf-4k");
    assert_eq!(slugify("---"), "");
    assert_eq!(slugify(""), "");
}

#[test]
fn unique_material_id_suffixes_on_collision() {
    let existing: BTreeSet<MaterialId> = [material_id("my-resin"), material_id("my-resin-2")]
        .into_iter()
        .collect();
    assert_eq!(
        unique_material_id("my-resin", &existing).unwrap(),
        material_id("my-resin-3")
    );
    assert_eq!(
        unique_material_id("fresh", &existing).unwrap(),
        material_id("fresh")
    );
    assert_eq!(unique_material_id("", &existing), None);
}

#[test]
fn validate_input_rejects_an_empty_name() {
    let mut input = valid_input("   ");
    input.normal_exposure_sec = "2.5".to_owned();
    assert_eq!(
        validate_input(&input, &BTreeSet::new()),
        Err(AuthoringError::EmptyName)
    );
}

#[test]
fn validate_input_rejects_a_name_without_slug_characters() {
    let input = valid_input("!!!");
    assert_eq!(
        validate_input(&input, &BTreeSet::new()),
        Err(AuthoringError::NoAvailableId)
    );
}

#[test]
fn validate_input_rejects_a_non_numeric_field() {
    let mut input = valid_input("My Resin");
    input.lift_speed_mm_min = "fast".to_owned();
    assert_eq!(
        validate_input(&input, &BTreeSet::new()),
        Err(AuthoringError::NotANumber {
            field: "Lift speed"
        })
    );
}

#[test]
fn validate_input_rejects_an_out_of_range_field() {
    let mut input = valid_input("My Resin");
    input.normal_exposure_sec = "-1".to_owned();
    assert_eq!(
        validate_input(&input, &BTreeSet::new()),
        Err(AuthoringError::OutOfRange {
            field: "Normal exposure"
        })
    );
}

#[test]
fn validate_input_accepts_a_full_form_and_derives_a_unique_id() {
    let existing: BTreeSet<MaterialId> = [material_id("my-resin")].into_iter().collect();
    let validated = validate_input(&valid_input("My Resin"), &existing).unwrap();
    assert_eq!(validated.id, material_id("my-resin-2"));
    assert_eq!(validated.name.as_str(), "My Resin");

    let expected = MaterialPerPrinter {
        normal_exposure_sec: Some(PositiveFiniteSec::new(2.5).unwrap()),
        bottom_exposure_sec: Some(PositiveFiniteSec::new(30.0).unwrap()),
        bottom_layer_count: Some(6),
        lift_distance_mm: Some(PositiveFiniteMm::new(6.0).unwrap()),
        lift_speed_mm_min: Some(PositiveFiniteMmPerMin::new(60.0).unwrap()),
        retract_speed_mm_min: Some(PositiveFiniteMmPerMin::new(150.0).unwrap()),
    };
    assert_eq!(validated.per_printer, expected);
}

#[test]
fn build_material_profile_seeds_from_source_and_adds_the_per_printer_block() {
    let source = source_profile();
    let printer = PrinterId::new("elegoo-mars-5").unwrap();
    let validated = validate_input(&valid_input("My Resin"), &BTreeSet::new()).unwrap();

    let profile = build_material_profile(&source, &printer, validated);

    assert_eq!(profile.identity.id, material_id("my-resin"));
    assert_eq!(profile.identity.name.as_str(), "My Resin");
    // Brand, family, version, intent and defaults come from the source.
    assert_eq!(profile.identity.brand.as_str(), "Brand");
    assert_eq!(profile.identity.family.as_str(), "tough");
    assert_eq!(
        profile.identity.profile_version,
        source.identity.profile_version
    );
    assert_eq!(profile.intent, source.intent);
    assert_eq!(profile.defaults, source.defaults);
    // Exactly one per-printer block, for the selected printer.
    assert_eq!(profile.per_printer.len(), 1);
    assert!(profile.per_printer.contains_key(&printer));
}
