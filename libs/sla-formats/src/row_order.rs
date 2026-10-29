/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Which edge of the plate a stored layer image starts at.
//!
//! The raster tier numbers rows from the plate's minimum Y. GOO and
//! CTB both store them from the maximum Y down, the order they store
//! their previews in, so both turn a rasterised layer around before
//! encoding it. A panel whose rows run along the plate's depth stores
//! that image turned a quarter turn as well.

use sc_sl1cer::raster::rle::RleRun;

/// Reorder `runs` into `out` so the raster's last row comes first.
///
/// `runs` must cover exactly `width` pixels on every row of the
/// frame; a shorter tail is dropped.
///
/// Runs crossing a row break are split at it and equal neighbours are
/// merged on the way out, so a frame of one value stays one run.
pub(crate) fn reverse_rows(runs: &[RleRun], width: u32, out: &mut Vec<RleRun>) {
    out.clear();
    let width = u64::from(width);
    let mut split: Vec<RleRun> = Vec::new();
    let mut row_starts: Vec<usize> = vec![0];
    let mut filled = 0_u64;

    for run in runs {
        let mut left = u64::from(run.length);
        while left > 0 {
            let take = left.min(width - filled);
            split.push(RleRun {
                value: run.value,
                length: take as u32,
            });
            filled += take;
            left -= take;
            if filled == width {
                filled = 0;
                row_starts.push(split.len());
            }
        }
    }

    for row in row_starts.windows(2).rev() {
        for run in &split[row[0]..row[1]] {
            match out.last_mut() {
                Some(last) if last.value == run.value => last.length += run.length,
                _ => out.push(*run),
            }
        }
    }
}

/// Turn a frame of `runs` (`width` pixels per row, `height` rows) a
/// quarter turn counter-clockwise into `out`: `height` pixels per row,
/// `width` rows, the frame's last column becoming the first row and its
/// first row the first column.
///
/// `runs` must cover the frame as [`reverse_rows`] expects; pixels past
/// its last row are dropped. Only the rectangle holding the lit pixels
/// is unpacked, into `lit`, so a mostly dark frame stays cheap; equal
/// neighbours are merged on the way out, as in [`reverse_rows`].
pub(crate) fn turn_ccw(
    runs: &[RleRun],
    width: u32,
    height: u32,
    lit: &mut Vec<u8>,
    out: &mut Vec<RleRun>,
) {
    out.clear();
    let (width, height) = (width as usize, height as usize);
    let frame = width * height;

    // The lit rectangle, `x1` and `y1` exclusive; empty for a dark frame.
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    let mut position = 0_usize;
    for run in runs {
        let end = (position + run.length as usize).min(frame);
        if run.value != 0 && end > position {
            let (first_row, first_col) = (position / width, position % width);
            let (last_row, last_col) = ((end - 1) / width, (end - 1) % width);
            // A run that wraps a row covers every column between.
            let (cols_start, cols_end) = if first_row == last_row {
                (first_col, last_col + 1)
            } else {
                (0, width)
            };
            x0 = x0.min(cols_start);
            x1 = x1.max(cols_end);
            y0 = y0.min(first_row);
            y1 = y1.max(last_row + 1);
        }
        position += run.length as usize;
    }
    let (lit_width, lit_height) = (x1.saturating_sub(x0), y1.saturating_sub(y0));

    lit.clear();
    lit.resize(lit_width * lit_height, 0);
    let mut position = 0_usize;
    for run in runs {
        let end = (position + run.length as usize).min(frame);
        if run.value != 0 {
            let mut at = position;
            while at < end {
                let (row, col) = (at / width, at % width);
                let take = (end - at).min(width - col);
                let start = (row - y0) * lit_width + (col - x0);
                lit[start..start + take].fill(run.value);
                at += take;
            }
        }
        position += run.length as usize;
    }

    for col in (0..width).rev() {
        if !(x0..x1).contains(&col) {
            push_run(out, 0, height);
            continue;
        }
        push_run(out, 0, y0);
        let slot = col - x0;
        let mut row = 0;
        while row < lit_height {
            let value = lit[row * lit_width + slot];
            let mut length = 1;
            while row + length < lit_height && lit[(row + length) * lit_width + slot] == value {
                length += 1;
            }
            push_run(out, value, length);
            row += length;
        }
        push_run(out, 0, height - y1);
    }
}

/// Append `length` pixels of `value`, merged into the last run when it
/// has the same value.
fn push_run(out: &mut Vec<RleRun>, value: u8, length: usize) {
    if length == 0 {
        return;
    }
    match out.last_mut() {
        Some(last) if last.value == value => last.length += length as u32,
        _ => out.push(RleRun {
            value,
            length: length as u32,
        }),
    }
}
