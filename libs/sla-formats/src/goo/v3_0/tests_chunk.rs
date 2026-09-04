/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use proptest::prelude::*;

use super::chunk::{MAX_RUN_LENGTH, encode_run};

/// Decode a chunk stream back to `(value, length)` runs, one entry
/// per chunk. Panics on a delta chunk, which the encoder never emits.
fn decode(bytes: &[u8]) -> Vec<(u8, u32)> {
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

fn encoded(value: u8, length: u32) -> Vec<u8> {
    let mut out = Vec::new();
    encode_run(value, length, &mut out);
    out
}

// The published specification lists a fourth worked example,
// `F1 CC BB AA FF`, whose run length is transcribed with 9 hex digits
// and cannot fit the 28-bit field. The bit rules are what this
// encoder follows, so that row is left out below.

#[test]
fn the_specified_black_run_example_encodes_byte_for_byte() {
    assert_eq!(encoded(0x00, 0x555_657F), vec![0x3F, 0x55, 0x56, 0x57]);
}

#[test]
fn the_specified_grey_run_example_encodes_byte_for_byte() {
    assert_eq!(
        encoded(0xAA, 0xBBC_C155),
        vec![0x75, 0xAA, 0xBB, 0xCC, 0x15],
    );
}

#[test]
fn the_specified_short_black_run_example_encodes_byte_for_byte() {
    assert_eq!(encoded(0x00, 5), vec![0x05]);
}

#[test]
fn a_white_run_carries_the_white_chunk_type() {
    assert_eq!(encoded(0xFF, 5), vec![0xC5]);
}

#[test]
fn a_grey_run_puts_its_value_before_its_length_bytes() {
    let bytes = encoded(0x80, 256);
    assert_eq!(bytes, vec![0x50, 0x80, 0x10]);
}

#[test]
fn a_run_of_15_pixels_needs_no_length_byte() {
    let bytes = encoded(0x00, 15);
    assert_eq!(bytes, vec![0x0F]);
    assert_eq!(decode(&bytes), vec![(0x00, 15)]);
}

#[test]
fn a_run_of_16_pixels_takes_the_first_length_byte() {
    let bytes = encoded(0x00, 16);
    assert_eq!(bytes, vec![0x10, 0x01]);
    assert_eq!(decode(&bytes), vec![(0x00, 16)]);
}

#[test]
fn the_length_byte_count_grows_at_each_width_boundary() {
    for (length, expected_bytes) in [
        (0xF, 1),
        (0x10, 2),
        (0xFFF, 2),
        (0x1000, 3),
        (0xF_FFFF, 3),
        (0x10_0000, 4),
        (MAX_RUN_LENGTH, 4),
    ] {
        let bytes = encoded(0x00, length);
        assert_eq!(bytes.len(), expected_bytes, "run of {length}");
        assert_eq!(decode(&bytes), vec![(0x00, length)], "run of {length}");
    }
}

#[test]
fn a_run_past_the_widest_chunk_is_split() {
    let bytes = encoded(0xFF, MAX_RUN_LENGTH + 1);
    assert_eq!(decode(&bytes), vec![(0xFF, MAX_RUN_LENGTH), (0xFF, 1)]);
}

#[test]
fn the_longest_run_a_u32_holds_splits_into_whole_chunks() {
    let decoded = decode(&encoded(0x00, u32::MAX));
    let total: u64 = decoded.iter().map(|(_, length)| u64::from(*length)).sum();
    assert_eq!(total, u64::from(u32::MAX));
    assert!(
        decoded.iter().all(|(value, _)| *value == 0x00),
        "a split run keeps its value",
    );
}

#[test]
fn a_zero_length_run_appends_nothing() {
    assert!(encoded(0x00, 0).is_empty());
}

#[test]
fn an_empty_frame_is_one_chunk_of_4_bytes() {
    let pixels = 4098 * 2560;
    let bytes = encoded(0x00, pixels);
    assert_eq!(bytes.len(), 4);
    assert_eq!(decode(&bytes), vec![(0x00, pixels)]);
}

proptest! {
    #[test]
    fn runs_within_one_chunk_round_trip(
        runs in prop::collection::vec((0_u8..=255, 1_u32..=MAX_RUN_LENGTH), 0..64)
    ) {
        let mut bytes = Vec::new();
        for (value, length) in &runs {
            encode_run(*value, *length, &mut bytes);
        }
        prop_assert_eq!(decode(&bytes), runs);
    }

    #[test]
    fn a_split_run_preserves_its_pixel_count(length in 1_u32..=u32::MAX) {
        let decoded = decode(&encoded(0x7F, length));
        let total: u64 = decoded.iter().map(|(_, run)| u64::from(*run)).sum();
        prop_assert_eq!(total, u64::from(length));
    }
}
