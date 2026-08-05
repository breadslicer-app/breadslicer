/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! End-to-end tests for the `slice` subcommand: feed in one of the
//! committed STL fixtures, ask for per-layer SVG, combined SVG, or
//! per-layer PNG output, and assert that the rendered topology or
//! pixel pattern matches what we expect from the fixture's geometry.

use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Directory of bundled test profiles, pointed at by the
/// `SC_SLICER_ASSETS_DIR` override so the by-id loader finds the
/// fake fixture profiles.
fn profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("profiles")
}

/// Path to a fixture profile TOML file, for exercising the
/// load-by-path branch of the `--printer-profile` discriminator.
fn profile_file(kind: &str, name: &str) -> PathBuf {
    profiles_dir().join(kind).join(name)
}

/// Run `slice --format svg-per-layer` on the named fixture into
/// `output_dir`. Uses a fixed schedule that produces 5 layers for
/// the cube (z in [0, 1]) and 10 for the prisms (z in [0, 2]).
fn slice_fixture_as_svg_per_layer(fixture: &str, output_dir: &Path) {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path(fixture))
        .arg("--target")
        .arg(output_dir)
        .arg("--target-format")
        .arg("svg-per-layer")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success();
}

/// Run `slice --target-format svg-combined` on the named fixture
/// into a single SVG file at `output_file`. Same fixed schedule as
/// [`slice_fixture_as_svg_per_layer`].
fn slice_fixture_as_svg_combined(fixture: &str, output_file: &Path) {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path(fixture))
        .arg("--target")
        .arg(output_file)
        .arg("--target-format")
        .arg("svg-combined")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success();
}

/// Run `slice --format png-per-layer` on the named fixture into
/// `output_dir`. Same fixed schedule as the SVG variants.
fn slice_fixture_as_png_per_layer(fixture: &str, output_dir: &Path) {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path(fixture))
        .arg("--target")
        .arg(output_dir)
        .arg("--target-format")
        .arg("png-per-layer")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success();
}

/// Decode an 8-bit grayscale PNG into (width, height, pixels).
fn decode_grayscale_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(BufReader::new(File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    assert_eq!(info.color_type, png::ColorType::Grayscale);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    (info.width, info.height, buffer)
}

/// Sample a pixel at raster coords (X from left, Y from bottom of
/// the build plate). PNG rows go top-down so the raster Y is
/// flipped here to map back into the buffer.
fn sample_at(pixels: &[u8], width: u32, height: u32, raster_x: u32, raster_y: u32) -> u8 {
    let png_row = height - 1 - raster_y;
    pixels[(png_row as usize) * (width as usize) + (raster_x as usize)]
}

/// Split a combined SVG into the inner substring of each
/// `<g id="layer-NNNN" ...>...</g>` block, in document order.
fn layer_groups(content: &str) -> Vec<&str> {
    content
        .split("<g id=\"layer-")
        .skip(1)
        .map(|chunk| {
            chunk
                .split("</g>")
                .next()
                .expect("each <g> opens and closes")
        })
        .collect()
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

#[test]
fn cube_fixture_svg_combined_packs_every_layer_into_a_single_file() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("cube.svg");
    slice_fixture_as_svg_combined("cube.stl", &out);

    assert!(out.is_file());
    let content = fs::read_to_string(&out).unwrap();
    let groups = layer_groups(&content);
    assert_eq!(groups.len(), 5, "expected 5 layer groups, got:\n{content}");
    assert!(content.contains("<metadata>"), "{content}");
    assert!(
        content.contains("<sc:build-volume x-mm=\"5\" y-mm=\"5\" z-mm=\"50\"/>"),
        "{content}"
    );
    assert!(content.contains("viewBox=\"-2.5 -2.5 5 5\""), "{content}");

    for (idx, group) in groups.iter().enumerate() {
        assert_eq!(
            group.matches("<path d=").count(),
            1,
            "group {idx} expected one solid path, got:\n{group}",
        );
        assert_eq!(
            group.matches("M ").count(),
            1,
            "group {idx} expected one M command, got:\n{group}",
        );
    }
}

#[test]
fn z_tunnel_prism_svg_combined_carries_polygon_with_hole_in_every_group() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("z_tunnel.svg");
    slice_fixture_as_svg_combined("prism_z_tunnel.stl", &out);

    let content = fs::read_to_string(&out).unwrap();
    let groups = layer_groups(&content);
    assert_eq!(
        groups.len(),
        10,
        "expected 10 layer groups, got:\n{content}"
    );
    assert!(
        content.contains("<sc:layer-schedule kind=\"uniform\" layer-height-mm=\"0.2\" first-layer-height-mm=\"0.2\" total-layers=\"10\"/>"),
        "{content}",
    );

    for (idx, group) in groups.iter().enumerate() {
        assert_eq!(
            group.matches("<path d=").count(),
            1,
            "group {idx} expected one <path> (outer + hole packed via evenodd), got:\n{group}",
        );
        assert_eq!(
            group.matches("M ").count(),
            2,
            "group {idx} expected two M commands (outer + hole), got:\n{group}",
        );
        assert!(
            group.contains("fill-rule=\"evenodd\""),
            "group {idx} missing evenodd fill rule, got:\n{group}",
        );
    }
}

#[test]
fn y_tunnel_prism_svg_combined_groups_match_per_layer_topology() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("y_tunnel.svg");
    slice_fixture_as_svg_combined("prism_y_tunnel.stl", &out);

    let content = fs::read_to_string(&out).unwrap();
    let groups = layer_groups(&content);
    assert_eq!(
        groups.len(),
        10,
        "expected 10 layer groups, got:\n{content}"
    );

    // Same indices as in the svg-per-layer test: layers 0-2 and 7-9
    // sit above and below the tunnel band (solid); 3-6 cut through
    // the tunnel (two disjoint rectangles).
    let solid_layer_indices: &[usize] = &[0, 1, 2, 7, 8, 9];
    let through_tunnel_layer_indices: &[usize] = &[3, 4, 5, 6];

    for &idx in solid_layer_indices {
        let group = groups[idx];
        assert_eq!(
            group.matches("<path d=").count(),
            1,
            "group {idx} expected one solid path, got:\n{group}",
        );
        assert_eq!(
            group.matches("M ").count(),
            1,
            "group {idx} expected one M command, got:\n{group}",
        );
    }
    for &idx in through_tunnel_layer_indices {
        let group = groups[idx];
        assert_eq!(
            group.matches("<path d=").count(),
            2,
            "group {idx} expected two paths, got:\n{group}",
        );
        assert_eq!(
            group.matches("M ").count(),
            2,
            "group {idx} expected two M commands, got:\n{group}",
        );
    }
}

fn sorted_png_files(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn cube_fixture_png_per_layer_emits_8bit_grayscale_500x500_per_layer() {
    let dir = tempdir().unwrap();
    slice_fixture_as_png_per_layer("cube.stl", dir.path());

    let layers = sorted_png_files(dir.path());
    assert_eq!(layers.len(), 5, "expected 5 PNG layers for the cube");

    for path in &layers {
        let (w, h, buf) = decode_grayscale_png(path);
        assert_eq!(w, 500, "{}: width", path.display());
        assert_eq!(h, 500, "{}: height", path.display());
        assert_eq!(
            buf.len(),
            (w * h) as usize,
            "{}: buffer size",
            path.display()
        );
    }

    // Layer 0 cuts the cube at z = 0.1; the cube occupies printer-
    // frame x, y in [-0.5, +0.5] mm. The 5 mm plate maps to 500 px
    // (100 px/mm) with the envelope's min corner (-2.5, -2.5) at
    // pixel (0, 0), so the cube renders at pixels [200, 300].
    // Inside: pixel (250, 250) at printer mm (0, 0), the plate
    // origin and the raster centre. Outside: pixel (100, 100) at
    // printer mm (-1.5, -1.5), well below the cube.
    let (w, h, buf) = decode_grayscale_png(&layers[0]);
    assert_eq!(
        sample_at(&buf, w, h, 250, 250),
        255,
        "raster pixel (250, 250) at the plate origin sits inside the cube",
    );
    assert_eq!(
        sample_at(&buf, w, h, 100, 100),
        0,
        "raster pixel (100, 100) at printer mm (-1.5, -1.5) sits outside the cube",
    );
}

#[test]
fn z_tunnel_prism_png_per_layer_carries_the_inner_hole_pixels() {
    let dir = tempdir().unwrap();
    slice_fixture_as_png_per_layer("prism_z_tunnel.stl", dir.path());

    let layers = sorted_png_files(dir.path());
    assert_eq!(
        layers.len(),
        10,
        "expected 10 PNG layers for the z-tunnel prism"
    );

    for path in &layers {
        let (w, h, _) = decode_grayscale_png(path);
        assert_eq!(w, 500);
        assert_eq!(h, 500);
    }

    // Outer box printer-frame x, y in [-1, +1] mm, tunnel x, y in
    // [-0.4, +0.4] mm. At 100 px/mm with the envelope's min corner
    // (-2.5, -2.5) at pixel (0, 0), the outer renders at pixels
    // [150, 350] and the tunnel at pixels [210, 290]. In raster
    // pixel coords:
    // - (170, 170) at printer mm (-0.8, -0.8): inside the outer
    //   ring, outside the tunnel -> filled (255).
    // - (250, 250) at printer mm (0, 0) (the plate origin, raster
    //   centre): inside the tunnel hole -> empty (0).
    // - (100, 100) at printer mm (-1.5, -1.5): outside the outer
    //   ring -> empty (0).
    let (w, h, buf) = decode_grayscale_png(&layers[0]);
    assert_eq!(
        sample_at(&buf, w, h, 170, 170),
        255,
        "raster pixel (170, 170) is inside the outer ring and outside the tunnel",
    );
    assert_eq!(
        sample_at(&buf, w, h, 250, 250),
        0,
        "raster pixel (250, 250) at the plate origin sits inside the tunnel hole",
    );
    assert_eq!(
        sample_at(&buf, w, h, 100, 100),
        0,
        "raster pixel (100, 100) at printer mm (-1.5, -1.5) sits outside the outer ring",
    );
}

#[test]
fn y_tunnel_prism_png_per_layer_transitions_above_below_and_through_the_tunnel() {
    let dir = tempdir().unwrap();
    slice_fixture_as_png_per_layer("prism_y_tunnel.stl", dir.path());

    let layers = sorted_png_files(dir.path());
    assert_eq!(
        layers.len(),
        10,
        "expected 10 PNG layers for the y-tunnel prism"
    );

    // Outer box printer-frame x, y in [-1, +1] mm, tunnel slab x in
    // [-0.4, +0.4] (spanning all Y). At 100 px/mm with the
    // envelope's min corner (-2.5, -2.5) at pixel (0, 0): outer at
    // pixels [150, 350], tunnel slab at pixel columns [210, 290].

    // Layer 0 (z = 0.1, below tunnel): solid outer rect. Pixel
    // (250, 250) at the plate origin should be filled.
    let (w, h, below) = decode_grayscale_png(&layers[0]);
    assert_eq!(
        sample_at(&below, w, h, 250, 250),
        255,
        "below-tunnel layer should be solid at the plate origin",
    );

    // Layer 3 (z = 0.7, in tunnel): outer rect minus tunnel slab.
    // Pixel (250, 250) (the plate origin) sits inside the tunnel
    // slab -> empty. Pixel (170, 250) at printer mm (-0.8, 0) sits
    // outside the tunnel slab but inside the outer ring -> filled.
    let (_, _, through) = decode_grayscale_png(&layers[3]);
    assert_eq!(
        sample_at(&through, w, h, 250, 250),
        0,
        "through-tunnel layer should be empty at the plate origin (inside the tunnel slab)",
    );
    assert_eq!(
        sample_at(&through, w, h, 170, 250),
        255,
        "through-tunnel layer should be filled at printer mm (-0.8, 0) (outside the tunnel slab)",
    );
}

#[test]
fn printer_profile_by_explicit_path_slices_successfully() {
    let out = tempdir().unwrap();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(out.path())
        .arg("--target-format")
        .arg("svg-per-layer")
        .arg("--printer-profile")
        .arg(profile_file("printers", "slicer-cli-e2e-test.toml"))
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success()
        .stdout(predicate::str::contains("Sliced 1 model: 5 layers"));
}

#[test]
fn unknown_material_id_fails_with_not_found() {
    let out = tempdir().unwrap();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(out.path())
        .arg("--target-format")
        .arg("svg-per-layer")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("no-such-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "not found in any of the configured sources",
        ));
}

#[test]
fn incomplete_material_fails_to_resolve() {
    let out = tempdir().unwrap();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(out.path())
        .arg("--target-format")
        .arg("svg-per-layer")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin-incomplete")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "could not resolve the print configuration",
        ));
}

/// A temporary user library whose `materials/extensions/` holds the
/// extendable resin's extension fixture, for `SC_SLICER_LIBRARY_DIR`.
fn library_with_extendable_extension() -> tempfile::TempDir {
    let library = tempdir().unwrap();
    let extensions_dir = library.path().join("materials").join("extensions");
    fs::create_dir_all(&extensions_dir).unwrap();
    fs::copy(
        profile_file("extensions", "slicer-cli-e2e-test-resin-extendable.toml"),
        extensions_dir.join("slicer-cli-e2e-test-resin-extendable.toml"),
    )
    .unwrap();
    library
}

/// Slice one fixture with the extendable resin (calibration only in
/// the user-library extension) and assert the expected layer count.
fn slice_with_extension_calibration(fixture: &str, expected_layers: u32) {
    let out = tempdir().unwrap();
    let library = library_with_extendable_extension();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path(fixture))
        .arg("--target")
        .arg(out.path())
        .arg("--target-format")
        .arg("svg-per-layer")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin-extendable")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .env("SC_SLICER_LIBRARY_DIR", library.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Sliced 1 model: {expected_layers} layers",
        )));
}

#[test]
fn extension_calibration_completes_the_resolution_for_the_cube() {
    // The base profile alone cannot resolve (no calibration block);
    // the same-id extension in the user library must supply it.
    slice_with_extension_calibration("cube.stl", 5);
}

#[test]
fn extension_calibration_completes_the_resolution_for_the_z_tunnel_prism() {
    slice_with_extension_calibration("prism_z_tunnel.stl", 10);
}

#[test]
fn extension_calibration_completes_the_resolution_for_the_y_tunnel_prism() {
    slice_with_extension_calibration("prism_y_tunnel.stl", 10);
}

#[test]
fn an_invalid_extension_is_reported_and_does_not_break_the_slice() {
    // The base resin is fully calibrated on its own; the file in the
    // library's extensions slot is not flagged as an extension. The
    // slice must proceed on the base's values and surface the
    // exclusion on stderr.
    let out = tempdir().unwrap();
    let library = tempdir().unwrap();
    let extensions_dir = library.path().join("materials").join("extensions");
    fs::create_dir_all(&extensions_dir).unwrap();
    fs::write(
        extensions_dir.join("slicer-cli-e2e-test-resin.toml"),
        r#"
[identity]
id = "slicer-cli-e2e-test-resin"
brand = "SoulCrafted"
family = "E2E"
name = "E2E Test Resin"
profile_version = "1.0.0"

[intent]

[defaults]

[per_printer."some-other-printer"]
normal_exposure_sec = 9.0
"#,
    )
    .unwrap();

    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(out.path())
        .arg("--target-format")
        .arg("svg-per-layer")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .env("SC_SLICER_LIBRARY_DIR", library.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Sliced 1 model: 5 layers"))
        .stderr(predicate::str::contains("not flagged as an extension"));
}
