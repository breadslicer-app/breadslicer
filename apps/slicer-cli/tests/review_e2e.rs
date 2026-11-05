/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! End-to-end tests for the `review` subcommand: slice a committed fixture
//! without writing a file and list the pre-flight findings, as text and as
//! JSON.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// The fake fixture profiles, found by id through `SC_SLICER_ASSETS_DIR`.
fn profiles_dir() -> PathBuf {
    fixture_path("profiles")
}

fn review_cube(extra: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("review")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .args(extra)
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
}

#[test]
fn review_prints_the_summary_and_the_time_spent() {
    review_cube(&[])
        .success()
        .stdout(predicate::str::contains(
            "Reviewed 1 leaf (1 model, 0 supports) over 5 layers:",
        ))
        .stdout(predicate::str::contains("Layer regions: 5 solid"))
        .stdout(predicate::str::contains("Review took "));
}

#[test]
fn review_writes_json_on_request() {
    let output = review_cube(&["--json"]).success().get_output().clone();
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(document["layers"], 5);
    assert_eq!(document["leaves"].as_array().unwrap().len(), 1);
    assert!(document["findings"].is_array());
    assert_eq!(
        document["summary"]["findings"],
        document["findings"].as_array().unwrap().len()
    );
    assert_eq!(document["regions"]["solid"], 5);
    assert!(document["timings_ms"]["sweep"].is_number());
    assert!(document["islands"].is_array());
    assert!(document["parts"].is_array());
    assert!(document["erased"].is_array());
}

#[test]
fn review_fills_on_request() {
    // The cube has no cavity: nothing to fill, and the review says so.
    let output = review_cube(&["--json", "--fill"])
        .success()
        .get_output()
        .clone();
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert!(document["filled"].is_null());
    review_cube(&["--fill"])
        .success()
        .stdout(predicate::str::contains("Filled ").not());
}

#[test]
fn review_writes_no_file() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .current_dir(dir.path())
        .arg("review")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
