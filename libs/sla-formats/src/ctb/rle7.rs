/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The run-length grammar of a CTB layer image.
//!
//! A record opens with the pixel value in its low 7 bits: the panel
//! reads 0 to 127, not a whole byte. A single pixel is that byte
//! alone; longer runs set the top bit and follow with the count, whose
//! leading bits say how many bytes it occupies and whose bytes run
//! most significant first. That count is the only big-endian quantity
//! in the family.

use sc_sl1cer::raster::rle::RleRun;

/// Set on the value byte when a count follows it.
const REPEATED: u8 = 0b1000_0000;

/// Leading bits of a count, by the number of bytes it occupies.
const COUNT_2_BYTES: u8 = 0b1000_0000;
const COUNT_3_BYTES: u8 = 0b1100_0000;
const COUNT_4_BYTES: u8 = 0b1110_0000;

/// Widest count each form expresses, the leading bits taken out.
const MAX_COUNT_1_BYTE: u32 = 0x7F;
const MAX_COUNT_2_BYTES: u32 = 0x3FFF;
const MAX_COUNT_3_BYTES: u32 = 0x1F_FFFF;

/// Longest pixel run a single record expresses.
pub const MAX_RUN_LENGTH: u32 = 0x0FFF_FFFF;

/// Append `runs` to `out` as one layer image, in the order they
/// arrive.
///
/// The runs are the layer's pixels in the order the container stores
/// them, so a caller that holds them in another row order reorders
/// before encoding.
pub fn encode_layer(runs: &[RleRun], out: &mut Vec<u8>) {
    for run in runs {
        push_run(run.value, run.length, out);
    }
}

/// Append the records covering `length` pixels of `value`, splitting a
/// run longer than [`MAX_RUN_LENGTH`].
fn push_run(value: u8, length: u32, out: &mut Vec<u8>) {
    let mut remaining = length;
    while remaining > 0 {
        let record = remaining.min(MAX_RUN_LENGTH);
        push_record(value, record, out);
        remaining -= record;
    }
}

fn push_record(value: u8, length: u32, out: &mut Vec<u8>) {
    let pixel = value >> 1;
    if length == 1 {
        out.push(pixel);
        return;
    }
    out.push(pixel | REPEATED);
    push_count(length, out);
}

fn push_count(length: u32, out: &mut Vec<u8>) {
    match length {
        0..=MAX_COUNT_1_BYTE => out.push(length as u8),
        0x80..=MAX_COUNT_2_BYTES => {
            out.push(COUNT_2_BYTES | (length >> 8) as u8);
            out.push(length as u8);
        }
        0x4000..=MAX_COUNT_3_BYTES => {
            out.push(COUNT_3_BYTES | (length >> 16) as u8);
            out.push((length >> 8) as u8);
            out.push(length as u8);
        }
        _ => {
            out.push(COUNT_4_BYTES | (length >> 24) as u8);
            out.push((length >> 16) as u8);
            out.push((length >> 8) as u8);
            out.push(length as u8);
        }
    }
}
