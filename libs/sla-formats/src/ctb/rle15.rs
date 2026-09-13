/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The run-length grammar of a CTB preview image.
//!
//! Pixels are RGB565 words in raster order, little-endian like the
//! rest of the container, with the green channel's low bit taken for a
//! run flag: red in bits 15:11, green in 10:6, the flag in bit 5, blue
//! in 4:0. A flagged word is followed by a count word `0x3nnn`, whose
//! 12 bits hold the copies that come *after* the first pixel, so a
//! count of 0 is one pixel written the long way.
//!
//! Runs cross rows freely, which is why a frame of one colour is a
//! single record rather than one per row.

use crate::preview::PreviewImage;

/// Channels per pixel of a [`PreviewImage`].
const CHANNELS: usize = 3;

/// Set on a pixel word when a count word follows it.
const RUN_FLAG: u16 = 0x0020;

/// Top nibble every count word carries.
const COUNT_TAG: u16 = 0x3000;

/// Copies one record expresses beyond its first pixel. The 12-bit
/// field would hold 1 more, and the slicer that authored the format
/// stops here.
pub const MAX_COPIES: u32 = 0x0FFE;

/// Append `image` to `out` as one preview blob, rows in the order the
/// image carries them.
pub fn encode_preview(image: &PreviewImage, out: &mut Vec<u8>) {
    let mut pixels = image.rgb().as_chunks::<CHANNELS>().0.iter().map(pack);
    let Some(mut current) = pixels.next() else {
        return;
    };
    let mut run = 1_u32;
    for pixel in pixels {
        if pixel == current {
            run += 1;
            continue;
        }
        push_run(current, run, out);
        current = pixel;
        run = 1;
    }
    push_run(current, run, out);
}

/// Pack one pixel into its word, the run flag left clear.
fn pack(rgb: &[u8; CHANNELS]) -> u16 {
    (u16::from(rgb[0] >> 3) << 11) | (u16::from(rgb[1] >> 3) << 6) | u16::from(rgb[2] >> 3)
}

/// Append the records covering `length` copies of `pixel`, splitting
/// what one count word cannot hold.
fn push_run(pixel: u16, length: u32, out: &mut Vec<u8>) {
    let mut remaining = length;
    while remaining > 1 {
        let copies = (remaining - 1).min(MAX_COPIES);
        push_word(pixel | RUN_FLAG, out);
        push_word(COUNT_TAG | copies as u16, out);
        remaining -= copies + 1;
    }
    if remaining == 1 {
        push_word(pixel, out);
    }
}

fn push_word(word: u16, out: &mut Vec<u8>) {
    out.extend_from_slice(&word.to_le_bytes());
}
