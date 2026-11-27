/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::test_fixture::{PIXELS_X, PIXELS_Y, expected_images, program, raster, stack};

/// Decodes one layer's runs as the machine does.
fn decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let nibble = bytes[i] >> 4;
        let mut repeat = u32::from(bytes[i] & 0xf);
        while i + 1 < bytes.len() && bytes[i + 1] >> 4 == nibble {
            i += 1;
            repeat = (repeat << 4) | u32::from(bytes[i] & 0xf);
        }
        out.extend(std::iter::repeat_n(nibble << 4 | nibble, repeat as usize));
        i += 1;
    }
    out
}

fn written(format: OtherFormat, layers: usize) -> (Vec<u8>, Vec<Vec<u8>>) {
    let raster = raster(AaLevel::X8, Mirror { x: true, y: false });
    let program = program("Orange 30", raster.clone(), layers);
    let stack = stack(layers);
    let mut out = Cursor::new(Vec::new());
    write_file(format, &program, None, &stack, &mut out).unwrap();
    (out.into_inner(), expected_images(&stack, &raster))
}

fn layers_after(bytes: &[u8], at: usize, count: usize) -> Vec<Vec<u8>> {
    let mut at = at;
    let mut layers = Vec::new();
    for _ in 0..count {
        let size = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        layers.push(decode(&bytes[at + 4..at + 4 + size]));
        at += 4 + size;
    }
    assert_eq!(at, bytes.len(), "nothing follows the last layer");
    layers
}

#[test]
fn the_runs_read_back_as_the_layers_at_16_greys() {
    let (bytes, expected) = written(OtherFormat::Lgs30, 5);
    assert_eq!(&bytes[..8], NAME);
    assert_eq!(bytes.len() > 180 + 120 * 150 * 2, true);
    let layers = layers_after(&bytes, 180 + 120 * 150 * 2, 5);
    for (layer, image) in layers.iter().zip(&expected) {
        let quantised: Vec<u8> = image.iter().map(|&g| stored_grey(g)).collect();
        assert_eq!(layer, &quantised);
    }
}

#[test]
fn the_orange_4k_reads_its_layers_turned_a_quarter_clockwise() {
    let (bytes, expected) = written(OtherFormat::Lgs4k, 2);
    let layers = layers_after(&bytes, 180 + 120 * 150 * 2, 2);
    let (w, h) = (PIXELS_X as usize, PIXELS_Y as usize);
    for (layer, image) in layers.iter().zip(&expected) {
        for y in 0..w {
            for x in 0..h {
                assert_eq!(layer[y * h + x], stored_grey(image[(h - 1 - x) * w + y]));
            }
        }
    }
}

#[test]
fn a_long_span_takes_a_byte_per_nibble() {
    assert_eq!(encode_layer(&[0xff; 0x123]), [0xf1, 0xf2, 0xf3]);
    assert_eq!(
        decode(&encode_layer(&[0, 0, 0x80, 0x8f, 255])),
        [0, 0, 0x88, 0x88, 255]
    );
}
