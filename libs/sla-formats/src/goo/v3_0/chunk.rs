/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The run-length chunk grammar of a GOO layer image.
//!
//! A chunk's tag byte is `TT SS CCCC`: the pair `TT` names the run's
//! colour, `SS` counts the length bytes that follow, and `CCCC` holds
//! the run length's low nibble. The length bytes carry the rest of it,
//! most significant first.

/// Chunk type bits, occupying the tag byte's top 2 bits. The fourth
/// type, a delta from the previous pixel, is never emitted.
const TYPE_BLACK_RUN: u8 = 0b0000_0000;
const TYPE_GREY_RUN: u8 = 0b0100_0000;
const TYPE_WHITE_RUN: u8 = 0b1100_0000;

const BLACK: u8 = 0x00;
const WHITE: u8 = 0xFF;

/// Longest pixel run a single chunk expresses.
pub const MAX_RUN_LENGTH: u32 = 0x0FFF_FFFF;

/// Append the chunks covering `length` pixels of `value` to `out`,
/// splitting a run longer than [`MAX_RUN_LENGTH`].
pub fn encode_run(value: u8, length: u32, out: &mut Vec<u8>) {
    let mut remaining = length;
    while remaining > 0 {
        let chunk_length = remaining.min(MAX_RUN_LENGTH);
        encode_chunk(value, chunk_length, out);
        remaining -= chunk_length;
    }
}

fn encode_chunk(value: u8, length: u32, out: &mut Vec<u8>) {
    let length_bytes = length_byte_count(length);
    let tag_tail = (length_bytes << 4) | ((length & 0x0F) as u8);

    match value {
        BLACK => out.push(TYPE_BLACK_RUN | tag_tail),
        WHITE => out.push(TYPE_WHITE_RUN | tag_tail),
        grey => {
            out.push(TYPE_GREY_RUN | tag_tail);
            out.push(grey);
        }
    }

    for position in (0..length_bytes).rev() {
        let shift = 4 + 8 * u32::from(position);
        out.push(((length >> shift) & 0xFF) as u8);
    }
}

/// Count the length bytes a run needs beyond the tag byte's nibble.
fn length_byte_count(length: u32) -> u8 {
    match length {
        0..=0xF => 0,
        0x10..=0xFFF => 1,
        0x1000..=0xF_FFFF => 2,
        _ => 3,
    }
}
