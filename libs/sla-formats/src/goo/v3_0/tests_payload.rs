/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_sl1cer::raster::rle::RleRun;

use super::payload::encode_payload;
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

#[test]
fn a_payload_leaves_what_the_buffer_already_held() {
    let mut out = vec![0xDE, 0xAD];
    encode_payload(&[run(0xFF, 5)], &mut out);

    assert_eq!(&out[..2], [0xDE, 0xAD]);
    assert_eq!(out[2], MAGIC);
    assert_eq!(out[out.len() - 1], checksum_of(&out[3..out.len() - 1]));
}
