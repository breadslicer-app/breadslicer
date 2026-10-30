/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! Pins which way the bundled Photon Mono M7 Pro and Halot One profiles turn a
//! part in the files they write, so a mirrored or turned print cannot come
//! back silently.
//!
//! The layout is the one of Lychee, whose files print right on the owner's
//! machines (#277, 2026-10-30). Breadslicer's and Lychee's `.pwsz` of one
//! Wolverine scene for the M7 Pro match on 99.8 % of the lit pixels, the rest
//! being edge pixels. Until then Breadslicer laid the plate out half a turn
//! round: unmirrored, which PR #142 confirmed against Photon Workshop, but a
//! part at the plate's front left landed at its back right.
//!
//! Lychee's `.cxdlp` is its `.pwsz` of the same scene transposed: it lays the
//! Halot One's plate out 128 mm wide and 81 mm deep, as the machine stands,
//! and its file's 2560-pixel axis runs along the plate's X. Since #134 the
//! Halot One's profile does the same (its panel stores the plate turned a
//! quarter turn counter-clockwise), and Breadslicer's pair relates the way
//! Lychee's does. A part on the plate's +X, +Y side therefore sits at the last
//! rows and last columns of both files: in the `.pwsz` the rows follow the
//! plate's Y and the columns its X; in the `.cxdlp` the rows follow the
//! plate's X and the columns its Y.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use sc_sla_formats_uvtools::read::{read_anycubic_zip, read_cxdlp};
use tempfile::tempdir;

fn bundled_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("profiles")
}

/// Appends an axis-aligned box as 12 triangles of an ASCII STL.
fn push_box(stl: &mut String, min: [f32; 3], max: [f32; 3]) {
    let corner = |i: usize| {
        [
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        ]
    };
    // Each face as 2 triangles wound outwards.
    let faces: [[usize; 4]; 6] = [
        [0, 2, 3, 1], // -Z
        [4, 5, 7, 6], // +Z
        [0, 1, 5, 4], // -Y
        [2, 6, 7, 3], // +Y
        [0, 4, 6, 2], // -X
        [1, 3, 7, 5], // +X
    ];
    for [a, b, c, d] in faces {
        for tri in [[a, b, c], [a, c, d]] {
            stl.push_str("facet normal 0 0 0\nouter loop\n");
            for i in tri {
                let [x, y, z] = corner(i);
                stl.push_str(&format!("vertex {x} {y} {z}\n"));
            }
            stl.push_str("endloop\nendfacet\n");
        }
    }
}

/// A 10 x 10 mm block and, on its +X, +Y diagonal, a 3 x 3 mm marker, both
/// 0.1 mm tall: 2 layers at the materials' 50 µm.
fn write_marked_plate(dir: &Path) -> PathBuf {
    let mut stl = String::from("solid marked\n");
    push_box(&mut stl, [0.0, 0.0, 0.0], [10.0, 10.0, 0.1]);
    push_box(&mut stl, [13.0, 13.0, 0.0], [16.0, 16.0, 0.1]);
    stl.push_str("endsolid marked\n");
    let path = dir.join("marked.stl");
    fs::write(&path, stl).unwrap();
    path
}

fn slice(source: &Path, target: &Path, printer: &str, material: &str) {
    Command::cargo_bin("sc-slicer-cli")
        .unwrap()
        .arg("slice")
        .arg("--source")
        .arg(source)
        .arg("--target")
        .arg(target)
        .arg("--printer-profile")
        .arg(printer)
        .arg("--material-profile")
        .arg(material)
        .env("SC_SLICER_ASSETS_DIR", bundled_profiles_dir())
        .assert()
        .success();
}

/// The lit pixels' centroid `(column, row)` of each connected blob, split at
/// the larger gap: returns `(block, marker)` by lit area.
fn block_and_marker(image: &[u8], width: usize) -> ((f64, f64), (f64, f64)) {
    let lit: Vec<(usize, usize)> = image
        .iter()
        .enumerate()
        .filter(|&(_, &v)| v >= 128)
        .map(|(i, _)| (i % width, i / width))
        .collect();
    assert!(!lit.is_empty(), "the layer is dark");
    // The block and the marker are diagonal neighbours with a gap between
    // them, so the lit pixels split by their distance to the 2 extreme ones.
    let first = lit[0];
    let last = lit[lit.len() - 1];
    let dist = |a: (usize, usize), b: (usize, usize)| {
        let dx = a.0 as f64 - b.0 as f64;
        let dy = a.1 as f64 - b.1 as f64;
        dx * dx + dy * dy
    };
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for &pixel in &lit {
        if dist(pixel, first) <= dist(pixel, last) {
            a.push(pixel);
        } else {
            b.push(pixel);
        }
    }
    let centroid = |pixels: &[(usize, usize)]| {
        let n = pixels.len() as f64;
        (
            pixels.iter().map(|p| p.0 as f64).sum::<f64>() / n,
            pixels.iter().map(|p| p.1 as f64).sum::<f64>() / n,
        )
    };
    if a.len() >= b.len() {
        (centroid(&a), centroid(&b))
    } else {
        (centroid(&b), centroid(&a))
    }
}

#[test]
fn the_m7_pro_pwsz_puts_the_plates_plus_x_plus_y_corner_at_the_bottom_right() {
    let dir = tempdir().unwrap();
    let source = write_marked_plate(dir.path());
    let target = dir.path().join("marked.pwsz");
    slice(
        &source,
        &target,
        "anycubic-photon-mono-m7-pro",
        "anycubic-standard-resin-grey-50um",
    );

    let file = read_anycubic_zip(BufReader::new(File::open(&target).unwrap())).unwrap();
    assert_eq!(file.settings["machine_type"]["res_x"], 13312);
    let (block, marker) = block_and_marker(&file.layers[0], 13312);
    assert!(
        marker.0 > block.0,
        "+X must land on the last columns, marker {marker:?} block {block:?}",
    );
    assert!(
        marker.1 > block.1,
        "+Y must land on the last rows, marker {marker:?} block {block:?}",
    );
}

#[test]
fn the_halot_one_cxdlp_puts_the_plates_plus_x_plus_y_corner_at_the_bottom_right() {
    let dir = tempdir().unwrap();
    let source = write_marked_plate(dir.path());
    let target = dir.path().join("marked.cxdlp");
    slice(
        &source,
        &target,
        "creality-halot-one",
        "creality-halot-one-printer-profile-50um",
    );

    let file = read_cxdlp(&fs::read(&target).unwrap()).unwrap();
    assert_eq!(file.version, 2, "the Halot One's default is CXDLP v2");
    // The panel, as Lychee's files name it: the plate is 128 x 81 mm.
    assert_eq!((file.resolution_x, file.resolution_y), (1620, 2560));
    assert_eq!(
        (file.display_width.as_str(), file.display_height.as_str()),
        ("81", "128")
    );
    let (block, marker) = block_and_marker(&file.layers[0], usize::from(file.resolution_x));
    assert!(
        marker.1 > block.1,
        "+X must land on the last rows, marker {marker:?} block {block:?}",
    );
    assert!(
        marker.0 > block.0,
        "+Y must land on the last columns, marker {marker:?} block {block:?}",
    );
}
