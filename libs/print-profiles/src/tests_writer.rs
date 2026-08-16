/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::MaterialId;

use crate::loader::{
    load_material_extension_from_library, load_material_profile_from_path,
    load_printer_profile_from_path,
};
use crate::material_profile::MaterialProfile;
use crate::printer_profile::PrinterProfile;
use crate::writer::{
    SaveMaterialExtensionErrorKind, save_material_extension_to_library,
    save_material_profile_to_library, save_material_profile_to_path,
    save_printer_profile_to_library, save_printer_profile_to_path,
};

const MINIMAL_TOML: &str = "\
[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"other\"
name = \"Resin\"
profile_version = \"1.0.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[defaults.scale_compensation_pct]
x = 0.0
y = 0.0
z = 0.0

[per_printer]
";

const WITH_PER_PRINTER_TOML: &str = "\
[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"other\"
name = \"Resin\"
profile_version = \"1.0.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[defaults.scale_compensation_pct]
x = 0.0
y = 0.0
z = 0.0

[per_printer.elegoo-saturn-4-ctb]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 6
lift_distance_mm = 6.0
lift_speed_mm_min = 60.0
retract_speed_mm_min = 150.0
";

fn parse(toml: &str) -> MaterialProfile {
    toml_edit::de::from_str(toml).unwrap()
}

#[test]
fn save_to_path_round_trips_a_minimal_profile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.toml");
    let original = parse(MINIMAL_TOML);

    save_material_profile_to_path(&path, &original).unwrap();
    let loaded = load_material_profile_from_path(&path).unwrap();
    assert_eq!(loaded, original);
}

#[test]
fn save_to_path_round_trips_per_printer_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.toml");
    let original = parse(WITH_PER_PRINTER_TOML);

    save_material_profile_to_path(&path, &original).unwrap();
    let loaded = load_material_profile_from_path(&path).unwrap();
    assert_eq!(loaded, original);
}

#[test]
fn save_to_path_writes_sectioned_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.toml");
    let original = parse(WITH_PER_PRINTER_TOML);

    save_material_profile_to_path(&path, &original).unwrap();

    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("[identity]"), "written file:\n{written}");
    assert!(written.contains("[intent]"), "written file:\n{written}");
    assert!(
        written.contains("[per_printer.elegoo-saturn-4-ctb]"),
        "written file:\n{written}"
    );
    assert!(
        !written.contains("identity = {"),
        "top-level structs must not be inline:\n{written}"
    );
}

#[test]
fn save_to_path_creates_missing_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/deeper/out.toml");
    let original = parse(MINIMAL_TOML);

    save_material_profile_to_path(&path, &original).unwrap();
    assert!(path.is_file());
}

#[test]
fn save_to_library_creates_the_materials_subdir_and_places_the_file_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let original = parse(MINIMAL_TOML);

    save_material_profile_to_library(dir.path(), &original).unwrap();

    let expected = dir.path().join("materials").join("brand-resin.toml");
    assert!(expected.is_file());
    let loaded = load_material_profile_from_path(&expected).unwrap();
    assert_eq!(loaded, original);
}

const PRINTER_TOML: &str = "\
[identity]
id = \"brand-printer\"
manufacturer = \"Brand\"
family = \"Series\"
name = \"Printer\"
profile_version = \"1.0.0\"

[hardware]
build_volume = { x_mm = 100.0, y_mm = 60.0, z_mm = 150.0 }
pixel_resolution = { x = 1000, y = 600 }
pixel_size_microns = { x = 100.0, y = 100.0 }
mirror = { x = false, y = false }

[capabilities]
output_formats = [{ format = \"png_per_layer\" }]
supports_grayscale = true
two_stage_lift = false
has_camera = false

[constraints]
";

fn parse_printer(toml: &str) -> PrinterProfile {
    toml_edit::de::from_str(toml).unwrap()
}

#[test]
fn save_printer_to_path_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.toml");
    let original = parse_printer(PRINTER_TOML);

    save_printer_profile_to_path(&path, &original).unwrap();
    let loaded = load_printer_profile_from_path(&path).unwrap();
    assert_eq!(loaded, original);
}

#[test]
fn save_printer_to_path_writes_sectioned_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.toml");
    let original = parse_printer(PRINTER_TOML);

    save_printer_profile_to_path(&path, &original).unwrap();

    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("[identity]"), "written file:\n{written}");
    assert!(
        written.contains("[hardware.build_volume]"),
        "written file:\n{written}"
    );
    assert!(
        written.contains("[capabilities]"),
        "written file:\n{written}"
    );
    assert!(
        !written.contains("identity = {"),
        "top-level structs must not be inline:\n{written}"
    );
    assert!(
        !written.contains("hardware = {"),
        "top-level structs must not be inline:\n{written}"
    );
}

#[test]
fn save_printer_to_path_creates_missing_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/deeper/out.toml");
    let original = parse_printer(PRINTER_TOML);

    save_printer_profile_to_path(&path, &original).unwrap();
    assert!(path.is_file());
}

#[test]
fn save_printer_to_library_creates_the_printers_subdir_and_places_the_file_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let original = parse_printer(PRINTER_TOML);

    save_printer_profile_to_library(dir.path(), &original).unwrap();

    let expected = dir.path().join("printers").join("brand-printer.toml");
    assert!(expected.is_file());
    let loaded = load_printer_profile_from_path(&expected).unwrap();
    assert_eq!(loaded, original);
}

/// An extension with empty `[intent]` / `[defaults]` sections and one
/// per-printer block, as the extend flow produces.
const EXTENSION_TOML: &str = "\
extension = true

[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"other\"
name = \"Resin\"
profile_version = \"1.0.0\"

[intent]

[defaults]

[per_printer.elegoo-saturn-4-ctb]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
";

#[test]
fn save_extension_rejects_a_profile_without_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let profile = parse(MINIMAL_TOML);
    assert!(!profile.extension);

    let err = save_material_extension_to_library(dir.path(), &profile).unwrap_err();
    assert!(matches!(
        err.kind,
        SaveMaterialExtensionErrorKind::NotAnExtension
    ));
    assert!(
        !dir.path().join("materials").exists(),
        "nothing may be written on rejection"
    );
}

#[test]
fn save_extension_creates_the_subdir_and_places_the_file_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let original = parse(EXTENSION_TOML);

    save_material_extension_to_library(dir.path(), &original).unwrap();

    let expected = dir
        .path()
        .join("materials")
        .join("extensions")
        .join("brand-resin.toml");
    assert!(expected.is_file());
}

#[test]
fn save_extension_round_trips_through_the_extension_loader() {
    let dir = tempfile::tempdir().unwrap();
    let original = parse(EXTENSION_TOML);

    save_material_extension_to_library(dir.path(), &original).unwrap();

    let id = MaterialId::new("brand-resin").unwrap();
    let loaded = load_material_extension_from_library(dir.path(), &id).unwrap();
    assert_eq!(loaded, original);
}

#[test]
fn save_extension_annotates_empty_intent_and_defaults_sections() {
    let dir = tempfile::tempdir().unwrap();
    let original = parse(EXTENSION_TOML);

    save_material_extension_to_library(dir.path(), &original).unwrap();

    let written = std::fs::read_to_string(
        dir.path()
            .join("materials")
            .join("extensions")
            .join("brand-resin.toml"),
    )
    .unwrap();
    let comment = "# Values are inherited from the parent profile.";
    let occurrences = written.matches(comment).count();
    assert_eq!(occurrences, 2, "written file:\n{written}");
    let intent_pos = written.find("[intent]").unwrap();
    let comment_pos = written.find(comment).unwrap();
    assert!(comment_pos < intent_pos, "written file:\n{written}");
}

#[test]
fn save_extension_does_not_annotate_populated_sections() {
    let dir = tempfile::tempdir().unwrap();
    // Full sections, still flagged as an extension: no comment applies.
    let toml = format!("extension = true\n{WITH_PER_PRINTER_TOML}");
    let original = parse(&toml);

    save_material_extension_to_library(dir.path(), &original).unwrap();

    let written = std::fs::read_to_string(
        dir.path()
            .join("materials")
            .join("extensions")
            .join("brand-resin.toml"),
    )
    .unwrap();
    assert!(
        !written.contains("inherited from the parent profile"),
        "written file:\n{written}"
    );
}
