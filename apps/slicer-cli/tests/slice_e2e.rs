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

/// Run `slice --format svg-combined` on the named fixture into a
/// single SVG file at `output_file`. Same fixed schedule as
/// [`slice_fixture_as_svg_per_layer`].
fn slice_fixture_as_svg_combined(fixture: &str, output_file: &Path) {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--input")
        .arg(fixture_path(fixture))
        .arg("--output")
        .arg(output_file)
        .arg("--format")
        .arg("svg-combined")
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

/// Run `slice --format png-per-layer` on the named fixture into
/// `output_dir`. Same fixed schedule as the SVG variants.
fn slice_fixture_as_png_per_layer(fixture: &str, output_dir: &Path) {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--input")
        .arg(fixture_path(fixture))
        .arg("--output")
        .arg(output_dir)
        .arg("--format")
        .arg("png-per-layer")
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
        content.contains("<sc:build-volume x-mm=\"5\" y-mm=\"5\" z-mm=\"1\"/>"),
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

    // Layer 0 cuts the cube at z = 0.1; the cube occupies x, y in
    // [-0.5, 0.5] mm. The raster spans x, y in [0, 5] mm (pixels
    // [0, 500]); only the upper-right quadrant of the cube falls
    // inside the raster, but that quadrant still contains pixels
    // strictly inside the polygon (e.g. mm 0.1, 0.1 -> raster
    // pixel 10, 10) and strictly outside it (e.g. mm 2.0, 2.0 ->
    // raster pixel 200, 200).
    let (w, h, buf) = decode_grayscale_png(&layers[0]);
    assert_eq!(
        sample_at(&buf, w, h, 10, 10),
        255,
        "raster pixel (10, 10) at mm (0.1, 0.1) sits inside the cube",
    );
    assert_eq!(
        sample_at(&buf, w, h, 200, 200),
        0,
        "raster pixel (200, 200) at mm (2.0, 2.0) sits outside the cube",
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

    // Outer box x, y in [-1, 1] mm, tunnel x, y in [-0.4, 0.4] mm.
    // Raster covers x, y in [0, 5] mm. In raster pixel coords:
    // - (70, 70) is at mm (0.7, 0.7) -> outside the tunnel, inside
    //   the outer box -> filled (255).
    // - (20, 20) is at mm (0.2, 0.2) -> inside the tunnel -> empty (0).
    // - (200, 200) is at mm (2.0, 2.0) -> outside the outer box ->
    //   empty (0).
    let (w, h, buf) = decode_grayscale_png(&layers[0]);
    assert_eq!(
        sample_at(&buf, w, h, 70, 70),
        255,
        "raster pixel (70, 70) at mm (0.7, 0.7) is inside the outer ring and outside the tunnel",
    );
    assert_eq!(
        sample_at(&buf, w, h, 20, 20),
        0,
        "raster pixel (20, 20) at mm (0.2, 0.2) sits inside the tunnel hole",
    );
    assert_eq!(
        sample_at(&buf, w, h, 200, 200),
        0,
        "raster pixel (200, 200) at mm (2.0, 2.0) sits outside the outer ring",
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

    // Layer 0 (z = 0.1, below tunnel): solid 2x2 outer rect. Pixel
    // (20, 20) at mm (0.2, 0.2) should be filled.
    let (w, h, below) = decode_grayscale_png(&layers[0]);
    assert_eq!(
        sample_at(&below, w, h, 20, 20),
        255,
        "below-tunnel layer should be solid at mm (0.2, 0.2)",
    );

    // Layer 3 (z = 0.7, in tunnel): outer rect minus tunnel slab x
    // in [-0.4, 0.4]. Pixel (20, 20) at mm (0.2, 0.2) is inside the
    // tunnel slab -> empty. Pixel (70, 20) at mm (0.7, 0.2) is
    // outside the tunnel slab -> filled.
    let (_, _, through) = decode_grayscale_png(&layers[3]);
    assert_eq!(
        sample_at(&through, w, h, 20, 20),
        0,
        "through-tunnel layer should be empty at mm (0.2, 0.2)",
    );
    assert_eq!(
        sample_at(&through, w, h, 70, 20),
        255,
        "through-tunnel layer should be filled at mm (0.7, 0.2) (outside the tunnel slab)",
    );
}
