/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_sl1cer::raster::rle::RleRun;

use crate::row_order::{reverse_rows, turn_ccw};

fn run(value: u8, length: u32) -> RleRun {
    RleRun { value, length }
}

fn reversed(runs: &[RleRun], width: u32) -> Vec<RleRun> {
    let mut out = Vec::new();
    reverse_rows(runs, width, &mut out);
    out
}

/// Expand runs into pixels, then read them back as rows.
fn rows_of(runs: &[RleRun], width: usize) -> Vec<Vec<u8>> {
    let mut pixels = Vec::new();
    for run in runs {
        pixels.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    pixels.chunks(width).map(<[u8]>::to_vec).collect()
}

#[test]
fn reversing_rows_puts_the_last_row_first() {
    let runs = [run(1, 3), run(2, 3), run(3, 3)];

    assert_eq!(
        rows_of(&reversed(&runs, 3), 3),
        vec![vec![3; 3], vec![2; 3], vec![1; 3]],
    );
}

#[test]
fn reversing_rows_leaves_the_order_within_a_row_alone() {
    let runs = [run(1, 1), run(2, 2), run(3, 1), run(4, 2)];

    assert_eq!(
        rows_of(&reversed(&runs, 3), 3),
        vec![vec![3, 4, 4], vec![1, 2, 2]],
    );
}

#[test]
fn a_run_crossing_a_row_break_is_split_at_it() {
    // One run of 4 over rows of 2: the halves land in opposite rows,
    // and the values around them swap.
    let runs = [run(9, 1), run(7, 4), run(8, 1)];

    assert_eq!(
        rows_of(&reversed(&runs, 2), 2),
        vec![vec![7, 8], vec![7, 7], vec![9, 7]],
    );
}

#[test]
fn a_frame_of_one_value_stays_a_single_run() {
    assert_eq!(reversed(&[run(0, 4096)], 64), vec![run(0, 4096)]);
}

#[test]
fn reversing_rows_preserves_the_pixel_count() {
    let runs = [run(0, 7), run(255, 11), run(0, 6)];

    assert_eq!(reversed(&runs, 6).iter().map(|r| r.length).sum::<u32>(), 24);
}

#[test]
fn reversing_rows_twice_is_the_original() {
    let runs = [run(0, 5), run(128, 3), run(255, 4)];
    let once = reversed(&runs, 4);

    assert_eq!(reversed(&once, 4), runs);
}

#[test]
fn reversing_rows_clears_what_the_buffer_already_held() {
    let mut out = vec![run(0xAB, 9)];
    reverse_rows(&[run(1, 2), run(2, 2)], 2, &mut out);

    assert_eq!(out, vec![run(2, 2), run(1, 2)]);
}

fn turned(runs: &[RleRun], width: u32, height: u32) -> Vec<RleRun> {
    let mut lit = Vec::new();
    let mut out = Vec::new();
    turn_ccw(runs, width, height, &mut lit, &mut out);
    out
}

/// Turn a frame of pixels a quarter turn counter-clockwise, one pixel
/// at a time.
fn turned_pixels(rows: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let width = rows[0].len();
    (0..width)
        .rev()
        .map(|col| rows.iter().map(|row| row[col]).collect())
        .collect()
}

/// Pack rows of pixels into runs, one run per pixel.
fn runs_of(rows: &[Vec<u8>]) -> Vec<RleRun> {
    rows.iter().flatten().map(|&value| run(value, 1)).collect()
}

#[test]
fn turning_puts_the_last_column_first() {
    // 3 x 2 frame:  1 2 3
    //               4 5 6
    let rows = vec![vec![1, 2, 3], vec![4, 5, 6]];
    assert_eq!(
        rows_of(&turned(&runs_of(&rows), 3, 2), 2),
        vec![vec![3, 6], vec![2, 5], vec![1, 4]],
    );
}

#[test]
fn turning_matches_a_pixel_by_pixel_turn() {
    // A lit block off-centre with a grey edge, in a 7 x 5 frame.
    let mut rows = vec![vec![0_u8; 7]; 5];
    for (r, row) in rows.iter_mut().enumerate().skip(1).take(3) {
        for (c, pixel) in row.iter_mut().enumerate().skip(4).take(2) {
            *pixel = (if c == 4 { 128 } else { 255 }) - r as u8;
        }
    }
    let out = turned(&runs_of(&rows), 7, 5);
    assert_eq!(rows_of(&out, 5), turned_pixels(&rows));
    assert_eq!(out.iter().map(|r| r.length).sum::<u32>(), 35);
}

#[test]
fn turning_a_dark_frame_keeps_it_one_run() {
    assert_eq!(turned(&[run(0, 12)], 4, 3), vec![run(0, 12)]);
}

#[test]
fn turning_merges_equal_neighbours() {
    // A full frame of one value stays one run.
    assert_eq!(turned(&[run(9, 6)], 3, 2), vec![run(9, 6)]);
}

#[test]
fn turning_handles_runs_wrapping_rows() {
    // 4 x 3 frame: one run from the end of row 0 into row 1.
    let runs = [run(0, 3), run(7, 3), run(0, 6)];
    let rows = rows_of(&runs, 4);
    assert_eq!(rows_of(&turned(&runs, 4, 3), 3), turned_pixels(&rows));
}

#[test]
fn four_turns_bring_the_frame_back() {
    let rows = vec![vec![1, 0, 0, 2], vec![0, 3, 0, 0], vec![0, 0, 0, 4]];
    let mut runs = runs_of(&rows);
    let (mut width, mut height) = (4, 3);
    for _ in 0..4 {
        runs = turned(&runs, width, height);
        (width, height) = (height, width);
    }
    assert_eq!(rows_of(&runs, 4), rows);
}
