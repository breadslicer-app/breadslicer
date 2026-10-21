/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::header::write_header;
use super::layout::{HEADER_SIZE, Layout, SETTINGS_SIZE, SIGNATURE_SIZE, VariableSections};

/// Where a file written by the vendor's own slicer put its
/// signature, beside the 48 bytes it opens with.
const REAL_SIGNATURE_OFFSET: u32 = 11_016_036;
const REAL_HEADER: [u8; 48] = [
    0x07, 0x01, 0xFD, 0x12, 0x20, 0x01, 0x00, 0x00, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x05, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x64, 0x17, 0xA8, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

fn layout() -> Layout {
    Layout::plan(VariableSections {
        large_preview_bytes: 41_776,
        small_preview_bytes: 41_776,
        machine_name_bytes: 19,
        resin_parameters_bytes: 59,
        layer_count: 960,
    })
}

fn written(signature_offset: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_header(&layout(), signature_offset, &mut bytes).unwrap();
    bytes
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn the_header_is_the_size_the_layout_reserves_for_it() {
    assert_eq!(written(0).len(), HEADER_SIZE as usize);
}

#[test]
fn the_file_is_named_by_its_magic_and_its_version() {
    let bytes = written(0);

    assert_eq!(u32_at(&bytes, 0), 0x12FD_0107);
    assert_eq!(u32_at(&bytes, 16), 5);
}

#[test]
fn the_settings_block_is_located_and_measured() {
    let bytes = written(0);

    assert_eq!(u32_at(&bytes, 4), SETTINGS_SIZE);
    assert_eq!(u32_at(&bytes, 8), layout().settings);
}

#[test]
fn the_signature_is_located_and_measured() {
    let bytes = written(REAL_SIGNATURE_OFFSET);

    assert_eq!(u32_at(&bytes, 20), SIGNATURE_SIZE);
    assert_eq!(u32_at(&bytes, 24), REAL_SIGNATURE_OFFSET);
}

#[test]
fn every_reserved_word_is_left_at_zero() {
    let bytes = written(REAL_SIGNATURE_OFFSET);

    for offset in [12, 28, 36, 44] {
        assert_eq!(u32_at(&bytes, offset), 0, "offset {offset}");
    }
}

#[test]
fn the_unexplained_constants_are_written_as_a_real_file_carries_them() {
    let bytes = written(0);

    assert_eq!(u32_at(&bytes, 32), 0x0001_0001, "a pair of 16-bit ones");
    assert_eq!(u32_at(&bytes, 40), 42);
}

#[test]
fn the_header_reproduces_a_real_files_own_bytes() {
    assert_eq!(written(REAL_SIGNATURE_OFFSET), REAL_HEADER);
}
