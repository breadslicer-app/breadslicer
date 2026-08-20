/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};

use sc_print_primitives::{MaterialId, PrinterId};

use super::calibration_target::CalibrationTarget;
use super::effective_material::{EffectiveMaterial, MaterialExtensionIssue};
use super::library::user_library_dir;
use super::loader::{
    LoadMaterialProfileByIdError, LoadPrinterProfileByIdError, LoadPrinterProfileErrorKind,
};
use super::profile_libraries::{ProfileLibraries, ProfileSource};

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

const EXTENSION_TOML: &str = r#"
extension = true

[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
name = "Blu"
family = "standard"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."printer-x"]
normal_exposure_sec = 3.5
"#;

fn write_printer(root: &Path, id: &str, contents: &str) {
    let dir = root.join("printers");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{id}.toml")), contents).unwrap();
}

fn write_material(root: &Path, id: &str, contents: &str) {
    let dir = root.join("materials");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{id}.toml")), contents).unwrap();
}

fn write_extension(root: &Path, id: &str, contents: &str) {
    let dir = root.join("materials").join("extensions");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{id}.toml")), contents).unwrap();
}

fn printer_id() -> PrinterId {
    PrinterId::new("elegoo-saturn-4-ultra").unwrap()
}

fn material_id() -> MaterialId {
    MaterialId::new("sirayatech-blu").unwrap()
}

/// A user root plus 2 bundled roots inside `dir`, all existing but
/// empty. Returned as `(libraries, user, bundled_a, bundled_b)`.
fn three_roots(dir: &Path) -> (ProfileLibraries, PathBuf, PathBuf, PathBuf) {
    let user = dir.join("user");
    let bundled_a = dir.join("bundled-a");
    let bundled_b = dir.join("bundled-b");
    for root in [&user, &bundled_a, &bundled_b] {
        fs::create_dir_all(root).unwrap();
    }
    let libraries = ProfileLibraries::new(
        Some(user.clone()),
        vec![bundled_a.clone(), bundled_b.clone()],
    );
    (libraries, user, bundled_a, bundled_b)
}

#[test]
fn roots_yields_the_user_root_first_then_the_bundled_roots_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, user, bundled_a, bundled_b) = three_roots(dir.path());
    let roots: Vec<(&Path, ProfileSource)> = libraries.roots().collect();
    assert_eq!(
        roots,
        vec![
            (user.as_path(), ProfileSource::User),
            (bundled_a.as_path(), ProfileSource::Bundled),
            (bundled_b.as_path(), ProfileSource::Bundled),
        ],
    );
}

#[test]
fn roots_without_a_user_root_yields_only_the_bundled_roots() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled");
    fs::create_dir_all(&bundled).unwrap();
    let libraries = ProfileLibraries::new(None, vec![bundled.clone()]);
    assert_eq!(libraries.user_library_dir(), None);
    let roots: Vec<(&Path, ProfileSource)> = libraries.roots().collect();
    assert_eq!(roots, vec![(bundled.as_path(), ProfileSource::Bundled)]);
}

#[test]
fn load_printer_by_id_prefers_the_user_root_over_every_bundled_root() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, user, bundled_a, bundled_b) = three_roots(dir.path());
    write_printer(&user, "elegoo-saturn-4-ultra", VALID_PRINTER_TOML);
    let sentinel_a = VALID_PRINTER_TOML.replace("ELEGOO", "BUNDLED-A");
    let sentinel_b = VALID_PRINTER_TOML.replace("ELEGOO", "BUNDLED-B");
    write_printer(&bundled_a, "elegoo-saturn-4-ultra", &sentinel_a);
    write_printer(&bundled_b, "elegoo-saturn-4-ultra", &sentinel_b);

    let profile = libraries.load_printer_by_id(&printer_id()).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "ELEGOO");
}

#[test]
fn load_printer_by_id_prefers_the_first_bundled_root_over_the_second() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, _user, bundled_a, bundled_b) = three_roots(dir.path());
    let sentinel_a = VALID_PRINTER_TOML.replace("ELEGOO", "BUNDLED-A");
    let sentinel_b = VALID_PRINTER_TOML.replace("ELEGOO", "BUNDLED-B");
    write_printer(&bundled_a, "elegoo-saturn-4-ultra", &sentinel_a);
    write_printer(&bundled_b, "elegoo-saturn-4-ultra", &sentinel_b);

    let profile = libraries.load_printer_by_id(&printer_id()).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "BUNDLED-A");
}

#[test]
fn load_printer_by_id_reaches_a_second_bundled_root() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, _user, _bundled_a, bundled_b) = three_roots(dir.path());
    write_printer(&bundled_b, "elegoo-saturn-4-ultra", VALID_PRINTER_TOML);

    let profile = libraries.load_printer_by_id(&printer_id()).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "ELEGOO");
}

#[test]
fn load_printer_by_id_without_a_user_root_loads_from_a_bundled_root() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled");
    fs::create_dir_all(&bundled).unwrap();
    write_printer(&bundled, "elegoo-saturn-4-ultra", VALID_PRINTER_TOML);
    let libraries = ProfileLibraries::new(None, vec![bundled]);

    let profile = libraries.load_printer_by_id(&printer_id()).unwrap();
    assert_eq!(profile.identity.manufacturer.as_str(), "ELEGOO");
}

#[test]
fn load_printer_by_id_not_found_lists_every_attempted_path_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, user, bundled_a, bundled_b) = three_roots(dir.path());

    let err = libraries.load_printer_by_id(&printer_id()).unwrap_err();
    match &err {
        LoadPrinterProfileByIdError::NotFoundAnywhere { attempted } => {
            let expected: Vec<PathBuf> = [&user, &bundled_a, &bundled_b]
                .into_iter()
                .map(|root| root.join("printers").join("elegoo-saturn-4-ultra.toml"))
                .collect();
            assert_eq!(attempted, &expected);
            let msg = format!("{err}");
            for path in &expected {
                assert!(msg.contains(path.to_str().unwrap()), "msg: {msg}");
            }
        }
        other => panic!("expected NotFoundAnywhere, got {other:?}"),
    }
}

#[test]
fn load_printer_by_id_halts_on_a_parse_error_in_an_earlier_root() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, user, bundled_a, _bundled_b) = three_roots(dir.path());
    write_printer(&user, "elegoo-saturn-4-ultra", "not valid toml [");
    write_printer(&bundled_a, "elegoo-saturn-4-ultra", VALID_PRINTER_TOML);

    let err = libraries.load_printer_by_id(&printer_id()).unwrap_err();
    match err {
        LoadPrinterProfileByIdError::Source(inner) => {
            assert!(inner.path.starts_with(&user));
            assert!(matches!(
                inner.kind,
                LoadPrinterProfileErrorKind::TomlParse(_)
            ));
        }
        other => panic!("expected Source, got {other:?}"),
    }
}

#[test]
fn load_printer_by_id_with_no_roots_renders_an_explicit_not_found_message() {
    let libraries = ProfileLibraries::new(None, Vec::new());
    let err = libraries.load_printer_by_id(&printer_id()).unwrap_err();
    assert!(matches!(
        &err,
        LoadPrinterProfileByIdError::NotFoundAnywhere { attempted } if attempted.is_empty(),
    ));
    let rendered = err.to_string();
    assert!(
        rendered.contains("no profile library roots configured"),
        "rendered: {rendered}",
    );
}

#[test]
fn load_material_by_id_reaches_a_second_bundled_root() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, _user, _bundled_a, bundled_b) = three_roots(dir.path());
    write_material(&bundled_b, "sirayatech-blu", VALID_MATERIAL_TOML);

    let profile = libraries.load_material_by_id(&material_id()).unwrap();
    assert_eq!(profile.identity.brand.as_str(), "Siraya Tech");
}

#[test]
fn load_material_by_id_not_found_lists_every_attempted_path_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, user, bundled_a, bundled_b) = three_roots(dir.path());

    let err = libraries.load_material_by_id(&material_id()).unwrap_err();
    match &err {
        LoadMaterialProfileByIdError::NotFoundAnywhere { attempted } => {
            let expected: Vec<PathBuf> = [&user, &bundled_a, &bundled_b]
                .into_iter()
                .map(|root| root.join("materials").join("sirayatech-blu.toml"))
                .collect();
            assert_eq!(attempted, &expected);
            let msg = format!("{err}");
            for path in &expected {
                assert!(msg.contains(path.to_str().unwrap()), "msg: {msg}");
            }
        }
        other => panic!("expected NotFoundAnywhere, got {other:?}"),
    }
}

#[test]
fn load_material_by_id_with_no_roots_renders_an_explicit_not_found_message() {
    let libraries = ProfileLibraries::new(None, Vec::new());
    let err = libraries.load_material_by_id(&material_id()).unwrap_err();
    let rendered = err.to_string();
    assert!(
        rendered.contains("no profile library roots configured"),
        "rendered: {rendered}",
    );
}

#[test]
fn effective_material_by_id_composes_the_user_extension_over_a_bundled_base() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, user, bundled_a, _bundled_b) = three_roots(dir.path());
    write_material(&bundled_a, "sirayatech-blu", VALID_MATERIAL_TOML);
    write_extension(&user, "sirayatech-blu", EXTENSION_TOML);

    let (effective, issues) = libraries.effective_material_by_id(&material_id()).unwrap();
    assert!(issues.is_empty(), "issues: {issues:?}");
    let printer_x = CalibrationTarget::from_printer(&PrinterId::new("printer-x").unwrap());
    assert_eq!(
        effective.per_printer()[&printer_x]
            .normal_exposure_sec
            .unwrap()
            .value(),
        3.5,
    );
}

#[test]
fn effective_material_by_id_without_a_user_root_yields_the_base_with_no_issues() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled");
    fs::create_dir_all(&bundled).unwrap();
    write_material(&bundled, "sirayatech-blu", VALID_MATERIAL_TOML);
    let libraries = ProfileLibraries::new(None, vec![bundled.clone()]);

    let (effective, issues) = libraries.effective_material_by_id(&material_id()).unwrap();
    assert!(issues.is_empty(), "issues: {issues:?}");
    let base = libraries.load_material_by_id(&material_id()).unwrap();
    assert_eq!(effective, EffectiveMaterial::from(base));
}

#[test]
fn effective_material_by_id_reports_an_unreadable_extension_and_degrades() {
    let dir = tempfile::tempdir().unwrap();
    let (libraries, user, bundled_a, _bundled_b) = three_roots(dir.path());
    write_material(&bundled_a, "sirayatech-blu", VALID_MATERIAL_TOML);
    write_extension(&user, "sirayatech-blu", "not valid toml [");

    let (effective, issues) = libraries.effective_material_by_id(&material_id()).unwrap();
    let base = libraries.load_material_by_id(&material_id()).unwrap();
    assert_eq!(effective, EffectiveMaterial::from(base));
    assert_eq!(issues.len(), 1, "issues: {issues:?}");
    match &issues[0] {
        MaterialExtensionIssue::ExtensionUnreadable { path, cause } => {
            assert!(path.ends_with("materials/extensions/sirayatech-blu.toml"));
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
fn compose_with_extension_without_a_user_root_yields_the_base_with_no_issues() {
    let libraries = ProfileLibraries::new(None, Vec::new());
    let base: super::material_profile::MaterialProfile =
        toml_edit::de::from_str(VALID_MATERIAL_TOML).unwrap();

    let (effective, issues) = libraries.compose_with_extension(base.clone());
    assert!(issues.is_empty(), "issues: {issues:?}");
    assert_eq!(effective, EffectiveMaterial::from(base));
}

#[test]
fn compose_with_extension_without_an_extension_file_yields_the_base_with_no_issues() {
    let dir = tempfile::tempdir().unwrap();
    let libraries = ProfileLibraries::new(Some(dir.path().to_path_buf()), Vec::new());
    let base: super::material_profile::MaterialProfile =
        toml_edit::de::from_str(VALID_MATERIAL_TOML).unwrap();

    let (effective, issues) = libraries.compose_with_extension(base.clone());
    assert!(issues.is_empty(), "issues: {issues:?}");
    assert_eq!(effective, EffectiveMaterial::from(base));
}

#[test]
fn compose_with_extension_salvages_valid_pairs_next_to_an_overlap() {
    let dir = tempfile::tempdir().unwrap();
    let base_toml =
        format!("{VALID_MATERIAL_TOML}\n[per_printer.\"printer-a\"]\nnormal_exposure_sec = 2.5\n");
    let extension_toml =
        format!("{EXTENSION_TOML}\n[per_printer.\"printer-a\"]\nnormal_exposure_sec = 9.0\n");
    write_extension(dir.path(), "sirayatech-blu", &extension_toml);
    let libraries = ProfileLibraries::new(Some(dir.path().to_path_buf()), Vec::new());
    let base: super::material_profile::MaterialProfile =
        toml_edit::de::from_str(&base_toml).unwrap();

    let (effective, issues) = libraries.compose_with_extension(base);

    let printer_a = CalibrationTarget::from_printer(&PrinterId::new("printer-a").unwrap());
    let printer_x = CalibrationTarget::from_printer(&PrinterId::new("printer-x").unwrap());
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
        effective.per_printer()[&printer_x]
            .normal_exposure_sec
            .unwrap()
            .value(),
        3.5,
    );
}

#[test]
fn platform_default_puts_the_user_library_first_and_bundled_profiles_after() {
    let libraries = ProfileLibraries::platform_default();
    let roots: Vec<(&Path, ProfileSource)> = libraries.roots().collect();
    match user_library_dir() {
        Ok(user) => {
            assert_eq!(libraries.user_library_dir(), Some(user.as_path()));
            assert_eq!(roots.first(), Some(&(user.as_path(), ProfileSource::User)));
        }
        Err(_) => assert_eq!(libraries.user_library_dir(), None),
    }
    for (root, source) in roots {
        if source == ProfileSource::Bundled {
            assert!(
                root.ends_with("profiles"),
                "bundled root not a profiles dir: {}",
                root.display(),
            );
        }
    }
}
