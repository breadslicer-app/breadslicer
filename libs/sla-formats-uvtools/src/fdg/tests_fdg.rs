/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::test_fixture::{expected_images, program, raster, stack};

/// Decodes one layer image as the machine does.
fn decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut last = 0_u8;
    for &code in bytes {
        if code & 0x80 == 0x80 {
            last = ((code & 0x7f) << 1) | (code & 1);
            if last >= 0xfc {
                last = 0xff;
            }
            out.push(last);
        } else {
            out.extend(std::iter::repeat_n(last, code as usize));
        }
    }
    out
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[test]
fn every_layer_reads_back_at_7_bit_greys_from_where_its_record_says() {
    let raster = raster(AaLevel::X4, Mirror { x: true, y: false });
    let program = program("Proxima 6", raster.clone(), 6);
    let stack = stack(6);
    let mut out = Cursor::new(Vec::new());
    write_file(&program, None, &stack, &mut out).unwrap();
    let bytes = out.into_inner();
    assert_eq!(u32_at(&bytes, 0), MAGIC);
    assert_eq!(u32_at(&bytes, 8), 6);
    let records = u32_at(&bytes, 52) as usize;
    for (index, image) in expected_images(&stack, &raster).iter().enumerate() {
        let record = records + index * 36;
        let (at, size) = (
            u32_at(&bytes, record + 12) as usize,
            u32_at(&bytes, record + 16) as usize,
        );
        let quantised: Vec<u8> = image.iter().map(|&g| stored_grey(g)).collect();
        assert_eq!(decode(&bytes[at..at + size]), quantised, "layer {index}");
    }
}

#[test]
fn a_run_breaks_at_the_middle_of_its_row_and_every_125_pixels() {
    let runs = [RleRun {
        value: 255,
        length: 300,
    }];
    // One 300-pixel row: 150 then 150, each a grey byte and its extra 149.
    assert_eq!(encode_layer(&runs, 300), [0xfc, 125, 24, 0xfc, 125, 24]);
}
