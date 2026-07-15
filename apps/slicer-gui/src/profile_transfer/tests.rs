/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fs;
use std::path::Path;

use sc_print_profiles::{CollisionReport, ExportSelection};

use super::*;

const PRINTER_TOML: &str = "\
[identity]
id = \"test-printer\"
manufacturer = \"SoulCrafted\"
family = \"Test\"
name = \"Test Printer\"
profile_version = \"1.0.0\"

[hardware]
build_volume = { x_mm = 5.0, y_mm = 5.0, z_mm = 50.0 }
pixel_resolution = { x = 500, y = 500 }
pixel_size_microns = { x = 10.0, y = 10.0 }
mirror = { x = false, y = false }

[capabilities]
output_formats = [{ format = \"png_per_layer\" }]
supports_grayscale = true
two_stage_lift = false
has_camera = false

[constraints]
";

const MATERIAL_TOML: &str = "\
[identity]
id = \"test-resin\"
brand = \"SoulCrafted\"
family = \"Standard\"
name = \"Test Resin\"
profile_version = \"1.0.0\"

[intent]

[defaults]
";

fn write_library(dir: &Path) {
    fs::create_dir_all(dir.join("printers")).unwrap();
    fs::create_dir_all(dir.join("materials")).unwrap();
    fs::write(dir.join("printers/test-printer.toml"), PRINTER_TOML).unwrap();
    fs::write(dir.join("materials/test-resin.toml"), MATERIAL_TOML).unwrap();
}

#[test]
fn export_scope_maps_to_the_matching_selection() {
    assert_eq!(ExportScope::Printers.selection(), ExportSelection::Printers);
    assert_eq!(
        ExportScope::Materials.selection(),
        ExportSelection::Materials
    );
}

#[test]
fn overwrite_confirmed_only_for_the_overwrite_button() {
    assert!(overwrite_confirmed(
        &MessageDialogResult::Custom("Overwrite".to_owned()),
        "Overwrite"
    ));
    assert!(!overwrite_confirmed(
        &MessageDialogResult::Custom("Cancel".to_owned()),
        "Overwrite"
    ));
    assert!(!overwrite_confirmed(
        &MessageDialogResult::Cancel,
        "Overwrite"
    ));
    assert!(!overwrite_confirmed(&MessageDialogResult::Ok, "Overwrite"));
}

#[test]
fn export_then_import_round_trips_into_a_fresh_library() {
    let source = tempfile::tempdir().unwrap();
    write_library(source.path());
    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("all.zip");

    run_export(source.path(), &ExportSelection::All, &archive).unwrap();

    let dest = tempfile::tempdir().unwrap();
    match run_import(dest.path(), &archive, false).unwrap() {
        ImportOutcome::Installed => {}
        ImportOutcome::Collisions(_) => panic!("fresh library should not collide"),
    }
    assert!(dest.path().join("printers/test-printer.toml").is_file());
    assert!(dest.path().join("materials/test-resin.toml").is_file());
}

#[test]
fn export_printers_scope_omits_materials() {
    let source = tempfile::tempdir().unwrap();
    write_library(source.path());
    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("printers.zip");

    run_export(source.path(), &ExportScope::Printers.selection(), &archive).unwrap();

    let dest = tempfile::tempdir().unwrap();
    run_import(dest.path(), &archive, false).unwrap();
    assert!(dest.path().join("printers/test-printer.toml").is_file());
    assert!(!dest.path().join("materials/test-resin.toml").exists());
}

#[test]
fn re_importing_reports_collisions_then_force_overwrites() {
    let source = tempfile::tempdir().unwrap();
    write_library(source.path());
    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("all.zip");
    run_export(source.path(), &ExportSelection::All, &archive).unwrap();

    let dest = tempfile::tempdir().unwrap();
    run_import(dest.path(), &archive, false).unwrap();

    // Second import without force must surface the collisions.
    let report = match run_import(dest.path(), &archive, false).unwrap() {
        ImportOutcome::Collisions(report) => report,
        ImportOutcome::Installed => panic!("expected a collision on re-import"),
    };
    assert!(!report.printers.is_empty() || !report.materials.is_empty());

    // Forcing the import installs it despite the collision.
    match run_import(dest.path(), &archive, true).unwrap() {
        ImportOutcome::Installed => {}
        ImportOutcome::Collisions(_) => panic!("force import must not report collisions"),
    }
}

#[test]
fn collision_report_lists_the_conflicting_ids() {
    let report = CollisionReport {
        printers: vec![Collision {
            id: "test-printer".to_owned(),
            library_path: "/lib/printers/test-printer.toml".into(),
        }],
        materials: Vec::new(),
    };
    let text = format_collision_report(&report);
    assert!(text.contains("Printers:"), "got: {text}");
    assert!(text.contains("test-printer"), "got: {text}");
    assert!(!text.contains("Materials:"), "got: {text}");
}
