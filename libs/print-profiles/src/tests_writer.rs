/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tests for the material-profile writers: a written file loads back
//! equal, and the library writer creates the `materials/` subdirectory
//! when absent.

use crate::loader::load_material_profile_from_path;
use crate::material_profile::MaterialProfile;
use crate::writer::{save_material_profile_to_library, save_material_profile_to_path};

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
