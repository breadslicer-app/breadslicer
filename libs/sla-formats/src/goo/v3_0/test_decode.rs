/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! An independent decoder for the tests, so no encoder is checked
//! against itself.

/// Decode a chunk stream to `(value, length)` runs, one entry per
/// chunk. Panics on a delta chunk, which the encoder never emits.
pub(super) fn decode_chunks(bytes: &[u8]) -> Vec<(u8, u32)> {
    let mut runs = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let tag = bytes[cursor];
        cursor += 1;
        let value = match tag >> 6 {
            0b00 => 0x00,
            0b11 => 0xFF,
            0b01 => {
                let grey = bytes[cursor];
                cursor += 1;
                grey
            }
            _ => panic!("delta chunk at byte {cursor}"),
        };
        let mut high = 0_u32;
        for _ in 0..((tag >> 4) & 0b11) {
            high = (high << 8) | u32::from(bytes[cursor]);
            cursor += 1;
        }
        runs.push((value, (high << 4) | u32::from(tag & 0x0F)));
    }
    runs
}
