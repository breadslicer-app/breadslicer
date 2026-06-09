/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! End-to-end tests for the `profiles` subcommand, driven against
//! the fake fixture library under `tests/fixtures/profiles/` via the
//! `SC_SLICER_LIBRARY_DIR` override.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

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
