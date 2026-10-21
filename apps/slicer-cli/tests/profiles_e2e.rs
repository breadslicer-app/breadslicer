/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! End-to-end tests for the `profiles` subcommand, driven against
//! the fake fixture library under `tests/fixtures/profiles/` via the
//! `SC_SLICER_LIBRARY_DIR` override.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn library_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("profiles")
}

fn profiles_list(args: &[&str]) -> assert_cmd::assert::Assert {
    let mut cmd = Command::cargo_bin("sc-slicer-cli").unwrap();
    cmd.arg("profiles").arg("list").args(args);
    cmd.env("SC_SLICER_LIBRARY_DIR", library_dir());
    cmd.assert()
}

fn profiles_export(args: &[&str]) -> assert_cmd::assert::Assert {
    let mut cmd = Command::cargo_bin("sc-slicer-cli").unwrap();
    cmd.arg("profiles").arg("export").args(args);
    cmd.env("SC_SLICER_LIBRARY_DIR", library_dir());
    cmd.assert()
}

fn profiles_import_into(library_dir: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut cmd = Command::cargo_bin("sc-slicer-cli").unwrap();
    cmd.arg("profiles").arg("import").args(args);
    cmd.env("SC_SLICER_LIBRARY_DIR", library_dir);
    cmd.assert()
}

/// The first four bytes of a non-empty local zip entry: the "PK\x03\x04"
/// local file header signature.
fn is_zip(path: &Path) -> bool {
    std::fs::read(path)
        .map(|bytes| bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04]))
        .unwrap_or(false)
}

#[test]
fn list_all_shows_the_printer_then_the_materials() {
    profiles_list(&[])
        .success()
        .stdout(predicate::str::contains(
            "slicer-cli-e2e-test\tE2E Test Printer\t5 x 5 x 50 mm, 500 x 500 px",
        ))
        .stdout(predicate::str::contains(
            "slicer-cli-e2e-test-resin\tE2E Test Resin\tSoulCrafted, E2E",
        ));
}

#[test]
fn list_printers_shows_only_the_printer() {
    profiles_list(&["printers"])
        .success()
        .stdout(predicate::str::contains(
            "slicer-cli-e2e-test\tE2E Test Printer\t5 x 5 x 50 mm, 500 x 500 px",
        ))
        .stdout(predicate::str::contains("E2E Test Resin").not());
}

#[test]
fn list_materials_shows_only_the_materials() {
    profiles_list(&["materials"])
        .success()
        .stdout(predicate::str::contains(
            "slicer-cli-e2e-test-resin\tE2E Test Resin\tSoulCrafted, E2E",
        ))
        .stdout(predicate::str::contains("E2E Test Printer").not());
}

#[test]
fn list_rejects_an_unknown_kind() {
    profiles_list(&["sprockets"]).failure();
}

#[test]
fn export_writes_a_zip_archive() {
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("bundle.zip");
    profiles_export(&[zip_path.to_str().unwrap()])
        .success()
        .stdout(predicate::str::contains("Exported profiles to"));
    assert!(is_zip(&zip_path), "expected a zip archive at {zip_path:?}");
}

#[test]
fn export_by_id_writes_a_zip_archive() {
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("bundle.zip");
    profiles_export(&["--id", "slicer-cli-e2e-test", zip_path.to_str().unwrap()]).success();
    assert!(is_zip(&zip_path), "expected a zip archive at {zip_path:?}");
}

#[test]
fn export_rejects_id_combined_with_kind_flag() {
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("bundle.zip");
    profiles_export(&[
        "--printers",
        "--id",
        "slicer-cli-e2e-test",
        zip_path.to_str().unwrap(),
    ])
    .failure();
}

#[test]
fn import_installs_then_reports_collisions_then_force_overwrites() {
    let pkg = TempDir::new().unwrap();
    let zip_path = pkg.path().join("bundle.zip");
    let zip_arg = zip_path.to_str().unwrap();
    profiles_export(&[zip_arg]).success();

    // First import into an empty library succeeds and installs the
    // fixture printer.
    let dest = TempDir::new().unwrap();
    profiles_import_into(dest.path(), &[zip_arg])
        .success()
        .stdout(predicate::str::contains("Imported profiles into"));
    assert!(
        dest.path()
            .join("printers")
            .join("slicer-cli-e2e-test.toml")
            .is_file(),
    );

    // Re-importing without --force collides and reports the conflict.
    profiles_import_into(dest.path(), &[zip_arg])
        .failure()
        .stderr(predicate::str::contains("would overwrite"))
        .stderr(predicate::str::contains("slicer-cli-e2e-test"));

    // --force lets it overwrite.
    profiles_import_into(dest.path(), &["--force", zip_arg]).success();
}
