/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! End-to-end tests for the `slice` subcommand: feed in one of the
//! committed STL fixtures, ask for per-layer SVG output, and assert
//! that the per-layer polygon topology matches what we expect from
//! the fixture's geometry.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::tempdir;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Run `slice --format svg-per-layer` on the named fixture into
/// `output_dir`. Uses a fixed schedule that produces 5 layers for
/// the cube (z in [0, 1]) and 10 for the prisms (z in [0, 2]).
fn slice_fixture_as_svg_per_layer(fixture: &str, output_dir: &Path) {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--input")
        .arg(fixture_path(fixture))
        .arg("--output")
        .arg(output_dir)
        .arg("--format")
        .arg("svg-per-layer")
        .arg("--layer-height")
        .arg("0.2")
        .arg("--build-x")
        .arg("5")
        .arg("--build-y")
        .arg("5")
        .arg("--pixels-x")
        .arg("500")
        .arg("--pixels-y")
        .arg("500")
        .assert()
        .success();
}

fn layer_files(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "svg"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn cube_fixture_produces_one_solid_polygon_per_layer() {
    let out = tempdir().unwrap();
    slice_fixture_as_svg_per_layer("cube.stl", out.path());

    let layers = layer_files(out.path());
    assert_eq!(
        layers.len(),
        5,
        "expected 5 layers for the cube (z in [0, 1], layer-height 0.2)",
    );

    for path in &layers {
        let content = fs::read_to_string(path).unwrap();
        assert_eq!(
            content.matches("<path d=").count(),
            1,
            "{}: expected one <path>, got:\n{content}",
            path.display(),
        );
        assert_eq!(
            content.matches("M ").count(),
            1,
            "{}: expected one M command (single solid polygon), got:\n{content}",
            path.display(),
        );
    }
}

#[test]
fn z_tunnel_prism_yields_polygon_with_hole_in_every_layer() {
    let out = tempdir().unwrap();
    slice_fixture_as_svg_per_layer("prism_z_tunnel.stl", out.path());

    let layers = layer_files(out.path());
    assert_eq!(
        layers.len(),
        10,
        "expected 10 layers for the z-tunnel prism (z in [0, 2], layer-height 0.2)",
    );

    for path in &layers {
        let content = fs::read_to_string(path).unwrap();
        assert_eq!(
            content.matches("<path d=").count(),
            1,
            "{}: outer ring and hole pack into a single <path> via the evenodd rule, got:\n{content}",
            path.display(),
        );
        assert_eq!(
            content.matches("M ").count(),
            2,
            "{}: expected outer + one hole (two M commands), got:\n{content}",
            path.display(),
        );
        assert!(
            content.contains("fill-rule=\"evenodd\""),
            "{}: polygon-with-hole must use the evenodd fill rule, got:\n{content}",
            path.display(),
        );
    }
}

#[test]
fn y_tunnel_prism_topology_transitions_above_below_and_through_the_tunnel() {
    let out = tempdir().unwrap();
    slice_fixture_as_svg_per_layer("prism_y_tunnel.stl", out.path());

    let layers = layer_files(out.path());
    assert_eq!(
        layers.len(),
        10,
        "expected 10 layers for the y-tunnel prism (z in [0, 2], layer-height 0.2)",
    );

    // Schedule slice planes: 0.1, 0.3, 0.5, 0.7, 0.9, 1.1, 1.3, 1.5, 1.7, 1.9.
    // Tunnel band z in [0.6, 1.4]. Layers passing through the tunnel
    // split into two disjoint rectangles; layers above and below the
    // tunnel band are a single solid rectangle.
    let solid_layer_indices: &[usize] = &[0, 1, 2, 7, 8, 9];
    let through_tunnel_layer_indices: &[usize] = &[3, 4, 5, 6];

    for &idx in solid_layer_indices {
        let content = fs::read_to_string(&layers[idx]).unwrap();
        assert_eq!(
            content.matches("<path d=").count(),
            1,
            "layer {idx} expected one solid path (above/below tunnel), got:\n{content}",
        );
        assert_eq!(
            content.matches("M ").count(),
            1,
            "layer {idx} expected one M command (solid rectangle), got:\n{content}",
        );
    }

    for &idx in through_tunnel_layer_indices {
        let content = fs::read_to_string(&layers[idx]).unwrap();
        assert_eq!(
            content.matches("<path d=").count(),
            2,
            "layer {idx} expected two paths (tunnel splits the layer into two rectangles), got:\n{content}",
        );
        assert_eq!(
            content.matches("M ").count(),
            2,
            "layer {idx} expected two M commands (two disjoint rectangles), got:\n{content}",
        );
    }
}
