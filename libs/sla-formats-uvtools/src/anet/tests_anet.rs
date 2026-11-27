/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use super::*;

/// Reads a layer's bit stream back as the machine does: fields most
/// significant bit first, bytes filled from their least significant bit.
fn decode(stream: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut bit = 0_usize;
    let mut take = |count: u32| -> u32 {
        let mut value = 0_u32;
        for _ in 0..count {
            let byte = stream[bit / 8];
            value = (value << 1) | u32::from((byte >> (bit % 8)) & 1);
            bit += 1;
        }
        value
    };
    let (width, height) = (take(16), take(16));
    let mut white = take(1) == 1;
    let mut pixels = Vec::new();
    while pixels.len() < (width * height) as usize {
        let size = take(5);
        let length = take(size + 1);
        pixels.extend(std::iter::repeat_n(
            if white { 255 } else { 0 },
            length as usize,
        ));
        white = !white;
    }
    (width, height, pixels)
}

#[test]
fn a_layer_reads_back_black_and_white_with_its_bounds_and_count() {
    let width = 10;
    let runs = [
        RleRun {
            value: 0,
            length: 12,
        },
        RleRun {
            value: 255,
            length: 3,
        },
        RleRun {
            value: 200,
            length: 2,
        },
        RleRun {
            value: 60,
            length: 3,
        },
        RleRun {
            value: 0,
            length: 20,
        },
    ];
    let bytes = encode_layer(&runs, width, 4);
    let be = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!(be(0), 5, "the lit pixels");
    assert_eq!((be(4), be(8), be(12), be(16)), (2, 1, 6, 1), "the bounds");
    let bits = be(20);
    let stream = &bytes[24..];
    assert_eq!(stream.len() as u32, bits.div_ceil(8));
    let (w, h, pixels) = decode(stream);
    assert_eq!((w, h), (10, 4));
    let mut expected = vec![0_u8; 40];
    expected[12..17].fill(255);
    assert_eq!(pixels, expected);
}
