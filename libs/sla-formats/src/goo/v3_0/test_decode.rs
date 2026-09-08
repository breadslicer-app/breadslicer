/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! An independent decoder for the tests, so no encoder is checked
//! against itself.
//!
//! Written from the format's own bit rules rather than shared with
//! the encoder, so its masks are spelled out here on purpose.

/// Decode a chunk stream to `(value, length)` runs, one entry per
/// chunk.
///
/// A delta chunk's value is read against the run before it, so the
/// slice must start at a layer image's first chunk.
pub(super) fn decode_chunks(bytes: &[u8]) -> Vec<(u8, u32)> {
    let mut runs = Vec::new();
    let mut cursor = 0;
    let mut previous = 0_u8;
    while cursor < bytes.len() {
        let tag = bytes[cursor];
        cursor += 1;
        let (value, length) = if tag >> 6 == 0b10 {
            let magnitude = tag & 0x0F;
            // Overflowing here means the encoder emitted a step off
            // the end of the range, which is the bug worth catching.
            let value = if tag & 0b0010_0000 == 0 {
                previous + magnitude
            } else {
                previous - magnitude
            };
            let length = if tag & 0b0001_0000 == 0 {
                1
            } else {
                let run = bytes[cursor];
                cursor += 1;
                u32::from(run)
            };
            (value, length)
        } else {
            let value = match tag >> 6 {
                0b00 => 0x00,
                0b11 => 0xFF,
                _ => {
                    let grey = bytes[cursor];
                    cursor += 1;
                    grey
                }
            };
            let mut high = 0_u32;
            for _ in 0..((tag >> 4) & 0b11) {
                high = (high << 8) | u32::from(bytes[cursor]);
                cursor += 1;
            }
            (value, (high << 4) | u32::from(tag & 0x0F))
        };
        runs.push((value, length));
        previous = value;
    }
    runs
}
