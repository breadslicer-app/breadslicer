/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! End-to-end tests for the `orient` subcommand: the options with their
//! breakdown, as text and as JSON, and the model written in its new pose.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn orient(extra: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("orient")
        .arg(fixture_path("prism_z_tunnel.stl"))
        .args(extra)
        .assert()
}

#[test]
fn orient_prints_the_options_and_the_time_per_stage() {
    orient(&[])
        .success()
        .stdout(predicate::str::contains("Option 1"))
        .stdout(predicate::str::contains("Support tips"))
        .stdout(predicate::str::contains("Peak peel force"))
        .stdout(predicate::str::contains("Recommended: "))
        .stdout(predicate::str::contains("stage 1"))
        .stdout(predicate::str::contains("final verdict"));
}

#[test]
fn orient_writes_json_on_request() {
    let output = orient(&["--json", "--preset", "lower-force"])
        .success()
        .get_output()
        .clone();
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(document["preset"], "lower-force");
    assert_eq!(document["closed"], true);
    let options = document["options"].as_array().expect("options");
    assert!(!options.is_empty() && options.len() <= 3);
    assert!(options[0]["cost"]["peakForceN"].is_number());
    assert!(document["timingsMs"]["search"].is_number());
}

#[test]
fn orient_fits_a_build_volume_and_writes_the_pose() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("oriented.stl");
    let output = orient(&[
        "--json",
        "--build-volume",
        "100,100,100",
        "-o",
        out.to_str().unwrap(),
    ])
    .success()
    .get_output()
    .clone();
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["options"][0]["cost"]["fit"]["fits"], true);
    assert!(out.is_file());
    // The written model, oriented again, stays where it is.
    let again = Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("orient")
        .arg(&out)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .clone();
    let again: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
    let kept =
        again["keepCurrent"] == true || again["options"][0]["tiltDeg"].as_f64().unwrap() <= 2.0;
    assert!(kept, "{again}");
}

#[test]
fn orient_refuses_an_unknown_preset() {
    orient(&["--preset", "fastest"])
        .failure()
        .stderr(predicate::str::contains("unknown preset"));
}
