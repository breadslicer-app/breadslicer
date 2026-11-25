/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::read::decode_ctb_layer;
use crate::test_fixture::{PIXELS_X, PIXELS_Y, expected_images, program, raster, stack};

fn written(machine: &str, aa: AaLevel) -> (Vec<u8>, Vec<Vec<u8>>) {
    let raster = raster(aa, Mirror { x: true, y: false });
    let program = program(machine, raster.clone(), 6);
    let stack = stack(6);
    let mut out = Cursor::new(Vec::new());
    write_file(&program, None, &stack, &mut out).unwrap();
    (out.into_inner(), expected_images(&stack, &raster))
}

fn u32_le(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Reads the model and every layer back, as UVtools decodes them.
fn read_back(bytes: &[u8]) -> (String, Vec<Vec<u8>>) {
    let model_length = u32::from_be_bytes(bytes[15..19].try_into().unwrap()) as usize;
    let model = String::from_utf8(bytes[19..19 + model_length - 1].to_vec()).unwrap();
    let fields = 19 + model_length;
    let resolution_x = u16::from_le_bytes(bytes[fields..fields + 2].try_into().unwrap()) as u32;
    let resolution_y = u16::from_le_bytes(bytes[fields + 2..fields + 4].try_into().unwrap()) as u32;
    // After the panel: 5 floats, then the bottom count, the small preview,
    // the table and the layer count.
    let table = u32_le(bytes, fields + 4 + 20 + 8) as usize;
    let count = u32_le(bytes, fields + 4 + 20 + 12) as usize;
    let codec = codec_for(&model, (resolution_x, resolution_y));
    let pixels = (resolution_x * resolution_y) as usize;
    let layers = (0..count)
        .map(|index| {
            let record = table + index * LAYER_DEF_BYTES as usize;
            let address = u32_le(bytes, record + 12) as usize;
            let size = u32_le(bytes, record + 16) as usize;
            let image = &bytes[address + LAYER_DEF_EX_BYTES as usize..address + size];
            match codec {
                LayerCodec::Rle7 => decode_ctb_layer(image, pixels).unwrap(),
                LayerCodec::PixelPairs => decode_pixel_pairs(image, resolution_x as usize, pixels),
            }
        })
        .collect();
    (model, layers)
}

fn decode_pixel_pairs(rle: &[u8], width: usize, pixels: usize) -> Vec<u8> {
    let mut image = vec![0_u8; pixels];
    let mut unit = 0_usize;
    let mut at = 0;
    while at < rle.len() {
        let code = rle[at];
        at += 1;
        assert_eq!(code & 0xc0, 0x80, "a pair opens every unit");
        let mut repeat = 0_usize;
        while at < rle.len() && rle[at] & 0x80 == 0 {
            repeat = (repeat << 7) | usize::from(rle[at]);
            at += 1;
        }
        assert!(
            unit % (width / 2) + repeat < width / 2,
            "a run stays in its row"
        );
        let expand = |gray3: u8| (gray3 << 5) | (gray3 << 2) | (gray3 >> 1);
        let (left, right) = (expand(code & 7), expand((code >> 3) & 7));
        for _ in 0..=repeat {
            image[unit * 2] = left;
            image[unit * 2 + 1] = right;
            unit += 1;
        }
    }
    assert_eq!(unit * 2, pixels, "every pixel is covered");
    image
}

#[test]
fn shifting_a_checksum_past_zeros_joins_2_spans() {
    let (first, second) = (b"Breadslicer ".as_slice(), b"CXDLP v4 checksum".as_slice());
    let whole = crc32_zero_start(0, &[first, second].concat());
    let joined =
        crc32_shift(crc32_zero_start(0, first), second.len() as u64) ^ crc32_zero_start(0, second);
    assert_eq!(joined, whole);
    assert_eq!(crc32_shift(0x1234_5678, 0), 0x1234_5678);
    let zeros = vec![0_u8; 1000];
    assert_eq!(
        crc32_shift(crc32_zero_start(0, first), 1000),
        crc32_zero_start(crc32_zero_start(0, first), &zeros)
    );
}

#[test]
fn the_file_ends_with_the_checksum_of_everything_before_it() {
    for machine in ["Halot Mage", "Halot X1"] {
        let (bytes, _) = written(machine, AaLevel::X4);
        let (body, tail) = bytes.split_at(bytes.len() - 4);
        assert_eq!(
            u32::from_be_bytes(tail.try_into().unwrap()),
            crc32_zero_start(0, body),
            "{machine}"
        );
    }
}

#[test]
fn every_layer_reads_back_as_the_slicer_rasterised_it() {
    let (bytes, expected) = written("Halot Mage", AaLevel::X4);
    let (model, layers) = read_back(&bytes);
    assert_eq!(model, "CL-103L");
    let stored: Vec<Vec<u8>> = expected
        .iter()
        .map(|image| image.iter().map(|&grey| rle7::stored_grey(grey)).collect())
        .collect();
    assert_eq!(layers, stored);
}

#[test]
fn a_halot_x1_keeps_its_layers_in_pixel_pairs() {
    let (bytes, expected) = written("Halot X1 Max", AaLevel::X4);
    let (model, layers) = read_back(&bytes);
    assert_eq!(model, "HALOT-X1");
    let stored: Vec<Vec<u8>> = expected
        .iter()
        .map(|image| image.iter().map(|&grey| pixel_pair_grey(grey)).collect())
        .collect();
    assert_eq!(layers, stored);
    assert_eq!(layers[0].len(), (PIXELS_X * PIXELS_Y) as usize);
}

#[test]
fn the_model_follows_what_uvtools_writes() {
    assert_eq!(printer_model("Halot Mage"), "CL-103L");
    assert_eq!(printer_model("Halot Mage Pro"), "CL-103");
    assert_eq!(printer_model("Halot X1"), "HALOT-X1");
    assert_eq!(printer_model("Halot X1 Neo"), "HALOT-X1");
    assert_eq!(printer_model("Creality Halot Mage CL-103L"), "CL-103L");
    assert_eq!(printer_model("Halot R6"), "Halot R6");
}

#[test]
fn a_long_row_run_counts_in_7_bit_groups() {
    let runs = [RleRun {
        value: 255,
        length: 400,
    }];
    let mut out = Vec::new();
    encode_pixel_pairs(&runs, 400, &mut out);
    // 200 pairs: the pair, then 199 as 1 and 71.
    assert_eq!(out, vec![0xbf, 1, 71]);
}
