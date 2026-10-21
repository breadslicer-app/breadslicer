/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;
use std::path::Path;

use sc_print_primitives::{MaterialId, PrinterId};
use tempfile::TempDir;

use super::listing::{list_material_extension_ids, list_material_ids, list_printer_ids};

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, b"").unwrap();
}

#[test]
fn list_printer_ids_returns_sorted_ids_from_toml_files() {
    let dir = TempDir::new().unwrap();
    let printers = dir.path().join("printers");
    touch(&printers.join("elegoo-saturn-4-ultra.toml"));
    touch(&printers.join("anycubic-photon-mono.toml"));

    let ids = list_printer_ids(dir.path()).unwrap();
    assert_eq!(
        ids,
        vec![
            PrinterId::new("anycubic-photon-mono").unwrap(),
            PrinterId::new("elegoo-saturn-4-ultra").unwrap(),
        ],
    );
}

#[test]
fn list_material_ids_returns_sorted_ids_from_toml_files() {
    let dir = TempDir::new().unwrap();
    let materials = dir.path().join("materials");
    touch(&materials.join("sirayatech-blu.toml"));
    touch(&materials.join("elegoo-standard-grey.toml"));

    let ids = list_material_ids(dir.path()).unwrap();
    assert_eq!(
        ids,
        vec![
            MaterialId::new("elegoo-standard-grey").unwrap(),
            MaterialId::new("sirayatech-blu").unwrap(),
        ],
    );
}

#[test]
fn missing_subdir_yields_an_empty_list() {
    let dir = TempDir::new().unwrap();
    assert!(list_printer_ids(dir.path()).unwrap().is_empty());
    assert!(list_material_ids(dir.path()).unwrap().is_empty());
}

#[test]
fn non_toml_files_are_skipped() {
    let dir = TempDir::new().unwrap();
    let printers = dir.path().join("printers");
    touch(&printers.join("elegoo-saturn-4-ultra.toml"));
    touch(&printers.join("readme.md"));
    touch(&printers.join("notes.txt"));

    let ids = list_printer_ids(dir.path()).unwrap();
    assert_eq!(ids, vec![PrinterId::new("elegoo-saturn-4-ultra").unwrap()]);
}

#[test]
fn files_whose_stem_is_not_a_valid_id_are_skipped() {
    let dir = TempDir::new().unwrap();
    let printers = dir.path().join("printers");
    touch(&printers.join("elegoo-saturn-4-ultra.toml"));
    // Uppercase and underscores are outside the id grammar.
    touch(&printers.join("Not_A_Valid_Id.toml"));

    let ids = list_printer_ids(dir.path()).unwrap();
    assert_eq!(ids, vec![PrinterId::new("elegoo-saturn-4-ultra").unwrap()]);
}

#[test]
fn empty_subdir_yields_an_empty_list() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("printers")).unwrap();
    assert!(list_printer_ids(dir.path()).unwrap().is_empty());
}

#[test]
fn list_material_extension_ids_returns_sorted_ids_from_the_extensions_subdir() {
    let dir = TempDir::new().unwrap();
    let extensions = dir.path().join("materials").join("extensions");
    touch(&extensions.join("sirayatech-blu.toml"));
    touch(&extensions.join("elegoo-standard-grey.toml"));

    let ids = list_material_extension_ids(dir.path()).unwrap();
    assert_eq!(
        ids,
        vec![
            MaterialId::new("elegoo-standard-grey").unwrap(),
            MaterialId::new("sirayatech-blu").unwrap(),
        ],
    );
}

#[test]
fn missing_extensions_subdir_yields_an_empty_list() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("materials")).unwrap();
    assert!(list_material_extension_ids(dir.path()).unwrap().is_empty());
}

#[test]
fn material_and_extension_listings_do_not_leak_into_each_other() {
    let dir = TempDir::new().unwrap();
    touch(&dir.path().join("materials").join("plain-resin.toml"));
    touch(
        &dir.path()
            .join("materials")
            .join("extensions")
            .join("extended-resin.toml"),
    );

    let materials = list_material_ids(dir.path()).unwrap();
    assert_eq!(materials, vec![MaterialId::new("plain-resin").unwrap()]);

    let extensions = list_material_extension_ids(dir.path()).unwrap();
    assert_eq!(extensions, vec![MaterialId::new("extended-resin").unwrap()]);
}
