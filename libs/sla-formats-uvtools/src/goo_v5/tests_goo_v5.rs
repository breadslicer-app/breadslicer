/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::test_fixture::{PIXELS_X, PIXELS_Y, expected_images, program, raster, stack};

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Reads a framed block back as UVtools' decoder does, in 8-bit greys.
fn decode(block: &[u8], pixels: usize) -> Vec<u8> {
    assert_eq!(block[0], LAYER_MAGIC);
    let last = block.len() - 1;
    let sum = block[1..last].iter().fold(0_u8, |s, &b| s.wrapping_add(b));
    assert_eq!(block[last], !sum, "the checksum");
    let max = u32::from(GREY_MAX);
    let lut = |v: u8| ((u32::from(v) * 255 + max / 2) / max).min(255) as u8;
    let mut out = Vec::with_capacity(pixels);
    let mut previous = 0_u8;
    let mut i = 1;
    while i < last {
        let tag = block[i];
        match tag >> 6 {
            1 => {
                let opt = (tag >> 4) & 3;
                let mut count = u32::from(tag & 0xF);
                for s in 0..opt {
                    i += 1;
                    count |= u32::from(block[i]) << (4 + 8 * u32::from(s));
                }
                out.extend(std::iter::repeat_n(lut(previous), count as usize + 1));
            }
            2 => {
                let diff = if tag == 0xA0 {
                    32
                } else {
                    i32::from(tag & 0x3F) - 32
                };
                previous = (i32::from(previous) + diff) as u8;
                assert!(previous <= GREY_MAX);
                out.push(lut(previous));
            }
            0 => {
                let bits = (tag >> 3) & 7;
                let (count, value) = if bits != 0 {
                    let v = if tag & 7 == 7 { GREY_MAX } else { tag & 7 };
                    (u32::from(bits), v)
                } else {
                    i += 1;
                    (u32::from(tag & 7) + 1, block[i])
                };
                out.extend(std::iter::repeat_n(lut(value), count as usize));
                previous = value;
            }
            _ => panic!("chunk type 3"),
        }
        i += 1;
    }
    assert_eq!(out.len(), pixels);
    out
}

#[test]
fn a_partition_reads_back_at_3_bits_through_every_chunk() {
    let runs = vec![
        (0, 3),
        (255, 1),
        (120, 2),
        (120, 20),
        (40, 9),
        (0, 5000),
        (255, 70_000),
        (200, 1),
        (0, 2_000_000),
    ];
    let pixels: usize = runs.iter().map(|&(_, n)| n as usize).sum();
    let mut expected = Vec::new();
    for &(grey, n) in &runs {
        expected.extend(std::iter::repeat_n(stored_grey(grey), n as usize));
    }
    assert_eq!(decode(&encode_partition(&runs), pixels), expected);
}

#[test]
fn stored_greys_are_the_8_levels() {
    assert_eq!(stored_grey(0), 0);
    assert_eq!(stored_grey(18), 0);
    assert_eq!(stored_grey(19), 36);
    assert_eq!(stored_grey(255), 255);
}

#[test]
fn a_whole_file_names_where_each_piece_lies_and_ends_on_its_md5() {
    let raster = raster(AaLevel::X4, Mirror { x: true, y: false });
    let program = program("Jupiter 2", raster.clone(), 6);
    let stack = stack(6);
    let mut out = Cursor::new(Vec::new());
    write_file(&program, None, &stack, &mut out).unwrap();
    let file = out.into_inner();
    assert_eq!(&file[..4], b"V5.1");
    let (content, md5) = file.split_at(file.len() - 32);
    let hex: String = Md5::digest(content)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(md5, hex.as_bytes());

    let tail = HEADER_BYTES as usize - 19;
    let addresses = [
        le32(&file, tail + 1),
        le32(&file, tail + 5),
        le32(&file, tail + 9),
    ];
    assert_eq!(addresses[0], HEADER_BYTES);
    assert_eq!(file[addresses[0] as usize], LDT_MAGIC);
    assert_eq!(file[addresses[1] as usize], IEDT_MAGIC);
    assert_eq!(file[addresses[2] as usize], RDT_MAGIC);
    assert_eq!(
        le32(&file, tail - 7),
        HEADER_BYTES - 19,
        "the V3 header length"
    );

    let (width, height) = (PIXELS_X as usize, PIXELS_Y as usize);
    let half = width / 2;
    let images = expected_images(&stack, &raster);
    for (index, expected) in images.iter().enumerate() {
        let entry = addresses[0] as usize + 1 + index * 8;
        let at = le32(&file, entry) as usize;
        assert_eq!(le32(&file, entry + 4), LAYER_DEF_BYTES);
        assert_eq!(&file[at + 64..at + 66], &DELIMITER);
        let mut image = vec![0_u8; width * height];
        for side in 0..2 {
            let entry = addresses[1] as usize + 1 + (index * 2 + side) * 8;
            let (at, size) = (le32(&file, entry) as usize, le32(&file, entry + 4) as usize);
            assert_eq!(&file[at + size - 2..at + size], &DELIMITER);
            let part = decode(&file[at..at + size - 2], half * height);
            for y in 0..height {
                image[y * width + side * half..y * width + (side + 1) * half]
                    .copy_from_slice(&part[y * half..(y + 1) * half]);
            }
        }
        let want: Vec<u8> = expected.iter().map(|&p| stored_grey(p)).collect();
        assert_eq!(image, want, "layer {index}");
    }
    let resin = addresses[2] as usize;
    assert_eq!(le32(&file, resin + 1), 1);
    let at = le32(&file, resin + 5) as usize;
    assert_eq!(le32(&file, resin + 9) as usize, RESIN_BYTES);
    assert_eq!(file[at], RESIN_MAGIC);
    assert_eq!(at + RESIN_BYTES, content.len());
}
