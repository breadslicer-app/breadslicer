/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tests of the island pieces: the cut at weak joints, checked by hand on
//! small shapes and against a brute-force reference on pseudo-random ones,
//! and the contacts between layers.

use super::*;

/// The run list of the pixels where `rows` has `#`, row 0 first.
fn runs(rows: &[&str]) -> Vec<Run> {
    let mut out = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        let bytes = row.as_bytes();
        let mut x = 0;
        while x < bytes.len() {
            if bytes[x] == b'#' {
                let start = x;
                while x < bytes.len() && bytes[x] == b'#' {
                    x += 1;
                }
                out.push(Run::new(y as u32, start as u32, x as u32));
            } else {
                x += 1;
            }
        }
    }
    out
}

/// The pieces of `solid`, labelled into regions first as a layer is.
fn cut_of(solid: &[Run]) -> (Vec<Run>, Vec<u32>, usize) {
    let (regions, count) = label_runs(solid, Connectivity::Eight);
    cut(solid, &regions, count)
}

/// The pieces of `rows` drawn back: each pixel shows its piece as a letter.
fn drawn(rows: &[&str]) -> Vec<String> {
    let solid = runs(rows);
    let (cut_runs, labels, _) = cut_of(&solid);
    let mut out: Vec<Vec<u8>> = rows.iter().map(|row| vec![b'.'; row.len()]).collect();
    for (run, &label) in cut_runs.iter().zip(&labels) {
        for x in run.x0..run.x1 {
            out[run.row as usize][x as usize] = b'A' + label as u8;
        }
    }
    out.into_iter()
        .map(|row| String::from_utf8(row).unwrap())
        .collect()
}

#[test]
fn a_block_is_one_piece() {
    let rows = ["#####", "#####", "#####", "#####"];
    assert_eq!(drawn(&rows), ["AAAAA", "AAAAA", "AAAAA", "AAAAA"]);
}

#[test]
fn blocks_meeting_at_a_corner_are_two_pieces() {
    let rows = [
        "#####......", //
        "#####......", //
        "#####......", //
        "#####......", //
        "#####......", //
        ".....######", //
        ".....######", //
        ".....######", //
        ".....######", //
        ".....######", //
    ];
    assert_eq!(
        drawn(&rows),
        [
            "AAAAA......", //
            "AAAAA......", //
            "AAAAA......", //
            "AAAAA......", //
            "AAAAA......", //
            ".....BBBBBB", //
            ".....BBBBBB", //
            ".....BBBBBB", //
            ".....BBBBBB", //
            ".....BBBBBB", //
        ]
    );
}

#[test]
fn a_one_pixel_neck_cuts_and_stays_with_the_first_piece() {
    let rows = [
        "#####.....#####", //
        "#####.....#####", //
        "###############", //
        "#####.....#####", //
        "#####.....#####", //
    ];
    assert_eq!(
        drawn(&rows),
        [
            "AAAAA.....BBBBB", //
            "AAAAA.....BBBBB", //
            "AAAAAAAAAABBBBB", //
            "AAAAA.....BBBBB", //
            "AAAAA.....BBBBB", //
        ]
    );
}

#[test]
fn a_two_pixel_neck_cuts_but_a_three_pixel_one_holds() {
    let two = [
        "#####....#####", //
        "#####....#####", //
        "##############", //
        "##############", //
        "#####....#####", //
        "#####....#####", //
    ];
    let (_, _, count) = cut_of(&runs(&two));
    assert_eq!(count, 2, "{:#?}", drawn(&two));
    let three = [
        "#####....#####", //
        "##############", //
        "##############", //
        "##############", //
        "#####....#####", //
    ];
    let (_, _, count) = cut_of(&runs(&three));
    assert_eq!(count, 1, "{:#?}", drawn(&three));
}

#[test]
fn parts_under_five_pixels_thick_are_not_cut() {
    // Blocks 4 pixels thick on a one-pixel neck, a strip 3 pixels tall that
    // narrows to 2, and a diagonal brace 3 to 4 pixels wide: each is thin
    // all along, so each is one piece.
    let blocks = [
        "####.....####", //
        "####.....####", //
        "#############", //
        "####.....####", //
    ];
    assert_eq!(cut_of(&runs(&blocks)).2, 1, "{:#?}", drawn(&blocks));
    let strip = [
        "########....########", //
        "####################", //
        "####################", //
    ];
    assert_eq!(cut_of(&runs(&strip)).2, 1, "{:#?}", drawn(&strip));
    let brace = [
        "###.........", //
        "####........", //
        ".####.......", //
        "..####......", //
        "...###......", //
        "....####....", //
        ".....###....", //
        "......####..", //
        ".......####.", //
        "........####", //
    ];
    assert_eq!(cut_of(&runs(&brace)).2, 1, "{:#?}", drawn(&brace));
}

#[test]
fn thin_parts_join_the_piece_they_touch_or_stand_alone() {
    // A hair one pixel wide hanging off a block joins it; a two-pixel stem
    // touching nothing thick is a piece of its own.
    let rows = [
        "######.......", //
        "######.......", //
        "#########....", //
        "######.....##", //
        "######.....##", //
        "######.......", //
    ];
    assert_eq!(
        drawn(&rows),
        [
            "AAAAAA.......", //
            "AAAAAA.......", //
            "AAAAAAAAA....", //
            "AAAAAA.....BB", //
            "AAAAAA.....BB", //
            "AAAAAA.......", //
        ]
    );
}

/// A pseudo-random `width` x `height` pixel set, about `percent` % lit.
fn random_rows(seed: u64, width: usize, height: usize, percent: u64) -> Vec<String> {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..height)
        .map(|_| {
            (0..width)
                .map(|_| {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    if (state >> 33) % 100 < percent {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect()
}

fn lit(rows: &[String], x: i64, y: i64) -> bool {
    y >= 0
        && x >= 0
        && (y as usize) < rows.len()
        && (x as usize) < rows[y as usize].len()
        && rows[y as usize].as_bytes()[x as usize] == b'#'
}

#[test]
fn the_cut_matches_a_brute_force_opening() {
    for seed in 0..60 {
        let rows = random_rows(seed, 24, 18, 55 + seed % 35);
        let strs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let solid = runs(&strs);
        // Erosion by brute force.
        let eroded = erode(&solid);
        let mut expected = Vec::new();
        for y in 0..rows.len() as i64 {
            for x in 0..24i64 {
                let all = (-1..=1).all(|dy| (-1..=1).all(|dx| lit(&rows, x + dx, y + dy)));
                if all {
                    expected.push((x as u32, y as u32));
                }
            }
        }
        let got: Vec<(u32, u32)> = eroded
            .iter()
            .flat_map(|run| (run.x0..run.x1).map(move |x| (x, run.row)))
            .collect();
        let mut got_sorted = got.clone();
        got_sorted.sort_unstable_by_key(|&(x, y)| (y, x));
        expected.sort_unstable_by_key(|&(x, y)| (y, x));
        assert_eq!(got_sorted, expected, "seed {seed}");

        // The cut covers the solid exactly once; eroded pixels that touch
        // share a piece; every pixel next to an eroded pixel is in a piece
        // that has one (a core's piece), any other is thin; and every piece
        // is 8-connected.
        let (cut_runs, labels, count) = cut_of(&solid);
        let mut piece_of = vec![vec![u32::MAX; 24]; rows.len()];
        for (run, &label) in cut_runs.iter().zip(&labels) {
            assert!((label as usize) < count);
            for x in run.x0..run.x1 {
                let slot = &mut piece_of[run.row as usize][x as usize];
                assert_eq!(*slot, u32::MAX, "seed {seed}: pixel cut twice");
                *slot = label;
            }
        }
        for (y, row) in piece_of.iter().enumerate() {
            for (x, &piece) in row.iter().enumerate() {
                assert_eq!(
                    lit(&rows, x as i64, y as i64),
                    piece != u32::MAX,
                    "seed {seed}: ({x}, {y})"
                );
            }
        }
        let piece_at = |(x, y): (u32, u32)| piece_of[y as usize][x as usize];
        let mut core_pieces = vec![false; count];
        for &pixel in &expected {
            core_pieces[piece_at(pixel) as usize] = true;
        }
        // Pieces that touch each other are both anchors: they hold a pixel
        // of the erosion by a 5 x 5 square.
        let mut anchors = vec![false; count];
        for y in 0..rows.len() as i64 {
            for x in 0..24i64 {
                let deep = (-2..=2).all(|dy| (-2..=2).all(|dx| lit(&rows, x + dx, y + dy)));
                if deep {
                    anchors[piece_of[y as usize][x as usize] as usize] = true;
                }
            }
        }
        for y in 0..rows.len() as i64 {
            for x in 0..24i64 {
                if !lit(&rows, x, y) {
                    continue;
                }
                let here = piece_of[y as usize][x as usize];
                for (dx, dy) in [(1, 0), (-1, 1), (0, 1), (1, 1)] {
                    if lit(&rows, x + dx, y + dy) {
                        let there = piece_of[(y + dy) as usize][(x + dx) as usize];
                        if there != here {
                            assert!(
                                anchors[here as usize] && anchors[there as usize],
                                "seed {seed}: pieces {here} and {there} touch at ({x}, {y})"
                            );
                        }
                    }
                }
            }
        }
        for &(x, y) in &expected {
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let next = ((i64::from(x) + dx) as u32, (i64::from(y) + dy) as u32);
                    if expected.contains(&next) {
                        assert_eq!(piece_at(next), piece_at((x, y)), "seed {seed}: core split");
                    }
                    assert!(
                        core_pieces[piece_at(next) as usize],
                        "seed {seed}: {next:?} is next to a core but in a thin piece"
                    );
                }
            }
        }
        for label in 0..count as u32 {
            let pixels: Vec<(i64, i64)> = (0..rows.len())
                .flat_map(|y| (0..24).map(move |x| (x as i64, y as i64)))
                .filter(|&(x, y)| piece_of[y as usize][x as usize] == label)
                .collect();
            assert!(!pixels.is_empty(), "seed {seed}: empty piece {label}");
            let mut seen = vec![pixels[0]];
            let mut stack = vec![pixels[0]];
            while let Some((x, y)) = stack.pop() {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let next = (x + dx, y + dy);
                        if pixels.contains(&next) && !seen.contains(&next) {
                            seen.push(next);
                            stack.push(next);
                        }
                    }
                }
            }
            assert_eq!(seen.len(), pixels.len(), "seed {seed}: piece {label} split");
        }
    }
}

#[test]
fn cutting_region_by_region_gives_the_same_pieces() {
    // Regions with one core are left whole without the full cut; the pieces
    // must be the same as cutting everything, up to their numbering.
    for seed in 0..80 {
        let rows = random_rows(seed + 1000, 30, 20, 30 + seed % 60);
        let strs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let solid = runs(&strs);
        let (fast_runs, fast_labels, fast_count) = cut_of(&solid);
        let (full_runs, full_labels, full_count) = cut_regions(&solid);
        assert_eq!(fast_count, full_count, "seed {seed}");
        let pixels = |runs: &[Run], labels: &[u32]| {
            let mut out: Vec<(u32, u32, u32)> = runs
                .iter()
                .zip(labels)
                .flat_map(|(run, &label)| (run.x0..run.x1).map(move |x| (run.row, x, label)))
                .collect();
            out.sort_unstable();
            out
        };
        let fast = pixels(&fast_runs, &fast_labels);
        let full = pixels(&full_runs, &full_labels);
        assert_eq!(fast.len(), full.len(), "seed {seed}");
        let mut map = vec![u32::MAX; fast_count];
        for (a, b) in fast.iter().zip(&full) {
            assert_eq!((a.0, a.1), (b.0, b.1), "seed {seed}");
            let slot = &mut map[a.2 as usize];
            assert!(
                *slot == u32::MAX || *slot == b.2,
                "seed {seed}: piece {} split",
                a.2
            );
            *slot = b.2;
        }
        let mut seen = map.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), fast_count, "seed {seed}: pieces merged");
    }
}

/// A layer of the given model and support pixels.
fn layer(model: &[&str], support: &[&str]) -> LayerPieces {
    let model = runs(model);
    let support = runs(support);
    let solid = crate::runs::union(&model, &support);
    let (solid_labels, solid_count) = label_runs(&solid, Connectivity::Eight);
    LayerPieces::cut(LayerPixels {
        model,
        support,
        solid,
        solid_labels,
        solid_count,
        ..LayerPixels::default()
    })
}

#[test]
fn contacts_split_model_and_support_below() {
    // Below: a model block with a support stem beside it. Above: one block
    // over both.
    let lower = layer(
        &["####....", "####....", "####....", "####...."],
        &["......##", "......##"],
    );
    let upper = layer(&["########", "########", "########", "########"], &[]);
    assert_eq!(lower.pieces.len(), 2);
    let found = contacts(&lower, &upper);
    assert_eq!(
        found,
        vec![
            Contact {
                lower: 0,
                upper: 0,
                model_px: 16,
                support_px: 0,
            },
            Contact {
                lower: 1,
                upper: 0,
                model_px: 0,
                support_px: 4,
            },
        ]
    );
    assert!(touches(&lower, &upper, &found).is_empty());
}

#[test]
fn a_piece_resting_on_nothing_lists_what_it_touches() {
    let lower = layer(&["###.....", "###.....", "###....."], &[]);
    let upper = layer(
        &["........", "........", "........", "...###..", "...###.."],
        &[],
    );
    let found = contacts(&lower, &upper);
    assert!(found.is_empty());
    assert_eq!(touches(&lower, &upper, &found), vec![(0, 0)]);
    let apart = layer(
        &["........", "........", "........", "....###.", "....###."],
        &[],
    );
    assert!(touches(&lower, &apart, &[]).is_empty());
}
