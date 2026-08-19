/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;
use std::io;
use std::path::Path;

use sc_print_primitives::{MaterialId, PrinterId};

use super::calibration_target::CalibrationTarget;
use super::effective_material::{EffectiveMaterial, MaterialExtensionIssue};
use super::loader::{
    LoadMaterialExtensionError, LoadMaterialProfileByIdError, LoadMaterialProfileError,
    LoadMaterialProfileErrorKind, LoadPrinterProfileByIdError, LoadPrinterProfileError,
    LoadPrinterProfileErrorKind, compose_with_library_extension,
    load_material_extension_from_library, load_material_extension_if_present,
    load_material_profile_by_id, load_material_profile_from_library,
    load_material_profile_from_path, load_printer_profile_by_id, load_printer_profile_from_library,
    load_printer_profile_from_path,
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
supports_grayscale = true
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

#[test]
fn load_printer_profile_from_library_resolves_id_to_path_and_loads() {
    let dir = tempfile::tempdir().unwrap();
    let printers_dir = dir.path().join("printers");
    fs::create_dir_all(&printers_dir).unwrap();
    let path = printers_dir.join("elegoo-saturn-4-ultra.toml");
    fs::write(&path, VALID_PRINTER_TOML).unwrap();

    let id = PrinterId::new("elegoo-saturn-4-ultra").unwrap();
    let profile = load_printer_profile_from_library(dir.path(), &id).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "ELEGOO");
}

#[test]
fn load_printer_profile_from_library_with_missing_file_returns_io_error_with_resolved_path() {
    let dir = tempfile::tempdir().unwrap();
    let id = PrinterId::new("does-not-exist").unwrap();
    let err = load_printer_profile_from_library(dir.path(), &id).unwrap_err();
    let expected = dir.path().join("printers").join("does-not-exist.toml");
    assert_eq!(err.path, expected);
    match err.kind {
        LoadPrinterProfileErrorKind::Io(io_err) => {
            assert_eq!(io_err.kind(), io::ErrorKind::NotFound);
        }
        other => panic!("expected Io kind, got {other:?}"),
    }
}

#[test]
fn load_material_profile_from_library_resolves_id_to_path_and_loads() {
    let dir = tempfile::tempdir().unwrap();
    let materials_dir = dir.path().join("materials");
    fs::create_dir_all(&materials_dir).unwrap();
    let path = materials_dir.join("sirayatech-blu.toml");
    fs::write(&path, VALID_MATERIAL_TOML).unwrap();

    let id = MaterialId::new("sirayatech-blu").unwrap();
    let profile = load_material_profile_from_library(dir.path(), &id).unwrap();
    assert_eq!(profile.identity.brand.as_str(), "Siraya Tech");
}

#[test]
fn load_material_profile_from_library_with_missing_file_returns_io_error_with_resolved_path() {
    let dir = tempfile::tempdir().unwrap();
    let id = MaterialId::new("does-not-exist").unwrap();
    let err = load_material_profile_from_library(dir.path(), &id).unwrap_err();
    let expected = dir.path().join("materials").join("does-not-exist.toml");
    assert_eq!(err.path, expected);
    match err.kind {
        LoadMaterialProfileErrorKind::Io(io_err) => {
            assert_eq!(io_err.kind(), io::ErrorKind::NotFound);
        }
        other => panic!("expected Io kind, got {other:?}"),
    }
}

#[test]
fn load_printer_profile_by_id_uses_explicit_path_when_provided() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(&library_dir).unwrap();
    fs::create_dir_all(&assets_dir).unwrap();
    let explicit = write_temp_file(dir.path(), "explicit.toml", VALID_PRINTER_TOML);

    let id = PrinterId::new("does-not-exist-as-id").unwrap();
    let profile =
        load_printer_profile_by_id(&id, Some(&explicit), &library_dir, &assets_dir).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "ELEGOO");
}

#[test]
fn load_printer_profile_by_id_explicit_path_failures_are_hard_errors() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(&library_dir).unwrap();
    fs::create_dir_all(&assets_dir).unwrap();
    // Put a valid fixture in the library so we know cascading would
    // have worked if the explicit-path semantics allowed it.
    let printers_dir = library_dir.join("printers");
    fs::create_dir_all(&printers_dir).unwrap();
    fs::write(
        printers_dir.join("elegoo-saturn-4-ultra.toml"),
        VALID_PRINTER_TOML,
    )
    .unwrap();
    let missing = dir.path().join("explicit-but-missing.toml");

    let id = PrinterId::new("elegoo-saturn-4-ultra").unwrap();
    let err =
        load_printer_profile_by_id(&id, Some(&missing), &library_dir, &assets_dir).unwrap_err();
    match err {
        LoadPrinterProfileByIdError::Source(inner) => {
            assert_eq!(inner.path, missing);
            assert!(matches!(inner.kind, LoadPrinterProfileErrorKind::Io(_),));
        }
        other => panic!("expected Source kind, got {other:?}"),
    }
}

#[test]
fn load_printer_profile_by_id_falls_back_from_library_to_assets() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(library_dir.join("printers")).unwrap();
    let assets_printers = assets_dir.join("printers");
    fs::create_dir_all(&assets_printers).unwrap();
    fs::write(
        assets_printers.join("elegoo-saturn-4-ultra.toml"),
        VALID_PRINTER_TOML,
    )
    .unwrap();

    let id = PrinterId::new("elegoo-saturn-4-ultra").unwrap();
    let profile = load_printer_profile_by_id(&id, None, &library_dir, &assets_dir).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "ELEGOO");
}

#[test]
fn load_printer_profile_by_id_prefers_library_over_assets_when_both_match() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(library_dir.join("printers")).unwrap();
    fs::create_dir_all(assets_dir.join("printers")).unwrap();
    // Library copy keeps the manufacturer as "ELEGOO"; assets copy
    // is mutated to a sentinel value so the test can tell which one
    // was returned.
    fs::write(
        library_dir
            .join("printers")
            .join("elegoo-saturn-4-ultra.toml"),
        VALID_PRINTER_TOML,
    )
    .unwrap();
    let assets_toml = VALID_PRINTER_TOML.replace("ELEGOO", "ASSETS-WIN");
    fs::write(
        assets_dir
            .join("printers")
            .join("elegoo-saturn-4-ultra.toml"),
        &assets_toml,
    )
    .unwrap();

    let id = PrinterId::new("elegoo-saturn-4-ultra").unwrap();
    let profile = load_printer_profile_by_id(&id, None, &library_dir, &assets_dir).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "ELEGOO");
}

#[test]
fn load_printer_profile_by_id_not_found_anywhere_lists_each_attempted_path() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(&library_dir).unwrap();
    fs::create_dir_all(&assets_dir).unwrap();

    let id = PrinterId::new("does-not-exist").unwrap();
    let err = load_printer_profile_by_id(&id, None, &library_dir, &assets_dir).unwrap_err();
    match &err {
        LoadPrinterProfileByIdError::NotFoundAnywhere { attempted } => {
            let library_path = library_dir.join("printers").join("does-not-exist.toml");
            let assets_path = assets_dir.join("printers").join("does-not-exist.toml");
            assert_eq!(attempted, &vec![library_path.clone(), assets_path.clone()]);
            let msg = format!("{err}");
            assert!(msg.contains(library_path.to_str().unwrap()), "msg: {msg}");
            assert!(msg.contains(assets_path.to_str().unwrap()), "msg: {msg}");
        }
        other => panic!("expected NotFoundAnywhere, got {other:?}"),
    }
}

#[test]
fn load_printer_profile_by_id_hard_fails_on_bad_toml_in_library_without_cascading() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(library_dir.join("printers")).unwrap();
    fs::create_dir_all(assets_dir.join("printers")).unwrap();
    // Library has a broken file; assets has a valid one. The
    // semantics: library's parse error should NOT cascade to assets.
    fs::write(
        library_dir
            .join("printers")
            .join("elegoo-saturn-4-ultra.toml"),
        "this is not = valid toml [\n",
    )
    .unwrap();
    fs::write(
        assets_dir
            .join("printers")
            .join("elegoo-saturn-4-ultra.toml"),
        VALID_PRINTER_TOML,
    )
    .unwrap();

    let id = PrinterId::new("elegoo-saturn-4-ultra").unwrap();
    let err = load_printer_profile_by_id(&id, None, &library_dir, &assets_dir).unwrap_err();
    match err {
        LoadPrinterProfileByIdError::Source(inner) => {
            assert!(matches!(
                inner.kind,
                LoadPrinterProfileErrorKind::TomlParse(_),
            ));
        }
        other => panic!("expected Source kind, got {other:?}"),
    }
}

#[test]
fn load_material_profile_by_id_uses_explicit_path_when_provided() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(&library_dir).unwrap();
    fs::create_dir_all(&assets_dir).unwrap();
    let explicit = write_temp_file(dir.path(), "explicit.toml", VALID_MATERIAL_TOML);

    let id = MaterialId::new("does-not-exist-as-id").unwrap();
    let profile =
        load_material_profile_by_id(&id, Some(&explicit), &library_dir, &assets_dir).unwrap();
    assert_eq!(profile.identity.brand.as_str(), "Siraya Tech");
}

#[test]
fn load_material_profile_by_id_falls_back_from_library_to_assets() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(library_dir.join("materials")).unwrap();
    let assets_materials = assets_dir.join("materials");
    fs::create_dir_all(&assets_materials).unwrap();
    fs::write(
        assets_materials.join("sirayatech-blu.toml"),
        VALID_MATERIAL_TOML,
    )
    .unwrap();

    let id = MaterialId::new("sirayatech-blu").unwrap();
    let profile = load_material_profile_by_id(&id, None, &library_dir, &assets_dir).unwrap();
    assert_eq!(profile.identity.brand.as_str(), "Siraya Tech");
}

#[test]
fn load_material_profile_by_id_not_found_anywhere_lists_each_attempted_path() {
    let dir = tempfile::tempdir().unwrap();
    let library_dir = dir.path().join("library");
    let assets_dir = dir.path().join("assets");
    fs::create_dir_all(&library_dir).unwrap();
    fs::create_dir_all(&assets_dir).unwrap();

    let id = MaterialId::new("does-not-exist").unwrap();
    let err = load_material_profile_by_id(&id, None, &library_dir, &assets_dir).unwrap_err();
    match &err {
        LoadMaterialProfileByIdError::NotFoundAnywhere { attempted } => {
            let library_path = library_dir.join("materials").join("does-not-exist.toml");
            let assets_path = assets_dir.join("materials").join("does-not-exist.toml");
            assert_eq!(attempted, &vec![library_path.clone(), assets_path.clone()]);
            let msg = format!("{err}");
            assert!(msg.contains(library_path.to_str().unwrap()), "msg: {msg}");
            assert!(msg.contains(assets_path.to_str().unwrap()), "msg: {msg}");
        }
        other => panic!("expected NotFoundAnywhere, got {other:?}"),
    }
}

#[test]
fn load_by_id_error_types_implement_error_trait() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<LoadPrinterProfileByIdError>();
    assert_error::<LoadMaterialProfileByIdError>();
    assert_error::<LoadMaterialExtensionError>();
}

#[test]
fn library_path_accessors_mirror_the_loader_layout() {
    use super::loader::{
        material_extension_path_in_library, material_profile_path_in_library,
        printer_profile_path_in_library,
    };

    let base = Path::new("/lib");
    let id = MaterialId::new("brand-resin").unwrap();
    assert_eq!(
        material_profile_path_in_library(base, &id),
        Path::new("/lib/materials/brand-resin.toml"),
    );
    assert_eq!(
        material_extension_path_in_library(base, &id),
        Path::new("/lib/materials/extensions/brand-resin.toml"),
    );
    let printer = PrinterId::new("vendor-printer").unwrap();
    assert_eq!(
        printer_profile_path_in_library(base, &printer),
        Path::new("/lib/printers/vendor-printer.toml"),
    );
}

fn write_extension_file(library_dir: &Path, id: &str, contents: &str) {
    let extensions_dir = library_dir.join("materials").join("extensions");
    fs::create_dir_all(&extensions_dir).unwrap();
    fs::write(extensions_dir.join(format!("{id}.toml")), contents).unwrap();
}

#[test]
fn load_material_extension_accepts_a_flagged_file() {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!("extension = true\n{VALID_MATERIAL_TOML}");
    write_extension_file(dir.path(), "sirayatech-blu", &toml);

    let id = MaterialId::new("sirayatech-blu").unwrap();
    let profile = load_material_extension_from_library(dir.path(), &id).unwrap();
    assert!(profile.extension);
    assert_eq!(profile.identity.brand.as_str(), "Siraya Tech");
}

#[test]
fn load_material_extension_rejects_an_unflagged_file() {
    let dir = tempfile::tempdir().unwrap();
    write_extension_file(dir.path(), "sirayatech-blu", VALID_MATERIAL_TOML);

    let id = MaterialId::new("sirayatech-blu").unwrap();
    let err = load_material_extension_from_library(dir.path(), &id).unwrap_err();
    match &err {
        LoadMaterialExtensionError::NotAnExtension { path } => {
            assert_eq!(
                path,
                &dir.path()
                    .join("materials")
                    .join("extensions")
                    .join("sirayatech-blu.toml"),
            );
            let msg = format!("{err}");
            assert!(msg.contains("sirayatech-blu.toml"), "msg: {msg}");
            assert!(msg.contains("extension flag"), "msg: {msg}");
        }
        other => panic!("expected NotAnExtension, got {other:?}"),
    }
}

#[test]
fn load_material_extension_from_missing_file_wraps_the_profile_error() {
    let dir = tempfile::tempdir().unwrap();
    let id = MaterialId::new("does-not-exist").unwrap();
    let err = load_material_extension_from_library(dir.path(), &id).unwrap_err();
    match err {
        LoadMaterialExtensionError::Profile(inner) => match inner.kind {
            LoadMaterialProfileErrorKind::Io(io_err) => {
                assert_eq!(io_err.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected Io kind, got {other:?}"),
        },
        other => panic!("expected Profile, got {other:?}"),
    }
}

#[test]
fn load_material_extension_does_not_resolve_plain_material_files() {
    let dir = tempfile::tempdir().unwrap();
    let materials_dir = dir.path().join("materials");
    fs::create_dir_all(&materials_dir).unwrap();
    let toml = format!("extension = true\n{VALID_MATERIAL_TOML}");
    fs::write(materials_dir.join("sirayatech-blu.toml"), toml).unwrap();

    let id = MaterialId::new("sirayatech-blu").unwrap();
    let err = load_material_extension_from_library(dir.path(), &id).unwrap_err();
    assert!(matches!(err, LoadMaterialExtensionError::Profile(_)));
}

const COMPOSABLE_BASE_TOML: &str = r#"
[identity]
id = "compose-resin"
brand = "Test"
family = "standard"
name = "Compose Resin"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[per_printer."printer-a"]
normal_exposure_sec = 2.5
"#;

const COMPOSABLE_EXTENSION_TOML: &str = r#"
extension = true

[identity]
id = "compose-resin"
brand = "Test"
family = "standard"
name = "Compose Resin"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."printer-b"]
normal_exposure_sec = 3.5
"#;

fn composable_base() -> super::material_profile::MaterialProfile {
    toml_edit::de::from_str(COMPOSABLE_BASE_TOML).unwrap()
}

#[test]
fn extension_if_present_returns_none_when_the_file_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let id = MaterialId::new("compose-resin").unwrap();
    assert_eq!(
        load_material_extension_if_present(dir.path(), &id).unwrap(),
        None,
    );
}

#[test]
fn extension_if_present_returns_the_document_without_flag_validation() {
    // Flag semantics belong to composition, which reports instead of
    // erroring; this loader hands the document over as parsed.
    let dir = tempfile::tempdir().unwrap();
    write_extension_file(dir.path(), "sirayatech-blu", VALID_MATERIAL_TOML);

    let id = MaterialId::new("sirayatech-blu").unwrap();
    let document = load_material_extension_if_present(dir.path(), &id)
        .unwrap()
        .expect("the document is present");
    assert!(!document.extension);
}

#[test]
fn extension_if_present_propagates_a_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    write_extension_file(dir.path(), "compose-resin", "not valid toml [");

    let id = MaterialId::new("compose-resin").unwrap();
    let err = load_material_extension_if_present(dir.path(), &id).unwrap_err();
    assert!(matches!(
        err.kind,
        LoadMaterialProfileErrorKind::TomlParse(_)
    ));
}

#[test]
fn compose_with_library_extension_without_a_file_yields_the_base_only() {
    let dir = tempfile::tempdir().unwrap();
    let (effective, issues) = compose_with_library_extension(composable_base(), dir.path());

    assert!(issues.is_empty());
    assert_eq!(effective, EffectiveMaterial::from(composable_base()));
}

#[test]
fn compose_with_library_extension_folds_a_valid_extension() {
    let dir = tempfile::tempdir().unwrap();
    write_extension_file(dir.path(), "compose-resin", COMPOSABLE_EXTENSION_TOML);

    let (effective, issues) = compose_with_library_extension(composable_base(), dir.path());

    assert!(issues.is_empty());
    let printer_b = CalibrationTarget::from_printer(&PrinterId::new("printer-b").unwrap());
    assert_eq!(
        effective.per_printer()[&printer_b]
            .normal_exposure_sec
            .unwrap()
            .value(),
        3.5,
    );
}

#[test]
fn compose_with_library_extension_reports_an_unreadable_file_and_degrades() {
    let dir = tempfile::tempdir().unwrap();
    write_extension_file(dir.path(), "compose-resin", "not valid toml [");

    let (effective, issues) = compose_with_library_extension(composable_base(), dir.path());

    assert_eq!(effective, EffectiveMaterial::from(composable_base()));
    assert_eq!(issues.len(), 1, "issues: {issues:?}");
    match &issues[0] {
        MaterialExtensionIssue::ExtensionUnreadable { path, cause } => {
            assert!(path.ends_with("materials/extensions/compose-resin.toml"));
            assert!(!cause.is_empty());
        }
        other => panic!("expected ExtensionUnreadable, got {other:?}"),
    }
    let rendered = issues[0].to_string();
    assert!(
        rendered.contains("could not be loaded and was ignored"),
        "rendered: {rendered}",
    );
}

#[test]
fn compose_with_library_extension_salvages_valid_pairs_next_to_an_overlap() {
    let mixed_extension = r#"
extension = true

[identity]
id = "compose-resin"
brand = "Test"
family = "standard"
name = "Compose Resin"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."printer-a"]
normal_exposure_sec = 9.0

[per_printer."printer-b"]
normal_exposure_sec = 3.5
"#;
    let dir = tempfile::tempdir().unwrap();
    write_extension_file(dir.path(), "compose-resin", mixed_extension);

    let (effective, issues) = compose_with_library_extension(composable_base(), dir.path());

    let printer_a = CalibrationTarget::from_printer(&PrinterId::new("printer-a").unwrap());
    let printer_b = CalibrationTarget::from_printer(&PrinterId::new("printer-b").unwrap());
    assert_eq!(
        issues,
        vec![MaterialExtensionIssue::OverlappingCalibrationPair {
            target: printer_a.clone(),
        }],
    );
    // The overlapped pair keeps the base's value; the new pair lands.
    assert_eq!(
        effective.per_printer()[&printer_a]
            .normal_exposure_sec
            .unwrap()
            .value(),
        2.5,
    );
    assert_eq!(
        effective.per_printer()[&printer_b]
            .normal_exposure_sec
            .unwrap()
            .value(),
        3.5,
    );
}
