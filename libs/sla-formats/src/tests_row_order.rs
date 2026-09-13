/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_sl1cer::raster::rle::RleRun;

use crate::row_order::reverse_rows;

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
