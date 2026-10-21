/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use proptest::prelude::*;
use sc_sl1cer::raster::rle::RleRun;

use super::rle7::{MAX_RUN_LENGTH, encode_layer};

/// Encode a single run into a fresh layer image.
fn encoded(value: u8, length: u32) -> Vec<u8> {
    let mut out = Vec::new();
    encode_layer(&[RleRun { value, length }], &mut out);
    out
}

/// Decode a layer image back into the 7-bit values and lengths it
/// carries, one entry per record.
fn decode(bytes: &[u8]) -> Vec<(u8, u32)> {
    let mut runs = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let value = bytes[at] & 0x7F;
        let repeated = bytes[at] & 0x80 != 0;
        at += 1;
        if !repeated {
            runs.push((value, 1));
            continue;
        }
        let lead = bytes[at];
        let (extra, mut length) = match lead {
            0x00..=0x7F => (0, u32::from(lead)),
            0x80..=0xBF => (1, u32::from(lead & 0x3F)),
            0xC0..=0xDF => (2, u32::from(lead & 0x1F)),
            _ => (3, u32::from(lead & 0x0F)),
        };
        at += 1;
        for _ in 0..extra {
            length = (length << 8) | u32::from(bytes[at]);
            at += 1;
        }
        runs.push((value, length));
    }
    runs
}

#[test]
fn a_single_pixel_run_is_one_byte_holding_its_value() {
    assert_eq!(encoded(0xFF, 1), vec![0x7F]);
    assert_eq!(encoded(0x00, 1), vec![0x00]);
}

#[test]
fn a_value_is_taken_to_the_panel_s_7_bit_range() {
    assert_eq!(encoded(0x00, 1), vec![0x00]);
    assert_eq!(encoded(0x01, 1), vec![0x00]);
    assert_eq!(encoded(0x80, 1), vec![0x40]);
    assert_eq!(encoded(0xFF, 1), vec![0x7F]);
}

#[test]
fn a_repeated_run_flags_its_value_and_follows_with_the_count() {
    assert_eq!(encoded(0xFF, 2), vec![0xFF, 0x02]);
    assert_eq!(encoded(0x00, 2), vec![0x80, 0x02]);
}

#[test]
fn a_count_of_7_bits_takes_1_byte() {
    assert_eq!(encoded(0x00, 0x7F), vec![0x80, 0x7F]);
}

#[test]
fn a_count_of_14_bits_takes_2() {
    assert_eq!(encoded(0x00, 0x80), vec![0x80, 0x80, 0x80]);
    assert_eq!(encoded(0x00, 0x3FFF), vec![0x80, 0xBF, 0xFF]);
}

#[test]
fn a_count_of_21_bits_takes_3() {
    assert_eq!(encoded(0x00, 0x4000), vec![0x80, 0xC0, 0x40, 0x00]);
    assert_eq!(encoded(0x00, 0x1F_FFFF), vec![0x80, 0xDF, 0xFF, 0xFF]);
}

#[test]
fn a_count_of_28_bits_takes_4() {
    assert_eq!(encoded(0x00, 0x20_0000), vec![0x80, 0xE0, 0x20, 0x00, 0x00]);
    assert_eq!(
        encoded(0x00, MAX_RUN_LENGTH),
        vec![0x80, 0xEF, 0xFF, 0xFF, 0xFF],
    );
}

#[test]
fn a_run_longer_than_one_record_splits() {
    assert_eq!(
        decode(&encoded(0xFF, MAX_RUN_LENGTH + 1)),
        vec![(0x7F, MAX_RUN_LENGTH), (0x7F, 1)],
    );
}

#[test]
fn the_records_follow_the_order_the_runs_arrive_in() {
    let mut out = Vec::new();
    encode_layer(
        &[
            RleRun {
                value: 0x00,
                length: 3,
            },
            RleRun {
                value: 0xFF,
                length: 1,
            },
            RleRun {
                value: 0x40,
                length: 300,
            },
        ],
        &mut out,
    );

    assert_eq!(decode(&out), vec![(0x00, 3), (0x7F, 1), (0x20, 300)]);
}

#[test]
fn an_empty_layer_encodes_to_nothing() {
    let mut out = Vec::new();
    encode_layer(&[], &mut out);
    assert!(out.is_empty(), "bytes: {out:?}");
}

proptest! {
    #[test]
    fn runs_within_one_record_round_trip(
        runs in prop::collection::vec((0_u8..=255, 1_u32..=MAX_RUN_LENGTH), 0..64)
    ) {
        let runs: Vec<RleRun> = runs
            .into_iter()
            .map(|(value, length)| RleRun { value, length })
            .collect();
        let mut bytes = Vec::new();
        encode_layer(&runs, &mut bytes);

        let expected: Vec<(u8, u32)> = runs
            .iter()
            .map(|run| (run.value >> 1, run.length))
            .collect();
        prop_assert_eq!(decode(&bytes), expected);
    }
}
