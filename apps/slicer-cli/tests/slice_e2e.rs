/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! End-to-end tests for the `slice` subcommand: feed in 1 of the committed STL
//! fixtures, ask for per-layer SVG, combined SVG, or per-layer PNG output, and
//! assert that the rendered topology or pixel pattern matches what we expect
//! from the fixture's geometry.

use std::fs;
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use assert_cmd::Command;
use predicates::prelude::*;
use sc_3mf::package::{Package, PartName};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_mesh_formats::filetypes::WriteFormat;
use sc_mesh_formats::scene_writer::write_scene;
use sc_print_primitives::{MaterialId, PrinterId};
use sc_project_formats::{EmbedProfiles, ProjectFileType, ProjectPayload, write_project};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use tempfile::tempdir;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Returns the directory of bundled test profiles, pointed at by the
/// `SC_SLICER_ASSETS_DIR` override so the by-id loader finds the fake fixture
/// profiles.
fn profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("profiles")
}

/// Returns the path to a fixture profile TOML file, for exercising the
/// load-by-path branch of the `--printer-profile` discriminator.
fn profile_file(kind: &str, name: &str) -> PathBuf {
    profiles_dir().join(kind).join(name)
}

/// Run `slice --format svg-per-layer` on the named fixture into `output_dir`.
/// Uses a fixed schedule that produces 5 layers for the cube (z in [0, 1]) and
/// 10 for the prisms (z in [0, 2]).
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

/// Run `slice --target-format svg-combined` on the named fixture into a single
/// SVG file at `output_file`. Same fixed schedule as
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

/// Run `slice --format png-per-layer` on the named fixture into `output_dir`.
/// Same fixed schedule as the SVG variants.
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

/// Sample a pixel at raster coords (X from left, Y from bottom of the build
/// plate). PNG rows go top-down so the raster Y is flipped here to map back
/// into the buffer.
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
    // Tunnel band z in [0.6, 1.4]. Layers passing through the tunnel split into
    // 2 disjoint rectangles; layers above and below the tunnel band are a
    // single solid rectangle.
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

    // Same indices as in the svg-per-layer test: layers 0-2 and 7-9 sit above
    // and below the tunnel band (solid); 3-6 cut through the tunnel (2 disjoint
    // rectangles).
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

    // Layer 0 cuts the cube at z = 0.1; the cube occupies printer-frame x, y in
    // [-0.5, +0.5] mm. The 5 mm plate maps to 500 px (100 px/mm) with the
    // envelope's min corner (-2.5, -2.5) at pixel (0, 0), so the cube renders
    // at pixels [200, 300]. Inside: pixel (250, 250) at printer mm (0, 0), the
    // plate origin and the raster centre. Outside: pixel (100, 100) at printer
    // mm (-1.5, -1.5), well below the cube.
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

    // Outer box printer-frame x, y in [-1, +1] mm, tunnel x, y in [-0.4, +0.4]
    // mm. At 100 px/mm with the envelope's min corner (-2.5, -2.5) at pixel
    // (0, 0), the outer renders at pixels [150, 350] and the tunnel at pixels
    // [210, 290]. In raster pixel coords:
    // - (170, 170) at printer mm (-0.8, -0.8): inside the outer ring, outside
    //   the tunnel -> filled (255).
    // - (250, 250) at printer mm (0, 0) (the plate origin, raster centre):
    //   inside the tunnel hole -> empty (0).
    // - (100, 100) at printer mm (-1.5, -1.5): outside the outer ring -> empty
    //   (0).
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
    // [-0.4, +0.4] (spanning all Y). At 100 px/mm with the envelope's min
    // corner (-2.5, -2.5) at pixel (0, 0): outer at pixels [150, 350], tunnel
    // slab at pixel columns [210, 290].

    // Layer 0 (z = 0.1, below tunnel): solid outer rect. Pixel (250, 250) at
    // the plate origin should be filled.
    let (w, h, below) = decode_grayscale_png(&layers[0]);
    assert_eq!(
        sample_at(&below, w, h, 250, 250),
        255,
        "below-tunnel layer should be solid at the plate origin",
    );

    // Layer 3 (z = 0.7, in tunnel): outer rect minus tunnel slab. Pixel
    // (250, 250) (the plate origin) sits inside the tunnel slab -> empty. Pixel
    // (170, 250) at printer mm (-0.8, 0) sits outside the tunnel slab but
    // inside the outer ring -> filled.
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
fn a_material_without_any_layer_height_fails_to_resolve() {
    // The incomplete fixture supplies neither an intent layer height nor
    // calibration: the only material parameter slicing needs is missing, so the
    // slice refuses.
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
            "no layer height resolves for this printer and material",
        ));
}

#[test]
fn a_material_without_print_calibration_slices_geometry() {
    // The extendable resin's base carries an intent layer height (0.2 mm) and
    // no per-printer calibration, and no extension is installed here. Geometry
    // slicing consumes the layer height only; the print calibration belongs to
    // the printer-file export step, so this pairing slices.
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
        .arg("slicer-cli-e2e-test-resin-extendable")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success()
        .stdout(predicate::str::contains("Sliced 1 model: 5 layers"));
}

/// Builds a temporary user library whose `materials/extensions/` holds the
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

/// Slice 1 fixture with the extendable resin while its same-id extension is
/// installed in a temporary user library, and assert the expected layer count.
/// Pins that the by-id route composes the extension without disturbing the
/// slice (extensions add print calibration, which geometry slicing does not
/// consume).
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
fn an_installed_extension_does_not_disturb_the_slice_of_the_cube() {
    slice_with_extension_calibration("cube.stl", 5);
}

#[test]
fn an_installed_extension_does_not_disturb_the_slice_of_the_z_tunnel_prism() {
    slice_with_extension_calibration("prism_z_tunnel.stl", 10);
}

#[test]
fn an_installed_extension_does_not_disturb_the_slice_of_the_y_tunnel_prism() {
    slice_with_extension_calibration("prism_y_tunnel.stl", 10);
}

#[test]
fn a_material_given_by_path_composes_the_library_extension() {
    // Same composition rule on both routes: a --material-profile PATH picks the
    // document, and the user library's same-id extension still composes over
    // it. Observable through the not-flagged exclusion warning, which only a
    // running composition can emit.
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
        .arg(profile_file("materials", "slicer-cli-e2e-test-resin.toml"))
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .env("SC_SLICER_LIBRARY_DIR", library.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Sliced 1 model: 5 layers"))
        .stderr(predicate::str::contains("not flagged as an extension"));
}

#[test]
fn an_invalid_extension_is_reported_and_does_not_break_the_slice() {
    // The base resin is fully calibrated on its own; the file in the library's
    // extensions slot is not flagged as an extension. The slice must proceed on
    // the base's values and surface the exclusion on stderr.
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

/// Builds a scene holding the origin-centred unit cube, translated to x, y in
/// [0.5, 1.5] and z in [5, 6]: off-centre and lifted, but inside the e2e
/// printer's 5 x 5 x 50 mm envelope, so it slices without placement.
fn lifted_cube_scene() -> Scene {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([1.0, 1.0, 5.5])),
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
    scene
}

/// Write `scene` as a project 3MF referencing the e2e profiles, returning the
/// raw package bytes.
fn project_bytes(scene: &Scene) -> Vec<u8> {
    let payload = ProjectPayload {
        printer: Some(PrinterId::new("slicer-cli-e2e-test").expect("valid slug")),
        resin: Some(MaterialId::new("slicer-cli-e2e-test-resin").expect("valid slug")),
        first_layer_height: None,
        output_format: None,
        embedded: Vec::new(),
        review_decisions: Vec::new(),
        plates: 0,
        plate_names: Vec::new(),
    };
    let mut bytes = Vec::new();
    write_project(
        scene,
        &payload,
        EmbedProfiles::No,
        None,
        None,
        ProjectFileType::ThreeMf,
        &mut Cursor::new(&mut bytes),
    )
    .expect("project fixture writes");
    bytes
}

/// Run `slice --target-format svg-per-layer` on `source` with the e2e profiles
/// plus `extra` arguments, returning the assertion handle for output checks.
fn slice_source_as_svg_per_layer(
    source: &Path,
    out_dir: &Path,
    extra: &[&str],
) -> assert_cmd::assert::Assert {
    let mut cmd = Command::cargo_bin("sc-slicer-cli").unwrap();
    cmd.arg("slice")
        .arg("--source")
        .arg(source)
        .arg("--target")
        .arg(out_dir)
        .arg("--target-format")
        .arg("svg-per-layer")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir());
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.assert()
}

#[test]
fn a_project_3mf_slices_where_it_was_saved() {
    // A project's placement is part of its saved content: the lifted cube (top
    // at z = 6) must slice to 30 layers at 0.2 mm, not be dropped to the plate
    // (5 layers).
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("lifted.3mf");
    fs::write(&source, project_bytes(&lifted_cube_scene())).unwrap();
    let out = tmp.path().join("layers");
    fs::create_dir(&out).unwrap();
    slice_source_as_svg_per_layer(&source, &out, &[])
        .success()
        .stdout(predicate::str::contains("Sliced 1 model: 30 layers"));
}

#[test]
fn no_auto_place_does_not_change_a_project() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("lifted.3mf");
    fs::write(&source, project_bytes(&lifted_cube_scene())).unwrap();
    let out = tmp.path().join("layers");
    fs::create_dir(&out).unwrap();
    slice_source_as_svg_per_layer(&source, &out, &["--no-auto-place"])
        .success()
        .stdout(predicate::str::contains("Sliced 1 model: 30 layers"));
}

#[test]
fn a_plain_3mf_still_auto_places() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("lifted-plain.3mf");
    write_scene(&lifted_cube_scene(), &source, WriteFormat::ThreeMf).expect("plain 3mf writes");
    let out = tmp.path().join("layers");
    fs::create_dir(&out).unwrap();
    slice_source_as_svg_per_layer(&source, &out, &[])
        .success()
        .stdout(predicate::str::contains("Sliced 1 model: 5 layers"));
}

#[test]
fn project_only_warnings_print_nothing() {
    // Foreign project data (here a Bambu/Orca marker part) is detected by the
    // project reader; `slice` ignores project data on purpose, so stderr must
    // stay silent about it.
    let tmp = tempdir().unwrap();
    let mut package = Package::read_from(Cursor::new(project_bytes(&lifted_cube_scene())))
        .expect("the project bytes reopen as a package");
    package
        .add_part(
            PartName::new("/Metadata/project_settings.config").expect("valid part name"),
            "application/json",
            b"{}".to_vec(),
        )
        .expect("the marker part is new to the package");
    let source = tmp.path().join("foreign.3mf");
    let mut file = File::create(&source).unwrap();
    package.write_to(&mut file).expect("the package rewrites");
    drop(file);
    let out = tmp.path().join("layers");
    fs::create_dir(&out).unwrap();
    slice_source_as_svg_per_layer(&source, &out, &[])
        .success()
        .stdout(predicate::str::contains("Sliced 1 model: 30 layers"))
        .stderr(predicate::str::is_empty());
}

// The GOO V3.0 offsets and constants below are the specification's, written out
// here instead of imported from the writer, so that a change on either side has
// to be made on both.

const GOO_HEADER_SIZE: usize = 195_477;
const GOO_MAGIC: [u8; 8] = [0x07, 0x00, 0x00, 0x00, 0x44, 0x4C, 0x50, 0x00];
const GOO_DELIMITER: [u8; 2] = [0x0D, 0x0A];
const GOO_FOOTER: [u8; 11] = [
    0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x44, 0x4C, 0x50, 0x00,
];
const GOO_PAYLOAD_MAGIC: u8 = 0x55;
const E2E_LAYER_HEIGHT_MM: f32 = 0.2;

fn be_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn be_f32(bytes: &[u8], at: usize) -> f32 {
    f32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Slice the cube fixture to `target`, naming the format explicitly only when
/// `format` carries one, and read back what was written.
fn slice_cube_to(target: &Path, format: Option<&str>) -> Vec<u8> {
    let mut command = Command::cargo_bin("sc-slicer-cli").unwrap();
    command
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(target)
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir());
    if let Some(format) = format {
        command.arg("--target-format").arg(format);
    }
    command.assert().success();
    fs::read(target).unwrap()
}

/// Walk the layer section by the format's own stride rule, returning 1 offset
/// per layer record and failing unless the walk lands exactly on the footer.
fn goo_layer_offsets(bytes: &[u8]) -> Vec<usize> {
    let footer_at = bytes.len() - GOO_FOOTER.len();
    let mut offsets = Vec::new();
    let mut at = GOO_HEADER_SIZE;
    while at < footer_at {
        assert!(at + 70 <= footer_at, "a record at {at} runs past the file");
        assert_eq!(
            bytes[at + 64..at + 66],
            GOO_DELIMITER,
            "record delimiter at {at}",
        );
        let data_size = be_u32(bytes, at + 66) as usize;
        assert!(
            at + 72 + data_size <= bytes.len(),
            "the payload at {at} runs past the file",
        );
        assert_eq!(
            bytes[at + 70 + data_size..at + 72 + data_size],
            GOO_DELIMITER,
            "payload delimiter at {at}",
        );
        offsets.push(at);
        at += 72 + data_size;
    }
    assert_eq!(at, footer_at, "the layer walk missed the footer");
    offsets
}

#[test]
fn a_goo_target_writes_a_file_that_walks_onto_its_footer() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.goo");
    let bytes = slice_cube_to(&target, Some("goo"));

    assert_eq!(&bytes[0..4], b"V3.0");
    assert_eq!(bytes[4..12], GOO_MAGIC);
    assert_eq!(bytes[27_106..27_108], GOO_DELIMITER);
    assert_eq!(bytes[195_308..195_310], GOO_DELIMITER);
    assert_eq!(be_u32(&bytes, 195_470) as usize, GOO_HEADER_SIZE);
    assert_eq!(bytes[bytes.len() - GOO_FOOTER.len()..], GOO_FOOTER);

    assert_eq!(goo_layer_offsets(&bytes).len(), 5);
    assert_eq!(be_u32(&bytes, 195_310), 5, "declared layer count");
}

#[test]
fn every_goo_payload_carries_its_own_checksum() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.goo");
    let bytes = slice_cube_to(&target, Some("goo"));

    for at in goo_layer_offsets(&bytes) {
        let data_size = be_u32(&bytes, at + 66) as usize;
        let payload = &bytes[at + 70..at + 70 + data_size];
        assert_eq!(payload[0], GOO_PAYLOAD_MAGIC, "payload magic at {at}");
        let sum = payload[1..payload.len() - 1]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        assert_eq!(payload[payload.len() - 1], !sum, "checksum at {at}");
    }
}

#[test]
fn the_goo_records_carry_the_schedule_the_slice_ran() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.goo");
    let bytes = slice_cube_to(&target, Some("goo"));

    let thickness = be_f32(&bytes, 195_332);
    assert!(
        (thickness - E2E_LAYER_HEIGHT_MM).abs() < 1e-6,
        "header thickness {thickness}",
    );
    for (index, at) in goo_layer_offsets(&bytes).into_iter().enumerate() {
        let slab_top = E2E_LAYER_HEIGHT_MM * (index + 1) as f32;
        let z = be_f32(&bytes, at + 6);
        assert!(
            (z - slab_top).abs() < 1e-5,
            "layer {index} sits at {z}, expected the slab top {slab_top}",
        );
    }
}

#[test]
fn a_requested_layer_height_reaches_the_goo_header_and_records() {
    // The e2e resin's own layer height is 0.2 mm, so 10 layers of 0.1 mm show
    // that the request reached both the slicing and the print configuration.
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.goo");
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(&target)
        .arg("--layer-height")
        .arg("0.1")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success();
    let bytes = fs::read(&target).unwrap();

    let offsets = goo_layer_offsets(&bytes);
    assert_eq!(offsets.len(), 10);
    assert_eq!(be_u32(&bytes, 195_310), 10, "declared layer count");
    let thickness = be_f32(&bytes, 195_332);
    assert!(
        (thickness - 0.1).abs() < 1e-6,
        "header thickness {thickness}"
    );
    for (index, at) in offsets.into_iter().enumerate() {
        let slab_top = 0.1 * (index + 1) as f32;
        let z = be_f32(&bytes, at + 6);
        assert!(
            (z - slab_top).abs() < 1e-5,
            "layer {index} sits at {z}, expected the slab top {slab_top}",
        );
    }
}

#[test]
fn a_dot_goo_target_needs_no_explicit_format() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.goo");
    let bytes = slice_cube_to(&target, None);

    assert_eq!(&bytes[0..4], b"V3.0");
    assert_eq!(goo_layer_offsets(&bytes).len(), 5);
}

#[test]
fn a_material_without_print_calibration_cannot_be_written_as_goo() {
    // The same pairing slices geometry (see
    // `a_material_without_print_calibration_slices_geometry`): a printer file
    // additionally needs the exposures, which this resin carries for no
    // printer.
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.goo");
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(&target)
        .arg("--target-format")
        .arg("goo")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin-extendable")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "could not resolve the print configuration",
        ));
    assert!(!target.exists(), "a refused export writes no file");
}

#[test]
fn an_uncalibrated_resin_reports_what_it_left_to_the_resolver() {
    // The e2e resin names the 7 values resolution requires and none of the
    // optional ones, so every optional value is a fallback.
    let dir = tempdir().unwrap();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(dir.path().join("plate.goo"))
        .arg("--target-format")
        .arg("goo")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "warning: 12 values for this print were chosen by no profile:",
        ))
        .stderr(predicate::str::contains(
            "  wait_after_cure_sec: no calibrated value, using 0 (no pause)",
        ))
        .stderr(predicate::str::contains(
            "  bottom_light_pwm: no calibrated value, using 100 (full power)",
        ))
        .stderr(predicate::str::contains(
            "  cured_density_g_per_cm3: no published figure, using 1.17 (a typical photopolymer)",
        ));
}

#[test]
fn a_fully_calibrated_resin_reports_nothing_and_still_writes() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.goo");
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(&target)
        .arg("--target-format")
        .arg("goo")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin-calibrated")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let bytes = fs::read(&target).unwrap();
    assert_eq!(goo_layer_offsets(&bytes).len(), 5);
}

#[test]
fn a_dot_ctb_target_takes_its_revision_from_the_printer() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.ctb");
    let bytes = slice_cube_to(&target, None);

    assert_eq!(&bytes[0..4], &[0x06, 0x01, 0xFD, 0x12], "the plain magic");
    assert_eq!(
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
        5,
        "the revision the printer declares",
    );
    assert_eq!(
        u32::from_le_bytes(bytes[68..72].try_into().unwrap()),
        5,
        "declared layer count",
    );
}

#[test]
fn naming_the_ctb_family_writes_the_same_file_as_the_extension_does() {
    let dir = tempdir().unwrap();
    let by_flag = slice_cube_to(&dir.path().join("flag.ctb"), Some("ctb"));
    let by_spelling = slice_cube_to(&dir.path().join("spelled.ctb"), Some("ctb@v5"));

    assert_eq!(by_flag.len(), by_spelling.len());
    assert_eq!(&by_flag[0..8], &by_spelling[0..8]);
}

#[test]
fn a_revision_without_a_writer_is_refused_by_name() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(dir.path().join("plate.goo"))
        .arg("--target-format")
        .arg("goo@v5.1")
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .failure()
        .stderr(predicate::str::contains("goo@v5.1"));
}

/// Names the encrypted revision for a printer that declares the plain one,
/// which is the case the spelling exists for.
#[test]
fn spelling_the_encrypted_ctb_revision_overrides_the_printer() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("plate.ctb");
    let bytes = slice_cube_to(&target, Some("ctb@v5enc"));

    assert_eq!(
        &bytes[0..4],
        &[0x07, 0x01, 0xFD, 0x12],
        "the encrypted magic",
    );
    assert_eq!(
        u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
        5,
        "the revision named on the flag",
    );

    let signature_at = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
    let signature_size = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
    assert_eq!(
        bytes.len(),
        signature_at + signature_size + 4,
        "the signature and its checksum close the file",
    );
}

#[test]
fn a_family_the_printer_does_not_declare_is_refused() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(dir.path().join("plate.goo"))
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test-no-goo")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .failure()
        .stderr(predicate::str::contains("declares no \"goo\" output"));
}

#[test]
fn a_target_extension_no_writer_claims_is_reported() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(fixture_path("cube.stl"))
        .arg("--target")
        .arg(dir.path().join("plate.xyz"))
        .arg("--printer-profile")
        .arg("slicer-cli-e2e-test")
        .arg("--material-profile")
        .arg("slicer-cli-e2e-test-resin")
        .env("SC_SLICER_ASSETS_DIR", profiles_dir())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "could not infer target format from path",
        ));
}
