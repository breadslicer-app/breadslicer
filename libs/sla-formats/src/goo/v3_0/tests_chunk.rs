/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use proptest::prelude::*;

use super::chunk::{MAX_RUN_LENGTH, RunEncoder};
use super::test_decode::decode_chunks as decode;

/// Encode a single run into a fresh image's chunk stream.
fn encoded(value: u8, length: u32) -> Vec<u8> {
    let mut out = Vec::new();
    RunEncoder::default().push_run(value, length, &mut out);
    out
}

/// Encode `value` over `length` pixels when 1 pixel of `previous`
/// came before it in the same image, returning only the bytes of the
/// second run.
fn encoded_after(previous: u8, value: u8, length: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = RunEncoder::default();
    encoder.push_run(previous, 1, &mut out);
    let lead = out.len();
    encoder.push_run(value, length, &mut out);
    out.split_off(lead)
}

/// Decode `bytes` as if 1 pixel of `previous` preceded them,
/// returning only the runs that `bytes` itself carries.
fn decode_after(previous: u8, bytes: &[u8]) -> Vec<(u8, u32)> {
    let mut stream = Vec::new();
    RunEncoder::default().push_run(previous, 1, &mut stream);
    stream.extend_from_slice(bytes);
    decode(&stream).split_off(1)
}

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

#[test]
fn the_specified_delta_examples_encode_byte_for_byte() {
    // Each row is a worked example from the specification: the step
    // to reach `value` from `previous`, over `length` pixels.
    for (previous, value, length, expected) in [
        (0x40, 0x41, 1, vec![0x81]),
        (0x40, 0x42, 255, vec![0x92, 0xFF]),
        (0x40, 0x3F, 1, vec![0xA1]),
        (0x40, 0x3E, 238, vec![0xB2, 0xEE]),
    ] {
        assert_eq!(
            encoded_after(previous, value, length),
            expected,
            "{previous:#04X} to {value:#04X} over {length}",
        );
    }
}

#[test]
fn a_delta_chunk_round_trips_to_its_run() {
    for (previous, value, length) in [(0x40, 0x41, 1), (0x40, 0x3E, 238)] {
        let bytes = encoded_after(previous, value, length);
        assert_eq!(
            decode_after(previous, &bytes),
            vec![(value, length)],
            "{previous:#04X} to {value:#04X} over {length}",
        );
    }
}

#[test]
fn the_first_run_of_an_image_has_no_previous_pixel_to_step_from() {
    assert_eq!(encoded(0x41, 1), vec![0x41, 0x41]);
}

#[test]
fn a_step_of_16_is_too_wide_for_a_delta() {
    assert_eq!(encoded_after(0x40, 0x50, 1), vec![0x41, 0x50]);
    assert_eq!(encoded_after(0x40, 0x4F, 1), vec![0x8F]);
}

#[test]
fn a_run_past_255_is_too_long_for_a_delta() {
    assert_eq!(encoded_after(0x40, 0x41, 256), vec![0x50, 0x41, 0x10]);
    assert_eq!(encoded_after(0x40, 0x41, 255), vec![0x91, 0xFF]);
}

#[test]
fn a_delta_is_taken_only_where_it_is_shorter_than_the_grey_chunk() {
    // Both forms cost 2 bytes over 2 to 15 pixels, so the grey chunk
    // keeps those; the delta wins at 1 pixel and again from 16 up.
    for (length, expected) in [
        (1, vec![0x81]),
        (2, vec![0x42, 0x41]),
        (15, vec![0x4F, 0x41]),
        (16, vec![0x91, 0x10]),
    ] {
        assert_eq!(
            encoded_after(0x40, 0x41, length),
            expected,
            "run of {length}",
        );
    }
}

#[test]
fn black_and_white_runs_keep_their_own_chunk_types() {
    assert_eq!(encoded_after(0x01, 0x00, 1), vec![0x01]);
    assert_eq!(encoded_after(0xFE, 0xFF, 1), vec![0xC1]);
}

proptest! {
    #[test]
    fn runs_within_one_chunk_round_trip(
        runs in prop::collection::vec((0_u8..=255, 1_u32..=MAX_RUN_LENGTH), 0..64)
    ) {
        let mut bytes = Vec::new();
        let mut encoder = RunEncoder::default();
        for (value, length) in &runs {
            encoder.push_run(*value, *length, &mut bytes);
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
