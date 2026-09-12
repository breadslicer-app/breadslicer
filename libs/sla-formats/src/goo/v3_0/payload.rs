/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! One layer image, framed as the file stores it.

use sc_sl1cer::raster::rle::RleRun;

use super::chunk::RunEncoder;

/// Byte that opens a layer image.
const PAYLOAD_MAGIC: u8 = 0x55;

/// Reorder `runs` into `out` so the raster's last row comes first.
///
/// The raster tier numbers rows from the plate's minimum Y, and this
/// container stores them from the maximum Y down, as it stores its
/// previews. `runs` must cover exactly `width` pixels on every row of
/// the frame; a shorter tail is dropped.
///
/// Runs crossing a row break are split at it and equal neighbours are
/// merged on the way out, so a frame of one value stays one run.
pub fn reverse_rows(runs: &[RleRun], width: u32, out: &mut Vec<RleRun>) {
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

/// Append `runs` to `out` as one layer image: the magic byte, the run
/// chunks, then a checksum byte covering everything after the magic.
pub fn encode_payload(runs: &[RleRun], out: &mut Vec<u8>) {
    out.push(PAYLOAD_MAGIC);
    let body = out.len();
    let mut encoder = RunEncoder::default();
    for run in runs {
        encoder.push_run(run.value, run.length, out);
    }
    let sum = out[body..]
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
    out.push(!sum);
}
