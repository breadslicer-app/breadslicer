/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;
use std::io;
use std::path::Path;

use super::loader::{
    LoadMaterialProfileError, LoadMaterialProfileErrorKind, LoadPrinterProfileError,
    LoadPrinterProfileErrorKind, load_material_profile_from_path, load_printer_profile_from_path,
};

const VALID_PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-saturn-4-ultra"
manufacturer = "ELEGOO"
family = "Saturn"
name = "Saturn 4 Ultra"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
anti_aliasing = "off"
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.01
"#;

const VALID_MATERIAL_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
name = "Blu"
family = "standard"
profile_version = "1.0.0"

[intent]
target_energy_density_mj_per_cm2 = 15.5

[defaults]
bottom_layer_count = 6
"#;

fn write_temp_file(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).expect("write fixture file");
    path
}

#[test]
fn load_printer_profile_from_path_parses_a_valid_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp_file(dir.path(), "printer.toml", VALID_PRINTER_TOML);
    let profile = load_printer_profile_from_path(&path).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "ELEGOO");
}

#[test]
fn load_printer_profile_from_missing_file_returns_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.toml");
    let err = load_printer_profile_from_path(&path).unwrap_err();
    assert_eq!(err.path, path);
    match err.kind {
        LoadPrinterProfileErrorKind::Io(io_err) => {
            assert_eq!(io_err.kind(), io::ErrorKind::NotFound);
        }
        other => panic!("expected Io kind, got {other:?}"),
    }
}

#[test]
fn load_printer_profile_from_invalid_toml_returns_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp_file(dir.path(), "garbage.toml", "this is not = valid toml [\n");
    let err = load_printer_profile_from_path(&path).unwrap_err();
    assert_eq!(err.path, path);
    assert!(matches!(
        err.kind,
        LoadPrinterProfileErrorKind::TomlParse(_)
    ));
}

#[test]
fn load_printer_profile_with_reserved_instance_section_returns_parse_error_mentioning_instance() {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!("{VALID_PRINTER_TOML}\n[instance]\nserial_number = \"X\"\n");
    let path = write_temp_file(dir.path(), "printer.toml", &toml);
    let err = load_printer_profile_from_path(&path).unwrap_err();
    assert!(matches!(
        err.kind,
        LoadPrinterProfileErrorKind::TomlParse(_)
    ));
    let msg = format!("{err}");
    assert!(msg.contains("instance"), "msg: {msg}");
}

#[test]
fn load_printer_profile_error_display_includes_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.toml");
    let err = load_printer_profile_from_path(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("missing.toml"), "msg: {msg}");
    assert!(msg.contains("printer profile"), "msg: {msg}");
}

#[test]
fn load_printer_profile_error_exposes_source_for_chaining() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.toml");
    let err = load_printer_profile_from_path(&path).unwrap_err();
    let source = std::error::Error::source(&err);
    assert!(source.is_some(), "expected a source error in the chain");
}

#[test]
fn load_material_profile_from_path_parses_a_valid_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp_file(dir.path(), "material.toml", VALID_MATERIAL_TOML);
    let profile = load_material_profile_from_path(&path).unwrap();
    assert_eq!(profile.identity.brand.as_str(), "Siraya Tech");
    assert!(profile.per_printer.is_empty());
}

#[test]
fn load_material_profile_from_missing_file_returns_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.toml");
    let err = load_material_profile_from_path(&path).unwrap_err();
    assert_eq!(err.path, path);
    match err.kind {
        LoadMaterialProfileErrorKind::Io(io_err) => {
            assert_eq!(io_err.kind(), io::ErrorKind::NotFound);
        }
        other => panic!("expected Io kind, got {other:?}"),
    }
}

#[test]
fn load_material_profile_from_invalid_toml_returns_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp_file(dir.path(), "garbage.toml", "this is not = valid toml [\n");
    let err = load_material_profile_from_path(&path).unwrap_err();
    assert!(matches!(
        err.kind,
        LoadMaterialProfileErrorKind::TomlParse(_)
    ));
}

#[test]
fn load_material_profile_with_reserved_per_environment_returns_parse_error_mentioning_per_environment()
 {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!("{VALID_MATERIAL_TOML}\n[per_environment.cold]\nbias = 0.1\n");
    let path = write_temp_file(dir.path(), "material.toml", &toml);
    let err = load_material_profile_from_path(&path).unwrap_err();
    assert!(matches!(
        err.kind,
        LoadMaterialProfileErrorKind::TomlParse(_)
    ));
    let msg = format!("{err}");
    assert!(msg.contains("per_environment"), "msg: {msg}");
}

#[test]
fn load_material_profile_error_display_includes_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.toml");
    let err = load_material_profile_from_path(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("missing.toml"), "msg: {msg}");
    assert!(msg.contains("material profile"), "msg: {msg}");
}

#[test]
fn loader_error_types_implement_error_trait() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<LoadPrinterProfileError>();
    assert_error::<LoadMaterialProfileError>();
}
