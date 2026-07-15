/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! End-to-end coverage of the profile catalog: merging the user and
//! bundled libraries, capturing the fields the pickers show, and
//! resolving thumbnails, driven from outside the crate against
//! tempdir fixtures.

use std::fs;
use std::path::Path;

use sc_print_primitives::{MaterialId, PrinterId};
use sc_slicer_gui::profile_catalog::{ProfileCatalog, ProfileSource};

/// Format an `f64` with a decimal point so TOML parses it as a float
/// (a bare `100` would be an integer and fail the `f64` field).
fn f(value: f64) -> String {
    format!("{value:?}")
}

fn write_printer(
    root: &Path,
    id: &str,
    manufacturer: &str,
    name: &str,
    (x, y, z): (f64, f64, f64),
    image: Option<&str>,
) {
    let printers = root.join("printers");
    fs::create_dir_all(&printers).unwrap();
    // 40 micron pixels: a build-volume axis of `n` mm implies `n * 25`
    // pixels, which keeps the envelope-consistency check happy.
    let res_x = (x * 25.0).round() as u32;
    let res_y = (y * 25.0).round() as u32;
    let image_line = match image {
        Some(rel) => format!("image_asset_path = \"{rel}\"\n"),
        None => String::new(),
    };
    let toml = format!(
        "[identity]\n\
         id = \"{id}\"\n\
         manufacturer = \"{manufacturer}\"\n\
         family = \"Test Family\"\n\
         name = \"{name}\"\n\
         profile_version = \"1.0.0\"\n\
         {image_line}\
         \n\
         [hardware]\n\
         \n\
         [hardware.build_volume]\n\
         x_mm = {x_mm}\n\
         y_mm = {y_mm}\n\
         z_mm = {z_mm}\n\
         \n\
         [hardware.pixel_resolution]\n\
         x = {res_x}\n\
         y = {res_y}\n\
         \n\
         [hardware.pixel_size_microns]\n\
         x = 40.0\n\
         y = 40.0\n\
         \n\
         [hardware.mirror]\n\
         x = false\n\
         y = false\n\
         \n\
         [capabilities]\n\
         output_formats = [{{ format = \"aff\", version = \"pm3\" }}]\n\
         supports_grayscale = true\n\
         two_stage_lift = false\n\
         has_camera = false\n\
         \n\
         [constraints]\n",
        x_mm = f(x),
        y_mm = f(y),
        z_mm = f(z),
    );
    fs::write(printers.join(format!("{id}.toml")), toml).unwrap();
}

fn write_material(
    root: &Path,
    id: &str,
    brand: &str,
    name: &str,
    family: &str,
    calibrated: &[&str],
) {
    let materials = root.join("materials");
    fs::create_dir_all(&materials).unwrap();
    let mut per_printer = String::from("[per_printer]\n");
    for printer_id in calibrated {
        per_printer.push_str(&format!(
            "\n[per_printer.{printer_id}]\n\
             normal_exposure_sec = 2.5\n\
             bottom_exposure_sec = 30.0\n\
             bottom_layer_count = 6\n\
             lift_distance_mm = 6.0\n\
             lift_speed_mm_min = 60.0\n\
             retract_speed_mm_min = 150.0\n"
        ));
    }
    let toml = format!(
        "[identity]\n\
         id = \"{id}\"\n\
         brand = \"{brand}\"\n\
         family = \"{family}\"\n\
         name = \"{name}\"\n\
         profile_version = \"1.0.0\"\n\
         \n\
         [intent]\n\
         target_layer_height_mm = 0.05\n\
         \n\
         [defaults]\n\
         bottom_layer_count = 5\n\
         \n\
         [defaults.scale_compensation_pct]\n\
         x = 0.0\n\
         y = 0.0\n\
         z = 0.0\n\
         \n\
         {per_printer}"
    );
    fs::write(materials.join(format!("{id}.toml")), toml).unwrap();
}

fn printer_id(id: &str) -> PrinterId {
    PrinterId::new(id).unwrap()
}

fn material_id(id: &str) -> MaterialId {
    MaterialId::new(id).unwrap()
}

#[test]
fn empty_libraries_yield_an_empty_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = ProfileCatalog::load(None, &dir.path().join("does-not-exist"));
    assert!(catalog.printers().is_empty());
    assert!(catalog.materials().is_empty());
}

#[test]
fn printers_are_sorted_by_manufacturer_then_name() {
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "z-one",
        "Zeta",
        "One",
        (100.0, 60.0, 80.0),
        None,
    );
    write_printer(
        bundled.path(),
        "a-two",
        "Alpha",
        "Two",
        (100.0, 60.0, 80.0),
        None,
    );
    write_printer(
        bundled.path(),
        "a-one",
        "Alpha",
        "One",
        (100.0, 60.0, 80.0),
        None,
    );

    let catalog = ProfileCatalog::load(None, bundled.path());
    let names: Vec<_> = catalog.printers().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["One", "Two", "One"]);
    let makers: Vec<_> = catalog
        .printers()
        .iter()
        .map(|e| e.manufacturer.as_str())
        .collect();
    assert_eq!(makers, ["Alpha", "Alpha", "Zeta"]);
}

#[test]
fn materials_are_sorted_by_brand_then_name() {
    let bundled = tempfile::tempdir().unwrap();
    write_material(
        bundled.path(),
        "z-resin",
        "Zeta",
        "Alpha Name",
        "other",
        &[],
    );
    write_material(bundled.path(), "a-resin-b", "Alpha", "B", "other", &[]);
    write_material(bundled.path(), "a-resin-a", "Alpha", "A", "other", &[]);

    let catalog = ProfileCatalog::load(None, bundled.path());
    let brands: Vec<_> = catalog
        .materials()
        .iter()
        .map(|e| e.brand.as_str())
        .collect();
    assert_eq!(brands, ["Alpha", "Alpha", "Zeta"]);
    let names: Vec<_> = catalog
        .materials()
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, ["A", "B", "Alpha Name"]);
}

#[test]
fn build_volume_is_captured_from_the_profile() {
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "vol",
        "Maker",
        "Model",
        (192.0, 120.0, 200.0),
        None,
    );

    let catalog = ProfileCatalog::load(None, bundled.path());
    let bv = catalog.printer_build_volume(&printer_id("vol")).unwrap();
    assert_eq!(bv.x_mm(), 192.0);
    assert_eq!(bv.y_mm(), 120.0);
    assert_eq!(bv.z_mm(), 200.0);
}

#[test]
fn image_path_resolves_absolute_against_the_winning_root() {
    let bundled = tempfile::tempdir().unwrap();
    fs::create_dir_all(bundled.path().join("images")).unwrap();
    fs::write(bundled.path().join("images/model.png"), b"not a real png").unwrap();
    write_printer(
        bundled.path(),
        "with-image",
        "Maker",
        "Model",
        (100.0, 60.0, 80.0),
        Some("images/model.png"),
    );

    let catalog = ProfileCatalog::load(None, bundled.path());
    let entry = &catalog.printers()[0];
    assert_eq!(
        entry.image_path,
        Some(bundled.path().join("images/model.png"))
    );
}

#[test]
fn image_path_is_none_when_the_field_is_absent() {
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "no-image",
        "Maker",
        "Model",
        (100.0, 60.0, 80.0),
        None,
    );

    let catalog = ProfileCatalog::load(None, bundled.path());
    assert_eq!(catalog.printers()[0].image_path, None);
}

#[test]
fn image_path_is_none_when_the_referenced_file_is_missing() {
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "dangling",
        "Maker",
        "Model",
        (100.0, 60.0, 80.0),
        Some("images/gone.png"),
    );

    let catalog = ProfileCatalog::load(None, bundled.path());
    assert_eq!(catalog.printers()[0].image_path, None);
}

#[test]
fn user_copy_wins_over_bundled_for_the_same_id() {
    let user = tempfile::tempdir().unwrap();
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "shared",
        "BundledMaker",
        "Bundled",
        (100.0, 60.0, 80.0),
        None,
    );
    write_printer(
        user.path(),
        "shared",
        "UserMaker",
        "User",
        (150.0, 90.0, 120.0),
        None,
    );

    let catalog = ProfileCatalog::load(Some(user.path()), bundled.path());
    assert_eq!(catalog.printers().len(), 1);
    let entry = &catalog.printers()[0];
    assert_eq!(entry.manufacturer, "UserMaker");
    assert_eq!(entry.source, ProfileSource::User);
    assert_eq!(entry.build_volume.x_mm(), 150.0);
}

#[test]
fn provenance_records_the_library_each_id_came_from() {
    let user = tempfile::tempdir().unwrap();
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "b-only",
        "Maker",
        "Bundled Only",
        (100.0, 60.0, 80.0),
        None,
    );
    write_printer(
        user.path(),
        "u-only",
        "Maker",
        "User Only",
        (100.0, 60.0, 80.0),
        None,
    );

    let catalog = ProfileCatalog::load(Some(user.path()), bundled.path());
    let bundled_entry = catalog
        .printers()
        .iter()
        .find(|e| e.id == printer_id("b-only"))
        .unwrap();
    let user_entry = catalog
        .printers()
        .iter()
        .find(|e| e.id == printer_id("u-only"))
        .unwrap();
    assert_eq!(bundled_entry.source, ProfileSource::Bundled);
    assert_eq!(user_entry.source, ProfileSource::User);
}

#[test]
fn manufacturers_are_sorted_and_deduplicated() {
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "z1",
        "Zeta",
        "One",
        (100.0, 60.0, 80.0),
        None,
    );
    write_printer(
        bundled.path(),
        "a1",
        "Alpha",
        "One",
        (100.0, 60.0, 80.0),
        None,
    );
    write_printer(
        bundled.path(),
        "a2",
        "Alpha",
        "Two",
        (100.0, 60.0, 80.0),
        None,
    );

    let catalog = ProfileCatalog::load(None, bundled.path());
    assert_eq!(
        catalog.manufacturers(),
        vec!["Alpha".to_owned(), "Zeta".to_owned()]
    );
}

#[test]
fn a_malformed_profile_is_skipped_without_dropping_the_valid_ones() {
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "good",
        "Maker",
        "Good",
        (100.0, 60.0, 80.0),
        None,
    );
    fs::write(
        bundled.path().join("printers/broken.toml"),
        "this is not valid printer toml",
    )
    .unwrap();

    let catalog = ProfileCatalog::load(None, bundled.path());
    assert_eq!(catalog.printers().len(), 1);
    assert_eq!(catalog.printers()[0].id, printer_id("good"));
}

#[test]
fn resin_has_preset_reflects_the_per_printer_blocks() {
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "printer-a",
        "Maker",
        "A",
        (100.0, 60.0, 80.0),
        None,
    );
    write_printer(
        bundled.path(),
        "printer-b",
        "Maker",
        "B",
        (100.0, 60.0, 80.0),
        None,
    );
    write_material(
        bundled.path(),
        "resin",
        "Brand",
        "Resin",
        "other",
        &["printer-a"],
    );

    let catalog = ProfileCatalog::load(None, bundled.path());
    assert!(catalog.resin_has_preset(&material_id("resin"), &printer_id("printer-a")));
    assert!(!catalog.resin_has_preset(&material_id("resin"), &printer_id("printer-b")));
    let entry = &catalog.materials()[0];
    assert_eq!(
        entry.calibrated_printers,
        [printer_id("printer-a")].into_iter().collect()
    );
}

#[test]
fn indices_locate_entries_by_id() {
    let bundled = tempfile::tempdir().unwrap();
    write_printer(
        bundled.path(),
        "a-printer",
        "Alpha",
        "A",
        (100.0, 60.0, 80.0),
        None,
    );
    write_printer(
        bundled.path(),
        "z-printer",
        "Zeta",
        "Z",
        (100.0, 60.0, 80.0),
        None,
    );
    write_material(bundled.path(), "a-resin", "Alpha", "A", "other", &[]);

    let catalog = ProfileCatalog::load(None, bundled.path());
    assert_eq!(catalog.printer_index(&printer_id("z-printer")), Some(1));
    assert_eq!(catalog.printer_index(&printer_id("missing")), None);
    assert_eq!(catalog.material_index(&material_id("a-resin")), Some(0));
    assert_eq!(catalog.material_index(&material_id("missing")), None);
}

#[test]
fn a_rebuild_picks_up_a_newly_written_user_profile() {
    let user = tempfile::tempdir().unwrap();
    let bundled = tempfile::tempdir().unwrap();
    write_material(bundled.path(), "shipped", "Brand", "Shipped", "other", &[]);

    let before = ProfileCatalog::load(Some(user.path()), bundled.path());
    assert_eq!(before.material_index(&material_id("authored")), None);

    write_material(user.path(), "authored", "Brand", "Authored", "other", &[]);
    let after = ProfileCatalog::load(Some(user.path()), bundled.path());
    let entry_index = after.material_index(&material_id("authored")).unwrap();
    assert_eq!(after.materials()[entry_index].source, ProfileSource::User);
}
