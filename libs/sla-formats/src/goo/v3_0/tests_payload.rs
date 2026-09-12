/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_sl1cer::raster::rle::RleRun;

use super::payload::{encode_payload, reverse_rows};
use super::test_decode::decode_chunks;

const MAGIC: u8 = 0x55;

fn run(value: u8, length: u32) -> RleRun {
    RleRun { value, length }
}

fn payload(runs: &[RleRun]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_payload(runs, &mut out);
    out
}

fn checksum_of(body: &[u8]) -> u8 {
    !body.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte))
}

#[test]
fn a_payload_opens_with_the_magic_byte() {
    assert_eq!(payload(&[run(0xFF, 20)])[0], MAGIC);
}

#[test]
fn the_last_byte_is_the_ones_complement_of_the_body_sum() {
    let bytes = payload(&[run(0x00, 4096), run(0x80, 3), run(0xFF, 17)]);
    let (body, checksum) = bytes[1..].split_at(bytes.len() - 2);
    assert_eq!(checksum, [checksum_of(body)]);
}

#[test]
fn the_checksum_covers_every_body_byte() {
    let bytes = payload(&[run(0x00, 4096), run(0x80, 3), run(0xFF, 17)]);
    let body = &bytes[1..bytes.len() - 1];
    let checksum = bytes[bytes.len() - 1];

    for index in 0..body.len() {
        let mut flipped = body.to_vec();
        flipped[index] ^= 0x01;
        assert_ne!(
            checksum_of(&flipped),
            checksum,
            "a flip at body byte {index} left the checksum alone",
        );
    }
}

#[test]
fn the_magic_byte_is_outside_the_checksum() {
    let bytes = payload(&[run(0xFF, 5)]);
    let over_everything = checksum_of(&bytes[..bytes.len() - 1]);
    assert_ne!(bytes[bytes.len() - 1], over_everything);
}

#[test]
fn an_all_black_frame_is_six_bytes() {
    let bytes = payload(&[run(0x00, 4098 * 2560)]);
    assert_eq!(bytes.len(), 6);
    assert_eq!(bytes[0], MAGIC);
    assert_eq!(
        decode_chunks(&bytes[1..bytes.len() - 1]),
        vec![(0x00, 4098 * 2560)],
    );
}

#[test]
fn a_layer_with_no_runs_is_the_magic_and_a_checksum() {
    assert_eq!(payload(&[]), vec![MAGIC, 0xFF]);
}

#[test]
fn the_runs_survive_the_frame() {
    let runs = [run(0x00, 15), run(0x7F, 1_048_576), run(0xFF, 4095)];
    let bytes = payload(&runs);
    assert_eq!(
        decode_chunks(&bytes[1..bytes.len() - 1]),
        vec![(0x00, 15), (0x7F, 1_048_576), (0xFF, 4095)],
    );
}

#[test]
fn a_delta_chunk_shortens_a_run_close_to_the_one_before_it() {
    let runs = [run(0x40, 1), run(0x41, 100)];
    let bytes = payload(&runs);
    assert_eq!(
        decode_chunks(&bytes[1..bytes.len() - 1]),
        vec![(0x40, 1), (0x41, 100)],
    );
    // Magic, the leading grey chunk's 2 bytes, the delta chunk's 2,
    // then the checksum. Without the delta the second run would need
    // a 3-byte grey chunk.
    assert_eq!(bytes.len(), 6);
}

#[test]
fn no_pixel_value_carries_from_one_payload_into_the_next() {
    // A delta is defined against the pixel before it, so a payload
    // whose first run is close to the previous payload's last must
    // still open with an absolute chunk.
    let first = payload(&[run(0x40, 1)]);
    let mut both = first.clone();
    encode_payload(&[run(0x41, 1)], &mut both);

    assert_eq!(&both[..first.len()], &first[..]);
    assert_eq!(both[first.len()..], *payload(&[run(0x41, 1)]));
}

fn reversed(runs: &[RleRun], width: u32) -> Vec<RleRun> {
    let mut out = Vec::new();
    reverse_rows(runs, width, &mut out);
    out
}

/// Expand runs into pixels, then read them back as rows.
fn rows_of(runs: &[RleRun], width: usize) -> Vec<Vec<u8>> {
    let mut pixels = Vec::new();
    for run in runs {
        pixels.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    pixels.chunks(width).map(<[u8]>::to_vec).collect()
}

#[test]
fn reversing_rows_puts_the_last_row_first() {
    let runs = [run(1, 3), run(2, 3), run(3, 3)];

    assert_eq!(
        rows_of(&reversed(&runs, 3), 3),
        vec![vec![3; 3], vec![2; 3], vec![1; 3]],
    );
}

#[test]
fn reversing_rows_leaves_the_order_within_a_row_alone() {
    let runs = [run(1, 1), run(2, 2), run(3, 1), run(4, 2)];

    assert_eq!(
        rows_of(&reversed(&runs, 3), 3),
        vec![vec![3, 4, 4], vec![1, 2, 2]],
    );
}

#[test]
fn a_run_crossing_a_row_break_is_split_at_it() {
    // One run of 4 over rows of 2: the halves land in opposite rows,
    // and the values around them swap.
    let runs = [run(9, 1), run(7, 4), run(8, 1)];

    assert_eq!(
        rows_of(&reversed(&runs, 2), 2),
        vec![vec![7, 8], vec![7, 7], vec![9, 7]],
    );
}

#[test]
fn a_frame_of_one_value_stays_a_single_run() {
    assert_eq!(reversed(&[run(0, 4096)], 64), vec![run(0, 4096)]);
}

#[test]
fn reversing_rows_preserves_the_pixel_count() {
    let runs = [run(0, 7), run(255, 11), run(0, 6)];

    assert_eq!(reversed(&runs, 6).iter().map(|r| r.length).sum::<u32>(), 24,);
}

#[test]
fn reversing_rows_twice_is_the_original() {
    let runs = [run(0, 5), run(128, 3), run(255, 4)];
    let once = reversed(&runs, 4);

    assert_eq!(reversed(&once, 4), runs);
}

#[test]
fn reversing_rows_clears_what_the_buffer_already_held() {
    let mut out = vec![run(0xAB, 9)];
    reverse_rows(&[run(1, 2), run(2, 2)], 2, &mut out);

    assert_eq!(out, vec![run(2, 2), run(1, 2)]);
}

#[test]
fn a_payload_leaves_what_the_buffer_already_held() {
    let mut out = vec![0xDE, 0xAD];
    encode_payload(&[run(0xFF, 5)], &mut out);

    assert_eq!(&out[..2], [0xDE, 0xAD]);
    assert_eq!(out[2], MAGIC);
    assert_eq!(out[out.len() - 1], checksum_of(&out[3..out.len() - 1]));
}
