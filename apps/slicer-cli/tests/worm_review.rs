/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The review on a real heavy scene (#432), against what its printer file
//! holds: a 171 mm hollowed model, 3536 layers of a 13312 x 5120 screen, with
//! 3057 support tips, sticks and twigs among them.
//!
//! Ignored: it needs that project, named by `BREADSLICER_WORM`, and the
//! release build to be quick.
//!
//! ```text
//! BREADSLICER_WORM=<path>/Worm_repaired_hollowed.bread \
//!     cargo test --release -p sc-slicer-cli --test main worm_review -- --ignored --nocapture
//! ```
//!
//! Layer numbers here are as the app shows them, from 1.

use std::process::Command;
use std::time::Instant;

use serde_json::Value;

/// Parts that start with no pixel of a tip under them and last: what
/// supports must go under.
const ISLANDS: [u64; 1] = [942];

/// Parts a tip holds. On 232 to 1501 only the last slice of a cone shows on
/// the layer right below, 5 to 40 pixels of it: any pixel holds.
const HELD: [u64; 11] = [
    232, 660, 1230, 1264, 1422, 1444, 1470, 1486, 1501, 1515, 1526,
];

/// Screen pixels of the parts that come loose, each on its first layer, the
/// floating speck at 2262 among them. All of them leave the layers, as debris
/// or, the ones a pixel thin, as stray pixels.
const LOOSE: [(u64, u32, u32); 36] = [
    (1119, 3275, 2355),
    (957, 5698, 3041),
    (1524, 4178, 4085),
    (938, 6540, 2875),
    (1049, 7117, 1922),
    (1444, 1882, 2770),
    (243, 4108, 2706),
    (1850, 11319, 2123),
    (1850, 11217, 1925),
    (1112, 4389, 1975),
    (1851, 11360, 2209),
    (2651, 7373, 2762),
    (1048, 7117, 1919),
    (364, 4211, 2835),
    (861, 3400, 3622),
    (987, 5340, 1871),
    (987, 5339, 1873),
    (987, 5338, 1875),
    (987, 5337, 1877),
    (1020, 5084, 3050),
    (1021, 5088, 3043),
    (1300, 7777, 2073),
    (1328, 4921, 3626),
    (1749, 2025, 3420),
    (1858, 11447, 2422),
    (2159, 9242, 3581),
    (2262, 9341, 3691),
    (2533, 9276, 2356),
    (2534, 9283, 2376),
    (2534, 9329, 2465),
    (2535, 9298, 2411),
    (2535, 9299, 2413),
    (2535, 9300, 2415),
    (2535, 9301, 2417),
    (2993, 8033, 2774),
    (2994, 8003, 2722),
];

const LAYER_MM: f64 = 0.05;
const PIXEL_MM: [f64; 2] = [0.0168, 0.0248];
const PLATE_MM: [f64; 2] = [223.6416, 126.976];

/// The middle of raster pixel `(x, y)` of this mirrored screen, in plate
/// millimetres.
fn plate_point(x: u32, y: u32) -> [f64; 2] {
    [
        0.5 * PLATE_MM[0] - (f64::from(x) + 0.5) * PIXEL_MM[0],
        (f64::from(y) + 0.5) * PIXEL_MM[1] - 0.5 * PLATE_MM[1],
    ]
}

/// Whether what the review erased on `layer` has a rectangle within a few
/// pixels of `point`: a part the sweep cleans goes pixel by pixel, and the
/// raster point of a one-pixel part names it from a neighbour.
fn erased_near(erased: &[Value], layer: u64, point: [f64; 2]) -> bool {
    const REACH_MM: f64 = 0.1;
    let on_layer = erased.iter().find(|slab| {
        let bottom = slab["bottom_mm"].as_f64().expect("a height");
        (bottom / LAYER_MM).round() as u64 + 1 == layer
    });
    on_layer.is_some_and(|slab| {
        slab["rects"]
            .as_array()
            .expect("rectangles")
            .iter()
            .any(|rect| {
                let r: Vec<f64> = rect
                    .as_array()
                    .expect("a rectangle")
                    .iter()
                    .map(|v| v.as_f64().expect("a number"))
                    .collect();
                r[0] - REACH_MM <= point[0]
                    && point[0] <= r[2] + REACH_MM
                    && r[1] - REACH_MM <= point[1]
                    && point[1] <= r[3] + REACH_MM
            })
    })
}

/// The question after Slice answered Yes (#437): every island gets a tip
/// under its first pixels, or as soon as it has grown to the tip's diameter,
/// and the review of the new slice lists none.
#[test]
#[ignore = "needs a real project, named by BREADSLICER_WORM"]
fn a_yes_to_the_question_leaves_the_heavy_scene_without_islands() {
    let project = std::env::var("BREADSLICER_WORM").expect("BREADSLICER_WORM names the project");
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_sc-slicer-cli"))
        .args([
            "review",
            &format!("-s={project}"),
            "--printer-profile=anycubic-photon-mono-m7-pro",
            "--material-profile=anycubic-abs-like-resin-pro-50um",
            "--no-auto-place",
            "--add-supports",
            "--json",
        ])
        .output()
        .expect("the review runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("whole run: {:?}", started.elapsed());
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let added = &report["added_supports"];
    eprintln!("cycle: {}", added["timings_ms"]);
    eprintln!("tips: {}", added["tips"]);
    eprintln!("diameter: {}", added["diameter_mm"]);

    let islands = report["islands"].as_array().expect("islands");
    let tips = added["tips"].as_array().expect("tips");
    assert!(tips.iter().all(|tip| tip["refused"].is_null()), "{tips:?}");
    for (index, island) in islands.iter().enumerate() {
        let first = island["first_layer"].as_u64().expect("a layer");
        let lowest = tips
            .iter()
            .filter(|tip| tip["island"].as_u64() == Some(index as u64))
            .filter_map(|tip| tip["layer"].as_u64())
            .min()
            .expect("a tip on every island");
        assert!(lowest >= first && lowest - first < 8, "{first}: {lowest}");
    }
    assert_eq!(added["too_narrow"].as_array().map(Vec::len), Some(0));
    assert_eq!(added["islands"].as_array().map(Vec::len), Some(0));
    let held = added["parts"].as_array().expect("parts").iter().any(|part| {
        part["first_layer"].as_u64() == Some(ISLANDS[0] - 1) && part["held"] == "tip"
    });
    assert!(held, "a tip holds what was an island");
}

#[test]
#[ignore = "needs a real project, named by BREADSLICER_WORM"]
fn the_review_of_the_heavy_scene_matches_its_printer_file() {
    let project = std::env::var("BREADSLICER_WORM").expect("BREADSLICER_WORM names the project");
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_sc-slicer-cli"))
        .args([
            "review",
            &format!("-s={project}"),
            "--printer-profile=anycubic-photon-mono-m7-pro",
            "--material-profile=anycubic-abs-like-resin-pro-50um",
            "--no-auto-place",
            "--json",
        ])
        .output()
        .expect("the review runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("open, slice and review: {:?}", started.elapsed());
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    eprintln!("timings: {}", report["timings_ms"]);

    let islands: Vec<u64> = report["islands"]
        .as_array()
        .expect("islands")
        .iter()
        .map(|island| island["first_layer"].as_u64().expect("a layer") + 1)
        .collect();
    assert_eq!(islands, ISLANDS);

    let parts = report["parts"].as_array().expect("parts");
    for first in HELD {
        let part = parts
            .iter()
            .find(|part| part["first_layer"].as_u64() == Some(first - 1) && part["held"] == "tip");
        assert!(part.is_some(), "the part of layer {first} is held");
    }

    let erased = report["erased"].as_array().expect("what left the layers");
    for (layer, x, y) in LOOSE {
        let point = plate_point(x, y);
        assert!(
            erased_near(erased, layer, point),
            "the part at ({x}, {y}) of layer {layer} is gone"
        );
    }
}
