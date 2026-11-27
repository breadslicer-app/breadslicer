/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::test_fixture::{PIXELS_X, PIXELS_Y, expected_images, program, raster, stack};

/// Reads a layer's runs back as UVtools does, from its first lit row.
fn decode(layer: &[u8], width: usize, height: usize) -> (u32, Vec<u8>, usize) {
    assert_eq!(&layer[..2], &[0x0D, 0x0A]);
    let lines = u32::from_be_bytes(layer[2..6].try_into().unwrap());
    let start_y = u16::from_be_bytes(layer[6..8].try_into().unwrap()) as usize;
    let mut image = vec![0_u8; width * height];
    let mut pixel = start_y * width;
    let mut n = 8;
    for _ in 0..lines {
        let mut code = layer[n];
        n += 1;
        let mut stride = 1_usize;
        if code & 1 == 1 {
            code &= 0xfe;
            let s = layer[n] as usize;
            n += 1;
            stride = if s & 0x80 == 0 {
                s
            } else if s & 0xc0 == 0x80 {
                n += 1;
                ((s & 0x3f) << 8) + layer[n - 1] as usize
            } else if s & 0xe0 == 0xc0 {
                n += 2;
                ((s & 0x1f) << 16) + ((layer[n - 2] as usize) << 8) + layer[n - 1] as usize
            } else {
                n += 3;
                ((s & 0x0f) << 24)
                    + ((layer[n - 3] as usize) << 16)
                    + ((layer[n - 2] as usize) << 8)
                    + layer[n - 1] as usize
            };
        }
        image[pixel..pixel + stride].fill(code);
        pixel += stride;
    }
    (lines, image, n)
}

fn runs_of(pixels: &[u8]) -> Vec<RleRun> {
    let mut runs: Vec<RleRun> = Vec::new();
    for &value in pixels {
        match runs.last_mut() {
            Some(run) if run.value == value => run.length += 1,
            _ => runs.push(RleRun { value, length: 1 }),
        }
    }
    runs
}

#[test]
fn a_layer_reads_back_from_its_first_lit_row_with_its_greys_even() {
    let (width, height) = (300_usize, 80_usize);
    let mut pixels = vec![0_u8; width * height];
    // A single pixel, a short run, one past 0x7f and one past 0x3fff.
    pixels[3 * width + 5] = 255;
    pixels[3 * width + 10..3 * width + 20].fill(201);
    pixels[5 * width..5 * width + 200].fill(128);
    pixels[10 * width..70 * width + 42].fill(77);
    let (lines, image, read) = decode(
        &encode_layer(&runs_of(&pixels), width as u32),
        width,
        height,
    );
    assert_eq!(read, encode_layer(&runs_of(&pixels), width as u32).len());
    let expected: Vec<u8> = pixels.iter().map(|&p| stored_grey(p)).collect();
    assert_eq!(image, expected);
    // The rest of the last row is the last run; nothing past it is written.
    let layer = encode_layer(&runs_of(&pixels), width as u32);
    assert_eq!(u16::from_be_bytes(layer[6..8].try_into().unwrap()), 3);
    assert!(lines > 0);
}

#[test]
fn the_settings_fill_their_126_bytes_with_the_program() {
    let mut program = program("Phenom Forge", raster(AaLevel::Off, Mirror::default()), 6);
    program.transition_layer_count = 1;
    let bytes = settings(&program);
    assert_eq!(bytes.len(), 126);
    let be16 = |at: usize| u16::from_be_bytes(bytes[at..at + 2].try_into().unwrap());
    let be24 = |at: usize| u32::from_be_bytes([0, bytes[at], bytes[at + 1], bytes[at + 2]]);
    assert_eq!((be16(0), be16(2)), (PIXELS_X as u16, PIXELS_Y as u16));
    assert_eq!(be24(22), 5000, "the layer height");
    assert_eq!(bytes[25], 2, "the bottom layers");
    assert_eq!(be24(26), 250, "the exposure");
    assert_eq!(be24(29), 3500, "the bottom exposure");
    assert_eq!(bytes[38], 1, "the transition layers");
    assert_eq!(be24(49), 150, "the wait before cure");
    assert_eq!((be24(52), be24(55)), (7000, 7000), "the bottom lift");
    assert_eq!((be24(58), be24(61)), (6000, 6000), "the lift");
    assert_eq!(be16(79), 60, "the bottom lift speed");
    assert_eq!(be16(86), 90, "the lift speed");
    assert_eq!(be16(95), 150, "the bottom retract speed");
    assert_eq!(be16(121), 300, "the bottom wait before cure");
}

#[test]
fn a_whole_file_reads_back_layer_by_layer() {
    let raster = raster(AaLevel::X4, Mirror { x: true, y: false });
    let program = program("Phenom Forge", raster.clone(), 6);
    let stack = stack(6);
    let mut out = Cursor::new(Vec::new());
    write_file(&program, None, &stack, &mut out).unwrap();
    let file = out.into_inner();
    let header_length = u32::from_be_bytes(file[..4].try_into().unwrap()) as usize;
    assert_eq!(u16::from_be_bytes([file[4], file[5]]), 4);
    let previews = [148 * 80 * 2, 300 * 140 * 2, 208 * 116 * 2, 404 * 240 * 2];
    assert_eq!(header_length, 7 + 126 + 12 + previews.iter().sum::<usize>());
    let mut settings_at = 7;
    for bytes in previews {
        let length = u32::from_be_bytes([
            0,
            file[settings_at],
            file[settings_at + 1],
            file[settings_at + 2],
        ]);
        assert_eq!(length as usize, bytes);
        settings_at += 3 + bytes;
    }
    assert_eq!(file[settings_at + 6], 1, "mirrored in X");
    let (width, height) = (PIXELS_X as usize, PIXELS_Y as usize);
    let mut at = header_length;
    for expected in expected_images(&stack, &raster) {
        let (_, image, read) = decode(&file[at..], width, height);
        let expected: Vec<u8> = expected.iter().map(|&p| stored_grey(p)).collect();
        assert_eq!(image, expected);
        at += read;
    }
    assert_eq!(at, file.len());
}
