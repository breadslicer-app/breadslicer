/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::cipher::{BLOCK_BYTES, encrypt_in_place, signature};

/// The signature two real files carry, beside the value each states
/// in its settings block. The files come from different producers,
/// so reproducing both pins the key, the initialisation vector, the
/// digest and the order they are applied in.
const VENDOR_SLICER_VALUE: u64 = 0;
const VENDOR_SLICER_SIGNATURE: [u8; 32] = [
    0x1E, 0x9C, 0x9A, 0x1E, 0x76, 0x89, 0x20, 0x1E, 0xCF, 0x60, 0xBB, 0xE2, 0x65, 0xF7, 0xE8, 0x40,
    0x87, 0xE9, 0xCF, 0xB5, 0x65, 0x65, 0x02, 0x39, 0x13, 0xD3, 0x5E, 0xE7, 0x1A, 0xCF, 0x87, 0xDC,
];
const THIRD_PARTY_SLICER_VALUE: u64 = 0x6A99_9FED;
const THIRD_PARTY_SLICER_SIGNATURE: [u8; 32] = [
    0xAE, 0x19, 0x41, 0x0A, 0xA2, 0x3F, 0x49, 0x7C, 0x2C, 0x1B, 0x42, 0xF6, 0x9C, 0xC2, 0x64, 0x2D,
    0x18, 0x5D, 0xF4, 0xF9, 0xD6, 0xAA, 0xC5, 0x99, 0x16, 0xDF, 0x5D, 0xFA, 0x14, 0xC7, 0x73, 0xAD,
];

#[test]
fn a_signature_reproduces_the_one_a_real_file_carries() {
    assert_eq!(signature(VENDOR_SLICER_VALUE), VENDOR_SLICER_SIGNATURE);
    assert_eq!(
        signature(THIRD_PARTY_SLICER_VALUE),
        THIRD_PARTY_SLICER_SIGNATURE
    );
}

#[test]
fn each_block_carries_the_one_before_it() {
    let mut repeated = [0; 2 * BLOCK_BYTES];
    encrypt_in_place(&mut repeated);

    assert_ne!(repeated[..BLOCK_BYTES], repeated[BLOCK_BYTES..]);
}

#[test]
fn a_range_is_encrypted_where_it_stands() {
    let mut data = [0; BLOCK_BYTES];
    encrypt_in_place(&mut data);

    assert_ne!(data, [0; BLOCK_BYTES]);
}
